# blockviz

See inside a codebase the way architecture figures do: as nested block diagrams you can zoom into without end. The repository is a diagram of programs and the files they exchange; zoom into a file and it becomes a diagram of its classes and functions; zoom into a function and you see its dataflow; zoom into a call and you are inside the code it calls.

Everything runs locally in the browser. A Rust core compiled to WebAssembly parses and lays out the whole project in a worker; the UI thread only draws.

```
npm install
npm run build        # builds the wasm core, then the web app
npm run dev          # http://127.0.0.1:5173 (run `npm run wasm` once first)
npm test             # vitest + cargo test
```

Open a GitHub repository (`#github=owner/repo` in the URL works too), a local folder (picker or drag-and-drop), or paste a single file. The bundled example is [karpathy/nanoGPT](https://github.com/karpathy/nanoGPT) (MIT), which also serves as the main test fixture; `tests/corpus.rs` checks the abstractions on trimmed snapshots of a FastAPI app, httpx, nanochat, Flask, a Django site and HTTPie, and `cargo test --test audit -- --ignored --nocapture` reports the blocks most likely to read badly (code-like titles, guard tags, empty branch arms).

A command-line view of the same analysis helps when tuning abstractions:

```
cargo run --release --bin blockviz -- path/to/repo --depth 4
```

## What you see

Every level is a dataflow diagram read top to bottom: inputs enter at the top, results leave at the bottom, arrows show what flows into what.

| Level | Blocks | Arrows |
|---|---|---|
| Repository | folders, programs, files on disk | imports, `exec`, files written by one program and read by another |
| File | classes, functions, sections of a script, settings tables | which definition uses which; values between sections |
| Class | its sub-components (`h: Block × n_layer`) and methods | method calls |
| Function | calls into project code, sub-module calls, library operations, I/O | the values passed, labelled with variable names |

In nanoGPT this gives `prepare.py → train.bin → train.py → ckpt.pt → sample.py` at the top, `LayerNorm, CausalSelfAttention, MLP → Block → GPT` inside `model.py`, and `x → ln_1 → attn → ⊕ → ln_2 → mlp → ⊕` inside `Block.forward`, the same structure as the Transformer figure, with each block zoomable into its own diagram.

### Abstraction rules

The goal is blocks a reader would name, not a syntax tree.

* A block is created for calls into project code, calls on sub-modules (`self.attn(x)`), library operations and I/O. Glue (reshapes, builtins, arithmetic on sizes) is folded into the block it feeds, or into one block per statement.
* A block's title is a name, never cut-off code: the operation for calls (`view · transpose`), the developer's own name for a value it produces (`token`, `allow_env_proxies`), the field a store sets (`Authorization`, captioned `set in request.headers`). The caption says how, as the formula with long parts elided whole (`settings.SENTRY_DSN and …`). Code appears in a title only when it fits whole (`T + 1`).
* `a + b` of two computed values becomes a ⊕ (residual connections, positional encodings).
* `print`, `assert`, imports, docstrings and `super().__init__()` never become blocks; side effects show as a marker and in the inspector.
* `if __name__ == "__main__"` is an entry-point marker plus a "Main" section, never a box containing `__name__`.
* Small loops and branches are drawn inline, like the "N×" box of an architecture figure, with repeat counts inferred from the code (`× n_layer` from `ModuleList([... for _ in range(config.n_layer)])`). A lone guarded statement gets an `if …` tag instead of a box.
* Every way out is visible. A guarded exit is a step tagged with its condition (`return None · if value is None`, `continue · if not line`); every returned value reaches the function's result; `if c: …; return x` followed by more code is an if/else. No branch arm is drawn empty: one that only sets or returns a value shows that step.
* Arrows say what a value is to the step. Solid: the data it works on. Faint: settings, and properties of values (`len(x)`, `x.shape`, `x.device`, type checks), but not conversions like `int(x)`. Dotted: a condition that decides whether the step runs.
* Long scripts are split into sections at comment-headed paragraphs (`# model init`, `# training loop`); runs of constants become a settings table.
* Light types name things the way a reader would: `self.transformer.wte(idx)` is an `Embedding`, `block` is a `Block`, `model.configure_optimizers` is `GPT.configure_optimizers`.
* Dependency diagrams are transitively reduced so their layering shows; a file's full import list is in the inspector.
* Framework code says what it is: `@router.get("/{id}")` on a router with `prefix="/items"` is the endpoint `GET /items/{id}`; click and Typer commands, Celery tasks and request hooks are labelled the same way, and Django `urlpatterns` become a route table.
* Files get roles a reader would use: app (creates `FastAPI()`, `Flask()`, a WSGI app), routes, script, tests, config (Django settings, Sphinx `conf.py`), module. Entry points come from main guards, `__main__.py`, app objects, and anything a script, Dockerfile, CI workflow, Makefile, compose file or `pyproject.toml`/`setup.cfg` console script runs.
* A shell script that runs several programs is a pipeline: zoom into `speedrun.sh` to see its steps in order, and into a step to see the program it runs.
* Library modules show their definitions. `__all__` lists, type aliases, loggers and optional or type-checking imports are not blocks; constants gather into one table. Runs of statements on one subject are one block or table: calls (`session` · add → commit → refresh, which opens to each call in order), `parser.add_argument` × 12, fields set one after another (`smtp_options` · set × 3) and repeated `text += …`.
* Generated code (migrations, protobuf stubs, files marked "auto-generated") is a single block, and empty `__init__.py` files are not drawn.
* A Docker Compose file is the system diagram: its services, what each depends on, what the proxy routes to, and, for services built from the repository, the code they are built from (zoom in).
* Database access (sessions, cursors, ORM managers) is marked as I/O, ORM models are tagged as tables, and views link to the templates they render.
* Meanings guessed from names apply only where the code shows the domain: architecture-figure colours (attention, norm, linear…) in modules that import an ML framework, database access in modules that use a database library (directly or through a project module). Names match by whole words from the head noun back, so `attn_dropout` is a dropout and `headers` is not a projection head.

### Other languages

Every text file is handled. Notebooks are analyzed as Python (cells become sections titled by the markdown above them). Other source files get an outline: their definitions, calls between definitions in brace languages, and imports resolved to files for JavaScript/TypeScript, Rust and C/C++. Config files become key/value tables (`.env` values are masked), docs become heading lists, and READMEs describe their folder. Binary files appear as assets.

## Architecture

```
crates/blockviz-core/          Rust → WebAssembly
  src/ir.rs                    the scene: one tree of nested, laid-out diagrams
  src/naming.rs                block titles from a language-neutral description of each step
  src/vocabulary.rs            what names and libraries mean: domains, NN categories, metadata,
                               database calls
  src/python/                  source + comments, symbol index and type inference, flow builder, effects
  src/python/names.rs          Python expressions described for naming; structural shortening
  src/python/frameworks.rs     routes, commands and tasks from framework decorators
  src/invocations.rs           commands in scripts and manifests that run project code
  src/compose.rs               Docker Compose services
  src/outline.rs               non-Python outlines and imports
  src/highlight/               syntax classes for the source view: Ruff's tokens and tree for
                               Python, table-driven lexers for other languages and formats
  src/project.rs               repository tree, cross-file relations, artifacts, edge lifting
  src/layout/                  bottom-up sizing; layered layout with shared trunks, PAV placement,
                               orthogonal routing with track allocation
src/
  core/                        worker, analyzer, loaders (GitHub, folder, drop, paste)
  canvas/                      camera (anchored for unlimited depth), renderer, engine
  ui/                          inspector (resizable, highlighted source), search, open dialog
```

**One uniform scene model.** Every node lives in its parent's interior coordinates. A node with an interior (children, a table, or a `ref` to another node's interior) draws it inside its `inner` rectangle, scaled to fit. That single rule is what makes zoom unbounded: the renderer never needs to know what a node is to descend into it. A call borrows its callee's interior by reference, so the same definition appears wherever it is called without being duplicated.

**Layout** happens once, in Rust, bottom-up: an interior is laid out before the box that contains it is sized. Edges carrying the same value share a trunk so one line visibly splits into its consumers, and node positions are solved per layer as weighted isotonic regression, which keeps long edges straight.

**Rendering** is immediate-mode Canvas 2D, driven only by invalidation. Each frame walks what is visible; boxes open smoothly as their interior becomes large enough to show something; text fades in by on-screen size. The camera is anchored to the deepest container that fills the viewport and every transform is derived from it, so precision does not degrade however deep you zoom.

## Python versions

Parsing uses [Ruff's parser](https://github.com/astral-sh/ruff) (`ruff_python_parser`), which follows the current grammar: PEP 701 f-strings and PEP 695 type parameters (3.12), type parameter defaults (3.13), t-strings and unparenthesized `except A, B:` (3.14), and `lazy import`, unpacking in comprehensions and `case +0:` (3.15). `tests/python_syntax.rs` covers each release from 3.8 to 3.15. The whole CPython 3.15.0rc3 `Lib/` (2,052 files, test suite included) parses with no errors apart from the one file CPython keeps deliberately invalid. The parser recovers from syntax errors, so a file with a typo still renders everything around the error.

## Limits

This is static analysis: arrows show likely dataflow and dependencies, not a runtime trace. Dynamic dispatch, code passed through strings and monkey-patching are invisible. Very large repositories work but get heavy in the browser (the full CPython standard library, about 330k lines, produces a 64 MB scene).
