#![cfg(windows)]
#![deny(unsafe_op_in_unsafe_fn)]
#[test]
fn child_overlapped_stdio_job_and_exit_wait() -> std::io::Result<()> {
    assert_eq!(
        turnloop_iocp_spike::process::probe(
            std::path::Path::new(env!("CARGO_BIN_EXE_stdio_child")),
            false
        )?,
        23
    );
    Ok(())
}
#[test]
fn child_exits_before_wait_registration() -> std::io::Result<()> {
    assert_eq!(
        turnloop_iocp_spike::process::probe(
            std::path::Path::new(env!("CARGO_BIN_EXE_stdio_child")),
            true
        )?,
        23
    );
    Ok(())
}

#[test]
fn job_terminates_a_running_child_and_grandchild() -> std::io::Result<()> {
    assert_ne!(
        turnloop_iocp_spike::process::probe_kill_tree(std::path::Path::new(env!(
            "CARGO_BIN_EXE_stdio_child"
        )))?,
        0
    );
    Ok(())
}
