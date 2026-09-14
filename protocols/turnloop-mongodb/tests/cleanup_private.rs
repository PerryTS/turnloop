mod common;
use common::{ports, Driver};
use std::{
    net::TcpStream,
    time::{Duration, Instant},
};
use turnloop_mongodb::bson::doc;
#[test]
#[ignore = "private server cleanup only"]
fn shutdown_private() {
    let entries = ports();
    let rs: Vec<_> = entries
        .iter()
        .filter(|(n, _)| n.starts_with("rs"))
        .collect();
    if let Some((_, p)) = rs.first() {
        if let Ok(mut d) =
            Driver::connect(&format!("mongodb://127.0.0.1:{p}/?directConnection=true"))
        {
            let members: Vec<_> = rs
                .iter()
                .enumerate()
                .map(|(i, (_, p))| doc! {"_id":i as i32,"host":format!("127.0.0.1:{p}")})
                .collect();
            let _ = d.run(
                "admin",
                doc! {"replSetInitiate":{"_id":"turnloop_test","members":members}},
            );
        }
    }
    let deadline = Instant::now() + Duration::from_secs(40);
    loop {
        let mut done = false;
        for (_, p) in &rs {
            if let Ok(mut d) =
                Driver::connect(&format!("mongodb://127.0.0.1:{p}/?directConnection=true"))
            {
                if d.core
                    .hello
                    .as_ref()
                    .unwrap()
                    .get_bool("ismaster")
                    .unwrap_or(false)
                {
                    let _=d.run("admin",doc!{"createUser":"lane","pwd":"pencil","roles":[{"role":"root","db":"admin"}]});
                    done = true;
                    break;
                }
            }
        }
        if done || rs.is_empty() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "Cleanup replica set election timed out"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    // Let the bootstrap user reach secondaries before stopping the primary.
    std::thread::sleep(Duration::from_secs(1));
    for (name, port) in entries {
        if TcpStream::connect(("127.0.0.1", port)).is_err() {
            continue;
        }
        let tls = if name == "tls" { "&tls=true" } else { "" };
        let uri = format!("mongodb://127.0.0.1:{port}/?directConnection=true{tls}");
        let mut d = Driver::connect(&uri).unwrap();
        if !name.starts_with("rs") {
            let _ = d.run(
                "admin",
                doc! {"createUser":"lane","pwd":"pencil","roles":[{"role":"root","db":"admin"}]},
            );
        }
        drop(d);
        let mut d = Driver::connect(&format!(
            "mongodb://lane:pencil@127.0.0.1:{port}/admin?directConnection=true{tls}"
        ))
        .unwrap();
        let result = d.run("admin", doc! {"shutdown":1,"force":true,"timeoutSecs":0});
        assert!(
            result.is_err_and(|e| e.kind == turnloop_mongodb::ErrorKind::Network),
            "shutdown must close socket"
        );
        let deadline = Instant::now() + Duration::from_secs(20);
        while TcpStream::connect(("127.0.0.1", port)).is_ok() {
            assert!(
                Instant::now() < deadline,
                "Private port {port} still listening"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        eprintln!("Stopped {name} on private port {port}");
    }
}
