#![cfg(all(
    feature = "executor",
    not(loom),
    any(
        target_vendor = "apple",
        target_os = "linux",
        target_os = "android",
        target_os = "freebsd",
        target_os = "windows"
    )
))]
#[test]
fn echo_64() {
    turnloop_contract::executor_contract::executor_echoes_64_real_connections::<
        turnloop::backend::Platform,
    >();
}
#[test]
fn timers_and_drop_cancel() {
    turnloop_contract::executor_contract::sleep_timeout_and_drop_cancel_pending_io::<
        turnloop::backend::Platform,
    >();
}
#[test]
fn borrowed_buffers_may_move() {
    turnloop_contract::executor_contract::pending_future_buffers_may_move_and_shrink::<
        turnloop::backend::Platform,
    >();
}
#[test]
fn udp_stdio_and_explicit_cancellation() {
    turnloop_contract::executor_contract::udp_stdio_and_join_cancel::<turnloop::backend::Platform>(
        std::ffi::OsStr::new(env!("CARGO_BIN_EXE_native_child")),
    );
}
#[test]
fn abandoned_ready_accept_closes_its_socket() {
    turnloop_contract::executor_contract::drop_ready_accept::<turnloop::backend::Platform>();
}

#[test]
fn pending_write_can_replace_its_buffer() {
    turnloop_contract::executor_contract::pending_writes_may_replace_the_caller_slice::<
        turnloop::backend::Platform,
    >();
}

#[test]
fn host_operations_complete_beside_executor_futures() {
    turnloop_contract::executor_contract::host_operations_share_the_loop::<
        turnloop::backend::Platform,
    >();
}
