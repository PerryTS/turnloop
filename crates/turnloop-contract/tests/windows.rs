#![cfg(all(windows, not(loom)))]
#![deny(unsafe_op_in_unsafe_fn)]
use turnloop::{backend::Platform, *};

macro_rules! contract {
    ($($name:ident),+ $(,)?) => { $(#[test] fn $name() { turnloop_contract::$name::<Platform>(); })+ };
}
contract!(
    bounded_turn,
    notify_parked,
    notify_running,
    queued_core_work,
    queued_post_idle_io,
    queued_terminals_idle_io,
    sustained_posts_idle_io,
    cancel_close_ordering,
    refused_connect_once,
    tcp_connect_timeout,
    single_connection_config,
    ref_unref,
    no_spin,
    quiet_deadline_accounting,
    timer_precision,
    udp_round_trip,
    detach_inflight,
    pool_and_dns,
    occupancy_classes_do_not_starve_each_other,
    long_jobs_settle_once_on_cancel_panic_and_shutdown,
    handoff_distribution,
    handoff_accept_exactly_once,
    kernel_accept_exactly_once,
    writev_and_shutdown,
    capacity_and_stale_ids,
    paged_growth_preserves_handles,
    accept_reserves_its_handle_slot,
    an_armed_accept_keeps_its_slot,
    ready_timer_liveness,
    pooled_lease_backpressure,
    io_and_posts_progress_with_repeating_timers
);
#[test]
fn echo_one() {
    turnloop_contract::tcp_echo::<Platform>(1);
}
#[test]
fn local_pipe_and_socket_passing() {
    let name = PipeName(format!(r"\\.\pipe\turnloop-contract-{}", std::process::id()).into());
    turnloop_contract::native_surface::ipc::<Platform>(&name);
}
#[test]
fn echo_64() {
    turnloop_contract::tcp_echo::<Platform>(64);
}
#[test]
fn children_256_exit_once() {
    turnloop_contract::native_surface::children::<Platform>(std::ffi::OsStr::new(env!(
        "CARGO_BIN_EXE_native_child"
    )));
}
#[test]
fn child_stdio_runs_the_driver() {
    turnloop_contract::native_surface::child_stdio::<Platform>(std::ffi::OsStr::new(env!(
        "CARGO_BIN_EXE_native_child"
    )));
}
#[test]
fn job_kills_child_and_grandchild() {
    turnloop_contract::native_surface::process_group::<Platform>(std::ffi::OsStr::new(env!(
        "CARGO_BIN_EXE_native_child"
    )));
}
#[test]
fn external_waits_route_and_cancel() {
    turnloop_contract::native_surface::external_waits::<Platform>();
}
#[test]
fn services_do_not_spin() {
    turnloop_contract::native_surface::services_no_spin::<Platform>(
        std::ffi::OsStr::new(env!("CARGO_BIN_EXE_native_child")),
        Signal::Break,
    );
}
#[test]
fn sockets_pass_to_child_and_back() {
    let name = PipeName(format!(r"\\.\pipe\turnloop-process-ipc-{}", std::process::id()).into());
    turnloop_contract::native_surface::ipc_process::<Platform>(
        std::ffi::OsStr::new(env!("CARGO_BIN_EXE_native_child")),
        &name,
    );
}
#[test]
fn four_loops_cross_post() {
    turnloop_contract::cross_post::<Platform>(4, 1000);
}
/// Windows has no SO_REUSEPORT, and SO_REUSEADDR there permits *hijacking* an
/// address rather than sharing it, so it must not stand in for either request.
/// Both are refused, which leaves `detach`/`attach` as the only multi-core
/// accept route on this platform — `handoff_accept_exactly_once` above.
#[test]
fn reuse_port_is_explicitly_unsupported() {
    turnloop_contract::reuse_port_refused::<Platform>(&[ReusePort::Share, ReusePort::Distribute]);
}
#[test]
fn gui_event_receives_cross_thread_posts() {
    use windows_sys::Win32::{
        Foundation::WAIT_OBJECT_0, UI::WindowsAndMessaging::MsgWaitForMultipleObjectsEx,
    };
    let mut driver = Loop::new(Config::default()).expect("loop");
    let Integration::Event(event) = driver.integration().expect("integration") else {
        panic!("event");
    };
    let poster = driver.poster();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(10));
        poster.post(Token(42), Payload::U64(99)).expect("post");
    });
    let event = event as *mut std::ffi::c_void;
    // SAFETY: borrowed event remains owned by the driver for the entire GUI wait.
    assert_eq!(
        // SAFETY: borrowed event remains owned by the driver for the entire GUI wait.
        unsafe { MsgWaitForMultipleObjectsEx(1, &event, 2000, 0, 0) },
        WAIT_OBJECT_0
    );
    let mut out = Completions::default();
    let info = driver.turn(Timeout::Now, &mut out).expect("turn");
    assert_eq!((info.os_waits, info.discovery_polls), (0, 0));
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].token, Token(42));
    assert!(matches!(out[0].result, OpResult::Posted(Payload::U64(99))));
    thread.join().expect("producer");
}

#[test]
fn gui_event_tracks_timer_reset_and_partial_output() {
    use std::time::Duration;
    use windows_sys::Win32::{
        Foundation::WAIT_OBJECT_0, UI::WindowsAndMessaging::MsgWaitForMultipleObjectsEx,
    };
    let mut driver = Loop::new(Config::default()).expect("loop");
    let Integration::Event(event) = driver.integration().expect("integration") else {
        panic!("event");
    };
    let event = event as *mut std::ffi::c_void;
    let mut out = Completions::with_capacity(1);
    for _ in 0..32 {
        let timer = driver
            .timer(driver.now() + Duration::from_secs(30), None, Token(1))
            .expect("timer");
        let deadline = driver.now() + Duration::from_millis(2);
        assert!(driver.timer_reset(timer, deadline));
        let watchdog = driver.now() + Duration::from_secs(2);
        loop {
            assert!(driver.now() < watchdog);
            assert_eq!(
                // SAFETY: borrowed event stays owned by driver throughout this GUI wait.
                unsafe { MsgWaitForMultipleObjectsEx(1, &event, 2000, 0, 0) },
                WAIT_OBJECT_0
            );
            let info = driver.turn(Timeout::Now, &mut out).expect("GUI turn");
            assert_eq!((info.os_waits, info.discovery_polls), (0, 0));
            if !out.is_empty() {
                assert!(driver.now() >= deadline);
                assert_eq!(out.len(), 1);
                assert!(matches!(out[0].result, OpResult::Timer));
                break;
            }
        }
        driver.close(timer, Token(2)).expect("close timer");
        driver.turn(Timeout::Now, &mut out).expect("close turn");
        assert!(matches!(out[0].result, OpResult::Closed));
    }
}
