#![deny(unsafe_op_in_unsafe_fn)]
#[cfg(windows)]
fn main() -> std::io::Result<()> {
    // This binary is launched by tests/console.rs with CREATE_NEW_CONSOLE.
    // Refuse accidental manual execution in the host's console.
    if std::env::args().nth(1).as_deref() != Some("--isolated-console") {
        return Err(std::io::Error::other("use cargo test --test console"));
    }
    // SAFETY: test harness contract requires a private console for this invocation.
    assert_eq!(
        // SAFETY: harness launched this process with CREATE_NEW_CONSOLE.
        unsafe { windlass_iocp_spike::console::isolated_probe() }?,
        2
    );
    std::process::exit(42);
}
#[cfg(not(windows))]
fn main() {
    eprintln!("Windows-only console test helper");
}
