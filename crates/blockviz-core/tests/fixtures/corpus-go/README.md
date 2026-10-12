# Go corpus fixtures

Trimmed snapshots of real repositories of different shapes, used by
`tests/corpus_go.rs` to check that diagrams read the way a developer would
describe each codebase. Lockfiles (`go.sum`), images and tool configuration
are left out; each keeps its license file.

| Fixture | Source | Commit | License |
|---|---|---|---|
| groupcache | github.com/golang/groupcache (all code) | 2c02b82 | Apache-2.0 |
| realworld | github.com/gothinkster/golang-gin-realworld-example-app (code, without the `unit_test.go` files) | 626c372 | MIT |
| redcon | github.com/tidwall/redcon (library and examples, without tests) | 7e866b4 | MIT |
| chi | github.com/go-chi/chi (`chi.go`, `chain.go`, `context.go`, `mux.go`, `tree.go`, eight middlewares, `_examples/rest`) | 167e1e3 | MIT |
| glow | github.com/charmbracelet/glow (code) | 8b0600f | MIT |
| example | github.com/golang/example (`hello`, `outyet`) | 7f05d21 | BSD-3-Clause |
