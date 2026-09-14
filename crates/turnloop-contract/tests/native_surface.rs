#![cfg(all(not(loom), any(target_vendor = "apple", target_os = "linux", target_os = "android", target_os = "freebsd")))]
use turnloop::*;

#[test]
fn local_echo_and_descriptor_ownership() {
    let path = std::env::temp_dir().join(format!("tl-ipc-{}.sock", std::process::id()));
    let name = PipeName(path.clone());
    turnloop_contract::native_surface::ipc::<backend::Platform>(&name);
    std::fs::remove_file(path).expect("remove listener path");
}

#[test]
fn concurrent_256_children_exit_once() {
    turnloop_contract::native_surface::children::<backend::Platform>(std::ffi::OsStr::new(env!("CARGO_BIN_EXE_native_child")));
}
#[test]
fn spawned_child_stdio_uses_the_driver() {
    turnloop_contract::native_surface::child_stdio::<backend::Platform>(std::ffi::OsStr::new(env!("CARGO_BIN_EXE_native_child")));
}
#[test]
fn signals_reach_four_loops_on_four_threads() {
    turnloop_contract::native_surface::signal_fanout::<backend::Platform>(|| {
        // SAFETY: SIGUSR1 is subscribed by all four loops before the barrier opens.
        assert_eq!(unsafe { libc::kill(libc::getpid(), libc::SIGUSR1) }, 0);
    });
}
#[test]
fn shared_external_wait_service_routes_and_cancels() {
    turnloop_contract::native_surface::external_waits::<backend::Platform>();
}
