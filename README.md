# windlass (working name)

An embeddable, cross-platform event-loop driver that the host turns: bounded `turn()`, completion-shaped I/O,
native backends for Linux (epoll), macOS/BSD (kqueue), Windows (IOCP), WASI 0.2/0.3 and the web.

Status: pre-alpha, under construction. The specification is [DESIGN.md](DESIGN.md); implementation lanes are in [LANES.md](LANES.md).
