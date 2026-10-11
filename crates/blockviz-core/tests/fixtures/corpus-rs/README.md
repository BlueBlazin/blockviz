# Rust corpus fixtures

Trimmed snapshots of real repositories of different shapes, used by
`tests/corpus_rust.rs` to check that diagrams read the way a developer would
describe each codebase. Lockfiles and binary files are left out; each keeps
its license file.

| Fixture | Source | Commit | License |
|---|---|---|---|
| mini-redis | github.com/tokio-rs/mini-redis (`src/`, `examples/`, `tests/`) | 3d93b42 | MIT |
| fd | github.com/sharkdp/fd (`src/`, `build.rs`) | 14dcd92 | MIT OR Apache-2.0 |
| ripgrep-core | github.com/BurntSushi/ripgrep (`crates/core`, workspace manifest) | 3fce3b5 | MIT OR Unlicense |
| axum-examples | github.com/tokio-rs/axum (`examples/`: todos, sqlx-postgres, jwt, chat, key-value-store, error-handling; a workspace) | e57b547 | MIT |
| llama2-rs | github.com/gaxler/llama2.rs (`llama2-rs/`) | 83a32ed | MIT |
