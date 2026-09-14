#![cfg(all(not(loom), any(target_vendor = "apple", target_os = "linux", target_os = "android", target_os = "freebsd")))]
use turnloop::*;

#[test]
fn local_echo_and_descriptor_ownership() {
    let path = std::env::temp_dir().join(format!("tl-ipc-{}.sock", std::process::id()));
    let name = PipeName(path.clone());
    turnloop_contract::native_surface::ipc::<backend::Platform>(&name);
    std::fs::remove_file(path).expect("remove listener path");
}
