# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0-alpha.3](https://github.com/PerryTS/turnloop/compare/turnloop-io-v0.1.0-alpha.2...turnloop-io-v0.1.0-alpha.3) - 2026-09-15

### Other
- `HalfClose` and `shutdown()` for `AsyncIo`, TLS streams and transports
- `linger_close(stream, scratch, deadline)`: half-close, discard until peer EOF or the deadline, then close
- Published dependencies use caret requirements instead of exact pins
- release v0.1.0-alpha.3
