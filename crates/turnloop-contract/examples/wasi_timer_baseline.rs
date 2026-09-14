//! Bare host-clock measurement: deliberately no turnloop calls or types.
#[cfg(not(target_os = "wasi"))]
fn main() {}

#[cfg(all(target_os = "wasi", target_env = "p2"))]
fn sleep_until(at: u64) {
    let pollable = wasip2::clocks::monotonic_clock::subscribe_instant(at);
    let ready = wasip2::io::poll::poll(&[&pollable]);
    assert_eq!(ready, [0]);
}

#[cfg(all(target_os = "wasi", target_env = "p3"))]
fn sleep_until(at: u64) {
    // p3 removed pollables. Use the same async-lowered clock and wait-set
    // primitive as the backend, with one clock task and no driver/executor.
    #[link(wasm_import_module = "wasi:clocks/monotonic-clock@0.3.0")]
    unsafe extern "C" {
        #[link_name = "[async-lower]wait-until"]
        fn deadline(at: u64) -> u32;
    }
    #[link(wasm_import_module = "$root")]
    unsafe extern "C" {
        #[link_name = "[waitable-set-new]"]
        fn new() -> u32;
        #[link_name = "[waitable-join]"]
        fn join(task: u32, set: u32);
        #[link_name = "[waitable-set-wait]"]
        fn wait(set: u32, payload: *mut [u32; 2]) -> u32;
        #[link_name = "[subtask-drop]"]
        fn drop_task(task: u32);
        #[link_name = "[waitable-set-drop]"]
        fn drop_set(set: u32);
    }
    // SAFETY: single owned deadline/set; no borrowed guest memory except the
    // live aligned wait payload, all resources dropped after completion.
    unsafe {
        let packed = deadline(at);
        let task = packed >> 4;
        if task != 0 {
            if packed & 15 < 2 {
                let set = new();
                join(task, set);
                let mut payload = [0; 2];
                let event = wait(set, &mut payload);
                assert_ne!(event, 0);
                assert_eq!(payload, [task, 2]);
                join(task, 0);
                drop_set(set);
            }
            drop_task(task);
        }
    }
}

#[cfg(target_os = "wasi")]
fn main() {
    #[cfg(target_env = "p2")]
    use wasip2::clocks::monotonic_clock::now;
    #[cfg(target_env = "p3")]
    fn now() -> u64 {
        #[link(wasm_import_module = "wasi:clocks/monotonic-clock@0.3.0")]
        unsafe extern "C" {
            #[link_name = "now"]
            fn clock_now() -> u64;
        }
        // SAFETY: scalar clock import, with metadata provided by WASI std.
        unsafe { clock_now() }
    }
    let mut samples = [0; 20];
    for sample in &mut samples {
        let at = now() + 250_000;
        sleep_until(at);
        let end = now();
        assert!(end >= at, "bare clock woke early");
        *sample = end - at;
    }
    println!("bare WASI lateness_ns={samples:?}");
    samples.sort_unstable();
    println!(
        "bare WASI expiries={} median_ns={}",
        samples.len(),
        samples[10]
    );
    assert_eq!(samples.len(), 20);
}
