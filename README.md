# x100

x100 is an agentic management environment: a desktop app and terminal interface that run teams of coding
agents as an organisation. You talk to one agent per project, the CEO. The CEO plans the work, forms a team of
leads, and reports back. Leads run their own workers. x100 is free and open source.

## Status

Nothing works yet. The repository holds the design and an empty Cargo workspace with one crate per area. Code
arrives milestone by milestone.

## Build and check

The toolchain is pinned in `rust-toolchain.toml` and rustup installs it on first use.

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

CI runs the same four commands on Linux and macOS.

## Documents

Read `docs/` for the design: `PRODUCT.md` (what x100 does), `ARCHITECTURE.md` (how it is built) and
`V1_PLAN.md` (the milestones).

## Licence

MIT or Apache-2.0, at your option. See `LICENSE-MIT` and `LICENSE-APACHE`.
