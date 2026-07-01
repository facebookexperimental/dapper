---
title: Installation
sidebar_label: Installation
---

# Installation

Build Dapper from the open source Rust workspace with Cargo.

## Prerequisites

- A Rust toolchain with Cargo.
- A checkout of the Dapper repository.

## Build

From the repository root:

```bash
cargo build --release -p dapper_cli --bin dapper
```

The binary is written under Cargo's release target directory:

```bash
./target/release/dapper help
```

Add `target/release` to your `PATH` or copy the binary into a directory already on your `PATH` if you want to run `dapper` directly.

## Verify

Confirm the CLI can render its built-in help:

```bash
dapper help
```

Then continue with [Getting Started](./getting-started.md).
