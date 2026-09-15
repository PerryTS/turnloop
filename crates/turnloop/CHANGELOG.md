# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0-alpha.4](https://github.com/PerryTS/turnloop/compare/turnloop-v0.1.0-alpha.3...turnloop-v0.1.0-alpha.4) - 2026-09-15

### Other

- A Windows socket duplicate inherits its IOCP association
- Hand a quiescent transport's descriptor back to the host
- Assert a floor, not an exact size, for kernel-chosen socket buffers
- Refuse an unappliable accept default at the accept, not only at listen
- Document the keep-alive partial-failure contract
- Point the socket-option docs at DESIGN 7.7 and simplify a test helper
- Report a creation-time nodelay WASI cannot apply, and cover the option surface
- Add a typed socket-option API on live handles
- Normalize WASI deadline-event accounting (I02) ([#28](https://github.com/PerryTS/turnloop/pull/28))
- Normalize WASI deadline-event accounting (I02)

## [0.1.0-alpha.3](https://github.com/PerryTS/turnloop/compare/turnloop-v0.1.0-alpha.2...turnloop-v0.1.0-alpha.3) - 2026-09-15

### Other
- Queued turns never block; zero-timeout discovery polls are counted separately from blocking waits (DESIGN §10 rule 3)
- Typed filesystem requests and filesystem watches on every backend (epoll/inotify, kqueue/FSEvents, IOCP ReadDirectoryChangesW, WASI 0.2/0.3)
- Windows: Node/libuv process, console and synchronous-handle semantics (`windows_hide`, `detached`, parent-lifetime job, permanent console handler, duplex read preemption)
- `AsyncIo::poll_shutdown` half-closes the write side without closing the handle
- Published dependencies use caret requirements instead of exact pins

- named-pipe backlog, same-port pipe routing, Windows contract equivalents
