# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0-alpha.4](https://github.com/PerryTS/turnloop/compare/turnloop-tls-v0.1.0-alpha.3...turnloop-tls-v0.1.0-alpha.4) - 2026-09-15

### Other

- Hand a quiescent transport's descriptor back to the host

## [0.1.0-alpha.3](https://github.com/PerryTS/turnloop/compare/turnloop-tls-v0.1.0-alpha.2...turnloop-tls-v0.1.0-alpha.3) - 2026-09-15

### Other
- Shutdown sends close_notify and half-closes TCP while reads keep decrypting; writes after close_notify are refused
- Fixed: a rejected TLS record blocked every later read
- release v0.1.0-alpha.3
