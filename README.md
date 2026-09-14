# turnloop

An embeddable event-loop driver for Rust. Your host owns the thread and calls
`turn()` to advance I/O, timers and cross-thread posts. Each turn returns
completions, makes at most one OS wait, and runs no user callbacks. There is no
runtime thread and no tokio dependency.

**Pre-alpha.** This workspace contains the Unix driver and runtime-independent
PostgreSQL, MySQL, Redis, MongoDB and SMTP protocol engines. APIs can change.
Protocol engines consume bytes and produce actions; host transport and executor
adapters are still being integrated. Perry is the first intended consumer.

| Platform | Current implementation | Validation |
|---|---|---|
| macOS arm64 | kqueue driver, sockets, timers, posts and pool | Native contracts and allocation tests |
| Linux x86_64 / arm64 | epoll, nanosecond waits, timerfd fallback | Cross-checked locally; native CI required |
| FreeBSD / Apple mobile / Android | Unix backend paths | Best effort; runtime validation pending |
| Windows x86_64 | Standalone IOCP spike | Cross-checked; production adapter and native contracts pending |
| WASI 0.2 / 0.3 | Standalone polling / component async spikes | Production adapters and shared contracts pending |
| Web | Standalone host-callback spike | Production adapter and browser contracts pending |

Windows and WASM are required for the first release; standalone spikes are
excluded from the publishable workspace. The platform table describes current
code, not a completed support promise. HTTP, TLS and WebSocket crates are being
worked on separately.

```rust,no_run
use std::time::Duration;
use turnloop::{Completions, Config, Loop, Timeout, Token};

let mut driver = Loop::new(Config::default())?;
let deadline = driver.now() + Duration::from_millis(2);
driver.timer(deadline, None, Token(42))?;
let mut completions = Completions::default();
driver.turn(Timeout::Until(deadline), &mut completions)?;
// Dispatch completions in the host's own event-loop phase.
# Ok::<(), turnloop::Error>(())
```

Use the pinned nightly for dependency resolution under the seven-day publication
soak. The workspace also builds with stable Rust 1.97.1.

- [Design and host boundary](DESIGN.md)
- [Contributing, checks and private test servers](CONTRIBUTING.md)
- [Release and first-publication procedure](RELEASING.md)
- [Integration status and verification](docs/INTEGRATION_REPORT.md)
- [MIT license](LICENSE)
