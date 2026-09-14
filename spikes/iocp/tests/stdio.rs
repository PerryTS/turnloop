#![cfg(windows)]
#![deny(unsafe_op_in_unsafe_fn)]
#[test]
fn synchronous_reader_posts_bytes_and_eof() -> std::io::Result<()> {
    assert_eq!(turnloop_iocp_spike::stdio::probe()?, 17);
    Ok(())
}
