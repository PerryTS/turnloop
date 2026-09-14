#![cfg(windows)]
#![deny(unsafe_op_in_unsafe_fn)]
#[test]
fn acceptex_connectex_echo_zero_read_cancel_and_close() -> std::io::Result<()> {
    let stats = windlass_iocp_spike::tcp::probe()?;
    assert_eq!(stats.accepted, 1);
    assert_eq!(stats.connected, 1);
    assert_eq!(stats.bytes_echoed, 128 * 8);
    assert!(stats.completions >= 515);
    assert!(stats.synchronous > 0);
    assert_eq!(stats.cancelled, 1);
    assert!(stats.closed_after_cancel);
    println!("{stats:?}");
    Ok(())
}
