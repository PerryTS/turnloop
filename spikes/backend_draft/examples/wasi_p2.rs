#![deny(unsafe_op_in_unsafe_fn)]
#[cfg(all(target_os = "wasi", target_env = "p2"))]
fn main() -> Result<(), wasi::sockets::network::ErrorCode> {
    use windlass_wasi_p2_spike::{Completion, ResultKind};
    use windlass_wasm_backend_draft::{DraftBackend, Integration, Timeout, wasi_p2::WasiP2};
    let mut backend = WasiP2::default();
    assert_eq!(backend.integration(), Integration::RuntimeOwned);
    let mut out = Vec::with_capacity(512);
    // Explicitly exercise the wait even if the short timer later expires before turn.
    let probe = DraftBackend::turn(
        &mut backend,
        Timeout::After(std::time::Duration::ZERO),
        &mut out,
    )?;
    assert!(probe.waited && out.is_empty());
    backend.timer(wasi::clocks::monotonic_clock::now() + 500_000, 42)?;
    let limit = wasi::clocks::monotonic_clock::now() + 1_000_000_000;
    let mut waits = usize::from(probe.waited);
    loop {
        let info = DraftBackend::turn(
            &mut backend,
            Timeout::After(std::time::Duration::from_millis(100)),
            &mut out,
        )?;
        waits += usize::from(info.waited);
        if info.completions > 0 {
            break;
        }
        assert!(wasi::clocks::monotonic_clock::now() < limit);
    }
    assert_eq!(
        out,
        [Completion {
            token: 42,
            result: ResultKind::Timer
        }]
    );
    assert!(waits > 0);
    let h = backend.timer(wasi::clocks::monotonic_clock::now() + 1_000_000_000, 43)?;
    assert!(backend.close(h, 44));
    let info = DraftBackend::turn(&mut backend, Timeout::Forever, &mut out)?;
    assert!(!info.waited);
    assert_eq!(
        out,
        [
            Completion {
                token: 43,
                result: ResultKind::Cancelled
            },
            Completion {
                token: 44,
                result: ResultKind::Closed
            }
        ]
    );
    DraftBackend::turn(&mut backend, Timeout::Now, &mut out)?;
    assert!(out.is_empty());
    println!("draft p2 contract PASS timer_completions=1 cancel=1 closed=1 waits={waits}");
    Ok(())
}
#[cfg(not(all(target_os = "wasi", target_env = "p2")))]
fn main() {}
