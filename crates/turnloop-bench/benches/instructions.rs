#![deny(unsafe_op_in_unsafe_fn)]
#[cfg(target_os = "linux")]
use gungraun::{library_benchmark, library_benchmark_group, main};
#[cfg(target_os = "linux")]
use std::{hint::black_box, time::Duration};
#[cfg(target_os = "linux")]
use turnloop::{Completions, Config, Loop, Timeout, Token};

#[cfg(target_os = "linux")]
#[library_benchmark]
fn control() -> u64 {
    let mut value = black_box(17_u64);
    for _ in 0..1024 {
        value = value.wrapping_mul(6364136223846793005).wrapping_add(1);
    }
    assert_ne!(value, 17);
    black_box(value)
}

#[cfg(target_os = "linux")]
fn setup() -> (Loop, Completions) {
    (
        Loop::new(Config::default()).expect("loop"),
        Completions::default(),
    )
}

// Return the owned fixture to Gungraun's unmeasured teardown. Dropping it in
// these functions counts destruction of all reserved file/service slots as
// idle/notify/timer work. Setup and teardown are one-time loop lifecycle costs.

#[cfg(target_os = "linux")]
#[library_benchmark(setup = setup, teardown = drop)]
fn idle((mut driver, mut out): (Loop, Completions)) -> (Loop, Completions) {
    let mut waits = 0;
    for _ in 0..100 {
        waits += driver
            .turn(Timeout::Now, &mut out)
            .expect("idle turn")
            .os_waits;
        assert!(out.is_empty());
    }
    assert_eq!(waits, 100);
    (driver, out)
}

#[cfg(target_os = "linux")]
#[library_benchmark(setup = setup, teardown = drop)]
fn notify((mut driver, mut out): (Loop, Completions)) -> (Loop, Completions) {
    let notifier = driver.notifier();
    let mut turns = 0;
    for _ in 0..100 {
        notifier.notify().expect("notify");
        driver
            .turn(Timeout::Forever, &mut out)
            .expect("consume wake");
        turns += 1;
    }
    assert_eq!(turns, 100);
    assert_eq!(notifier.wake_syscalls(), 0);
    (driver, out)
}

#[cfg(target_os = "linux")]
#[library_benchmark(setup = setup, teardown = drop)]
fn timer_cancel((mut driver, mut out): (Loop, Completions)) -> (Loop, Completions) {
    let mut cancelled = 0;
    for i in 0..100 {
        let h = driver
            .timer(driver.now() + Duration::from_secs(30), None, Token(i))
            .expect("timer");
        assert!(driver.cancel(driver.timer_op(h).expect("timer op")));
        driver.close(h, Token(i)).expect("close");
        driver
            .turn(Timeout::Now, &mut out)
            .expect("drain cancellation");
        assert_eq!(out.len(), 2);
        assert!(matches!(out[0].result, turnloop::OpResult::Cancelled));
        assert!(matches!(out[1].result, turnloop::OpResult::Closed));
        cancelled += 1;
    }
    assert_eq!(cancelled, 100);
    (driver, out)
}

#[cfg(target_os = "linux")]
library_benchmark_group!(name = operations; benchmarks = control, idle, notify, timer_cancel);
#[cfg(target_os = "linux")]
main!(library_benchmark_groups = operations);
#[cfg(not(target_os = "linux"))]
fn main() {}
