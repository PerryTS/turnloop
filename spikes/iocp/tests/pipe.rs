#![cfg(windows)]
#![deny(unsafe_op_in_unsafe_fn)]
#[test]
fn overlapped_pipe_pending_connect_and_io() -> std::io::Result<()> {
    assert_eq!(turnloop_iocp_spike::pipe::probe(false)?, 1280);
    Ok(())
}
#[test]
fn overlapped_pipe_client_beats_connect() -> std::io::Result<()> {
    assert_eq!(turnloop_iocp_spike::pipe::probe(true)?, 1280);
    Ok(())
}
