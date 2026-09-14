use serde_json::Value;
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    time::{Duration, Instant},
};
use turnloop_mongodb::{
    bson::doc,
    topology::{ServerType, Topology, TopologyType},
    uri::{Options, ReadPreference},
};
fn kind(s: &str) -> ServerType {
    match s {
        "RSPrimary" => ServerType::RSPrimary,
        "RSSecondary" => ServerType::RSSecondary,
        "RSArbiter" => ServerType::RSArbiter,
        "RSOther" => ServerType::RSOther,
        "RSGhost" => ServerType::RSGhost,
        "PossiblePrimary" => ServerType::PossiblePrimary,
        "Mongos" => ServerType::Mongos,
        "Standalone" => ServerType::Standalone,
        "Unknown" => ServerType::Unknown,
        _ => panic!("Unexpected server type {s}"),
    }
}
fn files(root: &Path, out: &mut Vec<std::path::PathBuf>) {
    for e in fs::read_dir(root).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            files(&p, out)
        } else if p.extension().is_some_and(|e| e == "json") {
            out.push(p)
        }
    }
}
fn run(path: &Path) {
    let v: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let now = Instant::now();
    let opts = Options::parse("mongodb://a/?directConnection=true").unwrap();
    let mut t = Topology::new(&opts, now);
    t.servers.clear();
    for s in v["topology_description"]["servers"].as_array().unwrap() {
        let a = s["address"].as_str().unwrap();
        let o = Options::parse(&format!("mongodb://{a}/?directConnection=true")).unwrap();
        let mut single = Topology::new(&o, now);
        single.update(
            a,
            &doc! {"ok":1,"maxWireVersion":27},
            now,
            Duration::from_secs_f64(s["avg_rtt_ms"].as_f64().unwrap_or(0.0) / 1000.0),
        );
        let mut server = single.servers.remove(a).unwrap();
        server.kind = kind(s["type"].as_str().unwrap());
        server.last_update = now + Duration::from_millis(s["lastUpdateTime"].as_u64().unwrap_or(0));
        server.last_write_ms = s["lastWrite"]["lastWriteDate"]["$numberLong"]
            .as_str()
            .map(|v| v.parse::<i64>().unwrap());
        server.max_wire_version = s["maxWireVersion"].as_i64().unwrap_or(27) as i32;
        server.rtt = s["avg_rtt_ms"]
            .as_f64()
            .map(|n| Duration::from_secs_f64(n / 1000.0));
        if let Some(tags) = s["tags"].as_object() {
            server.tags = tags
                .iter()
                .map(|(k, v)| (k.clone(), v.as_str().unwrap().to_owned()))
                .collect();
        }
        t.servers.insert(a.into(), server);
    }
    t.kind = match v["topology_description"]["type"].as_str().unwrap() {
        "Single" => TopologyType::Single,
        "Unknown" => TopologyType::Unknown,
        "Sharded" => TopologyType::Sharded,
        "ReplicaSetNoPrimary" => TopologyType::ReplicaSetNoPrimary,
        "ReplicaSetWithPrimary" => TopologyType::ReplicaSetWithPrimary,
        x => panic!("Unknown topology {x}"),
    };
    t.heartbeat = Duration::from_millis(v["heartbeatFrequencyMS"].as_u64().unwrap_or(10000));
    let p = &v["read_preference"];
    let pref = if v["operation"] == "write" {
        ReadPreference::Primary
    } else {
        match p["mode"].as_str().unwrap_or("Primary") {
            "Primary" => ReadPreference::Primary,
            "PrimaryPreferred" => ReadPreference::PrimaryPreferred,
            "Secondary" => ReadPreference::Secondary,
            "SecondaryPreferred" => ReadPreference::SecondaryPreferred,
            "Nearest" => ReadPreference::Nearest,
            x => panic!("Mode {x}"),
        }
    };
    let tags: Vec<BTreeMap<String, String>> = if v["operation"] == "write" {
        Vec::new()
    } else {
        p["tag_sets"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|t| {
                t.as_object()
                    .unwrap()
                    .iter()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap().into()))
                    .collect()
            })
            .collect()
    };
    let mut actual = Vec::new();
    let result = t.candidates_deprioritized(
        pref,
        &tags,
        p["maxStalenessSeconds"]
            .as_i64()
            .filter(|v| *v >= 0)
            .map(|v| Duration::from_secs(v as u64)),
        &v["deprioritized_servers"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|s| s["address"].as_str().unwrap())
            .collect::<Vec<_>>(),
        &mut actual,
    );
    if v["error"] == true {
        assert!(result.is_err());
        return;
    }
    result.unwrap();
    actual.sort();
    let mut expected: Vec<_> = v["in_latency_window"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["address"].as_str().unwrap())
        .collect();
    expected.sort();
    assert_eq!(actual, expected, "{}", path.display());
}
#[test]
fn official_server_selection() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/spec/selection");
    let mut list = Vec::new();
    files(&root, &mut list);
    list.sort();
    assert!(!list.is_empty());
    let mut failures = Vec::new();
    for p in &list {
        if std::panic::catch_unwind(|| run(p)).is_err() {
            failures.push(p.strip_prefix(&root).unwrap().display().to_string());
        }
    }
    eprintln!(
        "Server selection: {} fixtures, {} failures",
        list.len(),
        failures.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn official_max_staleness() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/spec/staleness");
    let mut list = Vec::new();
    files(&root, &mut list);
    list.sort();
    assert_eq!(list.len(), 32);
    let mut failures = Vec::new();
    for p in &list {
        if std::panic::catch_unwind(|| run(p)).is_err() {
            failures.push(p.strip_prefix(&root).unwrap().display().to_string());
        }
    }
    eprintln!(
        "Max staleness: {} fixtures, {} failures",
        list.len(),
        failures.len()
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
