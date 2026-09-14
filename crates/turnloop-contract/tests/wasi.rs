#![cfg(all(
    target_os = "wasi",
    any(
        target_env = "p2",
        all(target_env = "p3", feature = "wasi-p3-experimental")
    )
))]
use turnloop::backend::Platform;
use turnloop_contract as contract;
#[test]
fn bounded_wait() {
    contract::bounded_turn::<Platform>();
}
#[test]
fn running_notify() {
    contract::notify_running::<Platform>();
}
#[test]
fn tcp_one() {
    contract::tcp_echo::<Platform>(1);
}
#[test]
fn tcp_64() {
    contract::tcp_echo::<Platform>(64);
}
#[test]
fn cancel_close() {
    contract::cancel_close_ordering::<Platform>();
}
#[test]
fn refused_connect() {
    contract::refused_connect_once::<Platform>();
}
#[test]
fn ref_unref() {
    contract::ref_unref::<Platform>();
}
#[test]
fn timer_precision() {
    contract::timer_precision::<Platform>();
}
#[test]
fn udp() {
    contract::udp_round_trip::<Platform>();
}
#[test]
fn writev_shutdown() {
    contract::writev_and_shutdown::<Platform>();
}
#[test]
fn capacity_stale_ids() {
    contract::capacity_and_stale_ids::<Platform>();
}
#[test]
fn pooled_backpressure() {
    contract::pooled_lease_backpressure::<Platform>();
}
#[test]
fn timer_liveness() {
    contract::ready_timer_liveness::<Platform>();
}
#[test]
fn timers_io_posts() {
    contract::io_and_posts_progress_with_repeating_timers::<Platform>();
}
#[test]
fn no_spin() {
    contract::no_spin::<Platform>();
}
