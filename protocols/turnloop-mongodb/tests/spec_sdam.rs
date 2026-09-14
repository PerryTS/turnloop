use serde_json::Value;
use std::{
    fs,
    path::Path,
    time::{Duration, Instant},
};
use turnloop_mongodb::{
    bson::{self, Document},
    topology::{ApplicationError, ApplicationErrorKind, Topology, TopologyType},
    uri::Options,
};
fn doc(v: &Value) -> Document {
    bson::Bson::try_from(v.clone())
        .unwrap()
        .as_document()
        .unwrap()
        .clone()
}
fn check_optional<T: std::fmt::Debug + PartialEq>(
    v: &Value,
    k: &str,
    actual: Option<T>,
    parse: impl FnOnce(&Value) -> T,
) {
    if let Some(v) = v.get(k) {
        let expected = if v.is_null() { None } else { Some(parse(v)) };
        assert_eq!(actual, expected, "field {k}");
    }
}
fn check(t: &Topology, v: &Value) {
    assert_eq!(format!("{:?}", t.kind), v["topologyType"].as_str().unwrap());
    check_optional(v, "setName", t.set_name.clone(), |v| {
        v.as_str().unwrap().to_owned()
    });
    check_optional(
        v,
        "logicalSessionTimeoutMinutes",
        t.session_timeout(),
        |v| v.as_i64().unwrap(),
    );
    check_optional(v, "maxSetVersion", t.max_set_version, |v| {
        v.as_i64().unwrap() as i32
    });
    check_optional(v, "maxElectionId", t.max_election_id, |v| {
        doc(&serde_json::json!({"id":v}))
            .get_object_id("id")
            .unwrap()
    });
    if let Some(c) = v.get("compatible") {
        assert_eq!(t.compatible(), c.as_bool().unwrap());
    }
    let servers = v["servers"].as_object().unwrap();
    assert_eq!(
        t.servers.len(),
        servers.len(),
        "server list {:?}",
        t.servers.keys()
    );
    for (a, e) in servers {
        let s = t
            .servers
            .get(a)
            .unwrap_or_else(|| panic!("Missing server {a}"));
        assert_eq!(
            format!("{:?}", s.kind),
            e["type"].as_str().unwrap(),
            "server {a}"
        );
        check_optional(e, "setName", s.set_name.clone(), |v| {
            v.as_str().unwrap().to_owned()
        });
        check_optional(e, "setVersion", s.set_version, |v| {
            v.as_i64().unwrap() as i32
        });
        check_optional(e, "electionId", s.election_id, |v| {
            doc(&serde_json::json!({"id":v}))
                .get_object_id("id")
                .unwrap()
        });
        check_optional(e, "logicalSessionTimeoutMinutes", s.session_timeout, |v| {
            v.as_i64().unwrap()
        });
        check_optional(e, "topologyVersion", s.topology_version.clone(), doc);
        for (k, a) in [
            ("minWireVersion", s.min_wire_version),
            ("maxWireVersion", s.max_wire_version),
        ] {
            if let Some(v) = e.get(k) {
                assert_eq!(a, v.as_i64().unwrap() as i32, "{k} for {a}");
            }
        }
        if let Some(g) = e.get("pool").and_then(|p| p.get("generation")) {
            assert_eq!(s.generation, g.as_u64().unwrap(), "pool generation {a}");
        }
        if let Some(err) = e.get("error") {
            assert!(
                s.error
                    .as_deref()
                    .unwrap_or("")
                    .contains(err.as_str().unwrap()),
                "server error {:?}",
                s.error
            );
        }
    }
}
fn run(path: &Path) -> usize {
    let test: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let opts = Options::parse(test["uri"].as_str().unwrap()).unwrap();
    let now = Instant::now();
    let mut t = Topology::new(&opts, now);
    if path.parent().unwrap().ends_with("single")
        && opts.seeds.len() == 1
        && !opts.raw.contains_key("directconnection")
    {
        t.kind = TopologyType::Single;
    }
    let mut phases = 0;
    for phase in test["phases"].as_array().unwrap() {
        if let Some(responses) = phase["responses"].as_array() {
            for response in responses {
                t.update(
                    response[0].as_str().unwrap(),
                    &doc(&response[1]),
                    now + Duration::from_millis(phases * 1000),
                    Duration::from_millis(5),
                );
            }
        }
        if let Some(errors) = phase["applicationErrors"].as_array() {
            for e in errors {
                let address = e["address"].as_str().unwrap();
                let generation = e["generation"]
                    .as_u64()
                    .unwrap_or_else(|| t.servers[address].generation);
                let response = e.get("response").map(doc);
                t.application_error(
                    address,
                    ApplicationError {
                        generation,
                        max_wire_version: e["maxWireVersion"].as_i64().unwrap_or(0) as i32,
                        handshake_complete: e["when"] == "afterHandshakeCompletes",
                        kind: match e["type"].as_str().unwrap() {
                            "network" => ApplicationErrorKind::Network,
                            "timeout" => ApplicationErrorKind::Timeout,
                            "command" => ApplicationErrorKind::Command,
                            x => panic!("Unhandled error kind {x}"),
                        },
                        response: response.as_ref(),
                    },
                    now,
                );
            }
        }
        check(&t, &phase["outcome"]);
        phases += 1;
    }
    phases as usize
}
#[test]
fn official_sdam_json() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/spec/sdam");
    let mut count = 0;
    let mut phases = 0;
    let mut failures = Vec::new();
    for suite in ["single", "rs", "sharded", "errors"] {
        let mut files: Vec<_> = fs::read_dir(root.join(suite))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        files.sort();
        for file in files {
            count += 1;
            match std::panic::catch_unwind(|| run(&file)) {
                Ok(n) => phases += n,
                Err(_) => failures.push(file.strip_prefix(&root).unwrap().display().to_string()),
            }
        }
    }
    eprintln!(
        "SDAM: {} files, {} successful phases, {} failing files",
        count,
        phases,
        failures.len()
    );
    assert_eq!(count, 177);
    assert!(phases > 200);
    assert!(
        failures.is_empty(),
        "Failed fixtures:\n{}",
        failures.join("\n")
    );
}
