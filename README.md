# Privilege Separation for Rust

[![Crates.IO](https://img.shields.io/crates/v/privsep.svg)](https://crates.io/crates/privsep)
[![docs.rs](https://docs.rs/privsep/badge.svg)](https://docs.rs/privsep)
[![Build Status](https://github.com/reyk/privsep-rs/actions/workflows/build.yml/badge.svg)](https://github.com/reyk/privsep-rs/actions/workflows/build.yml)
[![License](https://img.shields.io/badge/license-ISC-blue.svg)](https://raw.githubusercontent.com/reyk/privsep-rs/main/LICENSE)

This crate is **experimental** and **WIP**.

## Minimum Rust version

This crate uses const generics and requires Rust 1.51 or later.

## TODO

Many things, including:

- Improve documentation and rustdoc.
- `process`:
  - Allow to spawn multiple processes of a same child (not really needed with tokio).
  - Improve naming of structs.
  - Add support for OS-specific sandboxing (e.g. OpenBSD pledge)
  - Add support for running privileged operations in a child before privdrop.
  - [Help to get `ancillary` into stable](https://github.com/rust-lang/rust/pull/83374).
  - Add suppport for nightly.
- `log`:
  - Improve async logging and lazy initialization of log messages.
- Write more tests to improve code coverage.

## Copyright and license

Licensed under an OpenBSD-ISC-style license, see [LICENSE](LICENSE) for details.
