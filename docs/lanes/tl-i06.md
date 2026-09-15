# tl-i06 — filesystem operations and watches

Status: implementation in progress. Base a0fbb1b; macOS arm64. Git metadata is
read-only; the integrator commits this working tree.

## Scope and decisions

Perry consumer inspection covers runtime fs/fd_ops.rs, fd_sync_ops.rs,
filehandle.rs, mod.rs and dir_glob_watch/{watch,watch_backend,watch_fsevents}.rs.
Promises/callback dispatch, Buffer rooting, Dirent/Stats conversion, recursive
copy/remove/glob and watchFile scheduling belong to Perry. The driver needs typed
open, cursor/positional bytes, metadata, directory enumeration, namespace changes,
sync/truncate and native watch change/overflow notifications.

D8 clarification proposed: typed regular files share the bounded process pool on
all native platforms. Windows dedicated synchronous workers remain restricted to
adopted streams/stdio and consoles, where interruptible reads require them.
No dependency or soak-policy change is planned. Web filesystem remains Unsupported.

## Verification ledger

Inspection commands (rg, cat, sed, git status): PASS; initial tree clean.
Implementation checks: UNRUN, pending implementation.
Linux/Windows runtime: UNRUN (no host). WASI fixtures: UNRUN, pending implementation.

## Open questions and next steps

Implement typed reusable storage, native execution and watch providers; add byte,
metadata/error, ordering/cancellation/backpressure and allocation contracts; add
capability-scoped WASI paths. Run native checks and strict cross-target Clippy.
Record every verification invocation and its result here.
