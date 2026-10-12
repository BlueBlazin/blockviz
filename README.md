# blockviz

See inside a codebase the way architecture figures do: as nested block diagrams you can zoom into without end. The repository is a diagram of programs and the files they exchange; zoom into a file and it becomes a diagram of its classes and functions; zoom into a function and you see its dataflow; zoom into a call and you are inside the code it calls.

Everything runs locally in the browser. A Rust core compiled to WebAssembly parses and lays out the whole project in a worker; the UI thread only draws.

[FEATURES.md](FEATURES.md) lists everything the app does.

```
npm install
npm run build        # builds the wasm core, then the web app
npm run dev          # http://127.0.0.1:5173 (run `npm run wasm` once first)
npm test             # vitest + cargo test
```

Open a GitHub repository (`#github=owner/repo` in the URL works too), a local folder (picker or drag-and-drop), or paste a single file. The bundled example is [karpathy/nanoGPT](https://github.com/karpathy/nanoGPT) (MIT), which also serves as the main test fixture. `tests/corpus.rs` checks the abstractions on trimmed snapshots of a FastAPI app, httpx, nanochat, Flask, a Django site and HTTPie; `tests/corpus_js.rs` does the same for an Express API, a Next.js app, a NestJS service, a React/Vite app, ky, serve and a Turborepo workspace; `tests/corpus_rust.rs` for mini-redis, fd, ripgrep's core crate, a workspace of axum examples and llama2.rs; `tests/corpus_go.rs` for groupcache, a gin RealWorld API, redcon, chi, glow and golang/example. `cargo test --test audit -- --ignored --nocapture` reports the blocks most likely to read badly (code-like titles, guard tags, empty branch arms) across all four corpora.

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

### Finding your way

A reader sees one small part at a time, so every part says where it fits.

* Opening a block (double-click, double-tap, Enter, search) zooms until its diagram is readable; a block with nothing inside is shown among its neighbours, not alone.
* The details panel shows a step's code inside the function around it, its lines marked. Clicking a line selects the block drawn for it; pointing at a block or a line points out the other.
* *Used by* lists every place a definition is used (callers of a function, constructions and components of a class, calls of a component), each a link, and outlines them in the diagram while it is in focus.
* Clicking an arrow follows its value through the function: every edge carrying that name, through the steps that re-bind it and into its branches and loops, with where it comes from and what reads it listed.
* Every jump is a step in the browser's history (Back, Alt+←), and the URL names the current view and selection (`#at=model.py/CausalSelfAttention/forward`), so a link reopens it.
* Inside code reached through a call (`… › Block › forward › attn`), a line under the breadcrumbs says where that code is defined, and long breadcrumb paths fold their middle into a menu.

### Reading a pull request

Open a pull request (`github.com/owner/repo/pull/123`) or a comparison (`…/compare/main...feature`) and the diagram is the code after the change, drawn the same way, coloured by what the change did: added blocks green, changed ones amber, what it did not touch faded, with new connections in green and removed ones as dashed red curves. Every block counts the changes inside it, so from the whole project down you see which parts of the architecture the change touches; the details panel lists the changes in diagram order (`N`/`P` step through them) and shows a block's code as it changed.

Both versions are analyzed (the head and the commit it branched off its base) and matched block by block: files by path through renames, folders by name or contents, blocks by kind and title, code moved within a file followed to its new place. Only files the change touched have changes, and a dependency still implied through others is neither new nor removed.

### Abstraction rules

The goal is blocks a reader would name, not a syntax tree.

* A block is created for calls into project code, calls on sub-modules (`self.attn(x)`), library operations and I/O. Glue (reshapes, builtins, arithmetic on sizes) is folded into the block it feeds, or into one block per statement.
* A block's title is a name, never cut-off code: the operation for calls (`view · transpose`), the developer's own name for a value it produces (`token`, `allow_env_proxies`), the field a store sets (`Authorization`, captioned `set in request.headers`). The caption says how, as the formula with long parts elided whole (`settings.SENTRY_DSN and …`). Code appears in a title only when it fits whole (`T + 1`).
* Values combined by an arithmetic operator are the operator, drawn as a circle (⊕, −, ×, ÷, @, //, %, ^) in every language: residual connections, positional encodings, `loss / steps`. Each operand must be a single value from a different place; `a + b + c` is one ⊕ of three. A constant operand (`T + 1`), a longer formula or a store into a field stays a formula step.
* `print`, `assert`, imports, docstrings and `super().__init__()` never become blocks; side effects show as a marker and in the inspector.
* `if __name__ == "__main__"` is an entry-point marker plus a "Main" section, never a box containing `__name__`.
* Small loops and branches are drawn inline, like the "N×" box of an architecture figure, with repeat counts inferred from the code (`× n_layer` from `ModuleList([... for _ in range(config.n_layer)])`). A lone guarded statement gets an `if …` tag instead of a box.
* Every way out is visible. A guarded exit is a step tagged with its condition (`return None · if value is None`, `continue · if not line`); every returned value reaches the function's result; `if c: …; return x` followed by more code is an if/else. No branch arm is drawn empty: one that only sets or returns a value shows that step.
* Every value is a solid arrow labelled with what it carries, whatever the value is: no arrow is hidden or drawn lighter because of its name. Dotted: a condition that decides whether the step runs.
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

### JavaScript and TypeScript

The same rules apply, read through JavaScript's idioms.

* Functions are functions in every spelling: `function f()`, `const f = () => …`, `exports.f = function …`, class methods, getters and setters (tagged).
* Objects that own functions are drawn like classes, and `this` inside their functions is the object: a constructor function with prototype methods (`function View() {}`, `View.prototype.render = …`), an object with functions attached (`var app = {}`, `app.listen = function …`, or `Object.create(proto)`), and an object literal of methods. Calling into a module that exports such an object (`require("./app").listen()`) reaches its function. A wrapped function (`catchAsync(async (req, res) => …)`, `memo(Card)`) is the function it wraps, tagged with the wrapper. A function that returns a function draws the returned one inside it.
* A function handed to a call is drawn inside that call: `router.get("/:id", …)` is the endpoint `GET /v1/users/:id` (mount prefixes from `app.use("/v1/users", router)` included), `addEventListener("click", …)` is `click`. Short arrows registered under a name count too.
* Promise chains pass values on the way `await` does: `fetch(url).then(r => r.json()).then(onData)` is `fetch → onData`, with no block for the chain.
* Blocks inside callbacks say when they run: `each` for `map`, `forEach` and `filter`; `on click` for an `onClick` handler, even inside a list.
* React: a capitalised function that returns JSX is a component and `useX` is a hook; elements are named by tag (`<Dialog.Root>`) and library elements given only literal props are decoration; `"use client"` modules are tagged. Next.js files get their roles (page, layout, route handler), NestJS controllers their `@Controller` prefixes, and Express apps their route tables.
* Schemas (zod, yup, joi, valibot, TypeBox, Mongoose) and DTO classes are tables of fields; interfaces, enums and type aliases are data; a namespace groups its functions; overload signatures are one function.
* Imports resolve the way bundlers and `tsc` do: `tsconfig`/`jsconfig` paths, `baseUrl` and `extends`, the `@/` alias, workspace packages by name, `.js` specifiers that name `.ts` files, `index` files, `require()` and `import()`. Node builtins are not listed as packages, and stylesheet imports link to their files.
* Database access is found by where an object comes from: a model defined next to Mongoose, a query through a service that extends `PrismaClient`.
* An object a module creates once (`const users = new UsersService()`) is known in every function of that module, including those written above it.
* Python's words are not used for JavaScript: `switch`, `catch`, `this`, objects and arrays.

### Rust

The same rules again, read through Rust's idioms.

* A struct or enum is drawn with the methods of its `impl` blocks; an `impl` in another module is drawn where it is written and leads to its type, and its methods are found from calls anywhere. The traits a type implements are its subtitle, and each trait method says which trait it belongs to (`fmt — Display`, `from — From<io::Error>`). A trait lists its required methods and draws its provided ones.
* Fields typed by project or library types are components (`blocks: Block × n_layer`, `listener: TcpListener`); wrappers are looked through (`Arc<Mutex<Db>>` is a `Db`), and standard types and synchronisation primitives are state, not components. `self.ln_1.forward(x)` calls the component, as `self.ln_1(x)` does in PyTorch, so a candle or burn model reads like nanoGPT.
* Values computed by `if`, `match` and blocks go where they are sent: `let x = match …` is a match whose arms set `x`, and a function's last expression is its result. `Ok(x)` and `Some(x)` are `x`; an `Err` handed back is an exit tagged `returns error`; `?` passes values on. `let … else` is a guard (`return · unless Some(path)`).
* Match arms are their patterns, with their guards (`Mode::Search(mode) if threads == 1`); names a pattern binds come from the matched value, and a value unpacked from an enum variant has the variant's type, so `match self { Get(cmd) => cmd.apply(…) }` reaches `Get::apply`. `_ => {}` arms are not drawn.
* Macros are what they stand for: `format!` and `println!` are formatted text (prints), `vec![x; n]` is `[x] * n`, `assert!` an assertion, `panic!`, `bail!` and `unreachable!` exits; logging macros are effects; a macro whose body is not Rust (`json!`) reads the variables it names.
* Glue that only borrows, clones, converts or unwraps (`&x`, `.clone()`, `.unwrap()`, `.into()`, `.iter()`, `as`) is left out, and `xs.iter().map(|x| f(x)).collect()` is a comprehension (`each of xs`).
* Modules resolve the way Cargo and rustc do: crate roots from `Cargo.toml` (libraries, binaries, `src/bin`, examples, tests, benches, build scripts, workspaces), `mod` declarations with `#[path]`, and `crate::`, `super::`, `self::` and other crates of the workspace. A module depends on what its code uses: a crate root that only re-exports (`pub use db::Db`) is not drawn between its modules and their users.
* Routes come from router builders by shape: axum's `.route("/users", get(list).post(create))` with `.nest` and `.merge`, actix-web's `web::resource`, `web::scope` and `#[get("/…")]` (and Rocket's attributes). Handlers are named by their routes and routers are route tables. Derives say what a type is for (`command line` for clap, `DB row`, `error type`), `sqlx`, diesel and sea-orm queries are database steps, and `#[cfg(unix)]`, `#[cfg(feature = "x")]` variants are tagged with their condition.
* Files are binaries (entry points), libraries, examples, tests, benchmarks or build scripts by their crate.

### Go

The same rules again, read through Go's idioms.

* A package is the files of a directory: they use each other's names without importing them, and a file depends on the file that defines what it uses (`store.Open` is a dependency on `store/db.go`, not on the package). Imports resolve through `go.mod` modules (nested modules, local `replace` directives, `go.work`), and by directory for code without a module.
* A struct is drawn with the methods declared for it in the same file; methods declared in another file are drawn there as the type's `methods` and lead to it. Embedded types are bases, so their methods are found. Interfaces are tables of method signatures, and a type lists the project's interfaces it satisfies (`Circle — Shape`), matched by method signature since Go never says so.
* A call through an interface value goes to what the program puts behind it: the types it passes, stores, returns or asserts where the interface is expected (`return &pgStore{db}` from a function returning `Store`). One such implementation and the call is that method (`Get — pgStore`); several and the call names them and leads to the interface (`Lookup — lru or redisCache`). A type that only happens to have the methods is not taken for it (a library's `Validator` is whatever its user registers), test doubles do not count outside tests, and methods that only return a field (`Width()`) read as the value.
* Errors are passed on, not drawn: `if err != nil { return …, err }` (wrapped or not, with cleanup before it, or `panic`, `log.Fatal`, `os.Exit` after printing) leaves only the call, as Rust's `?` does. Checks that do something else (retry, fall back, log and continue) stay branches. An error made here (`return nil, ErrClosed`, `errors.New("…")`, `fmt.Errorf("…: %w", err)`) is an exit tagged `returns error`, named by its message.
* `go f()` and `defer f()` are steps marked `in background` and `on return`; deferred cleanup (`defer f.Close()`, `defer mu.Unlock()`, a deferred function that only closes) and lock and WaitGroup bookkeeping are left out. Receiving (`<-ch`) waits for a value; a send is a `send` step. `select` is a choice among channel operations, a type switch a match whose cases type the value, and a switch without a tag an `if`/`else if` chain (`fallthrough` joins cases).
* Results: `db, err := Open(…)` types `db` from `Open`'s declared result, so `db.Save(…)` reaches the method. `xs = append(xs, x)` adds to a list, composite literals are constructor calls, lists and maps, conversions and `&`/`*` are glue, and `fmt.Sprintf` builds formatted text.
* Routes come from router calls: gin and echo (`r.GET`), chi and fiber (`r.Get`, `r.Route`, `r.With(…)`, `r.Mount`), gorilla and net/http (`HandleFunc`, Go 1.22 patterns like `"GET /items/{id}"`), with group prefixes followed across functions and packages (`articles.Register(v1.Group("/articles"))`). Handlers are named by their routes and registrations are route tables. cobra and urfave/cli commands are named by their `Use`; `database/sql`, sqlx, gorm, pgx, Mongo, Redis and bbolt calls are database steps (a gorm chain is named by the call that runs it); struct tags say what a type is for (`serializable`, `DB row`).
* Files are binaries (`package main` with `func main`), tests (`TestX`, `BenchmarkX`, `FuzzX`, `ExampleX` are tagged; `if got != want { t.Errorf(…) }` is an assertion), or modules; build constraints (`//go:build`, `// +build`, `_linux.go`) tag the file, generated files (`// Code generated … DO NOT EDIT.`) are shown but not opened, and `testdata` and `vendor` are not the project's code.

### Other languages

Every text file is handled. Notebooks are analyzed as Python (cells become sections titled by the markdown above them). Other source files get an outline: their definitions, calls between definitions in brace languages, and imports resolved to files for C/C++. Config files become key/value tables (`.env` values are masked), docs become heading lists, and READMEs describe their folder. Binary files appear as assets.

## Architecture

```
crates/blockviz-core/          Rust → WebAssembly
  src/ir.rs                    the scene: one tree of nested, laid-out diagrams
  src/naming.rs                block titles from a language-neutral description of each step
  src/vocabulary.rs            what names and libraries mean: domains, NN categories, metadata,
                               database calls
  src/code/syntax.rs           the language-neutral syntax tree every front end lowers into
  src/code/                    analysis over it: source + comments, symbol index and type
                               inference, flow builder, effects
  src/code/names.rs            expressions described for naming; structural shortening
  src/code/frameworks.rs       routes, commands, tasks, components from decorators and calls
  src/python/                  Ruff's tree → syntax tree
  src/js/                      oxc's tree → syntax tree; import resolution (tsconfig, workspaces)
  src/rust/                    syn's tree → syntax tree; module resolution (Cargo manifests,
                               `mod` declarations); a small lexer for comments
  src/go/                      a Go parser (a port of go/scanner and go/parser, checked against
                               go/parser), its tree → syntax tree; packages by go.mod
  src/invocations.rs           commands in scripts and manifests that run project code
  src/compose.rs               Docker Compose services
  src/outline.rs               outlines and imports for other languages
  src/highlight/               syntax classes for the source view: Ruff's tokens and tree for
                               Python, table-driven lexers for other languages and formats
  src/project.rs               repository tree, cross-file relations, artifacts, edge lifting
  src/layout/                  bottom-up sizing; layered layout with shared trunks, PAV placement,
                               orthogonal routing with track allocation
src/
  core/                        worker, analyzer, loaders (GitHub repositories, pull requests and
                               comparisons, folder, drop, paste), saved downloads
  canvas/                      camera (anchored for unlimited depth), renderer, engine, locations
                               (paths as titles), value tracing, scene diff (two versions matched
                               block by block)
  ui/                          inspector (resizable, source linked to blocks, used by), value panel,
                               changes panel and line diff, history and links, breadcrumbs,
                               search, open dialog
```

**One uniform scene model.** Every node lives in its parent's interior coordinates. A node with an interior (children, a table, or a `ref` to another node's interior) draws it inside its `inner` rectangle, scaled to fit. That single rule is what makes zoom unbounded: the renderer never needs to know what a node is to descend into it. A call borrows its callee's interior by reference, so the same definition appears wherever it is called without being duplicated.

**Layout** happens once, in Rust, bottom-up: an interior is laid out before the box that contains it is sized. Edges carrying the same value share a trunk so one line visibly splits into its consumers, labelled under the block it leaves; with "Tidy arrows" (the switch beside the home button, or `--trunks` on the command line) all values a block sends share one trunk that branches to each reader, every branch labelled just above the block it reaches. Node positions are solved per layer as weighted isotonic regression, which keeps long edges straight.

**Rendering** is immediate-mode Canvas 2D, driven only by invalidation. Each frame walks what is visible; boxes open smoothly as their interior becomes large enough to show something; text fades in by on-screen size. The camera is anchored to the deepest container that fills the viewport and every transform is derived from it, so precision does not degrade however deep you zoom.

## Language versions

Python parsing uses [Ruff's parser](https://github.com/astral-sh/ruff) (`ruff_python_parser`), which follows the current grammar: PEP 701 f-strings and PEP 695 type parameters (3.12), type parameter defaults (3.13), t-strings and unparenthesized `except A, B:` (3.14), and `lazy import`, unpacking in comprehensions and `case +0:` (3.15). `tests/python_syntax.rs` covers each release from 3.8 to 3.15. The whole CPython 3.15.0rc3 `Lib/` (2,052 files, test suite included) parses with no errors apart from the one file CPython keeps deliberately invalid. The parser recovers from syntax errors, so a file with a typo still renders everything around the error.

JavaScript and TypeScript are parsed by [oxc](https://github.com/oxc-project/oxc) (`oxc_parser`): ES2025 (import attributes, `using` declarations, regexp `v` flag, top-level await), current TypeScript (decorators, `accessor`, `satisfies`, `const` type parameters, `asserts` guards) and JSX in `.jsx`/`.tsx`. `tests/js_syntax.rs` covers each, plus CommonJS and UMD modules. Where oxc gives up at a syntax error, the statement holding it is blanked out and the file parsed again, so the rest still renders. vuejs/core (about 160k lines of TypeScript) analyzes in under a second natively.

Rust is parsed by [syn](https://github.com/dtolnay/syn) with span locations: editions 2015 to 2024, including `let … else`, `if let` chains, async closures, `use<…>` capture lists, `&raw` references, inline `const` blocks, C string literals and `unsafe extern` blocks. `tests/rust_syntax.rs` covers each on a small crate with every kind of target. syn does not recover from syntax errors, so on an error the body of the function holding it (or the top-level item) is blanked out and the file parsed again. ripgrep (about 56k lines) analyzes in under half a second natively.

Go is parsed by a hand-written port of Go's own scanner and parser (`src/go/`), up to Go 1.24: generics with type sets and generic type aliases, range over integers and functions, and every older form. Its tree mirrors go/ast node for node, positions included, and `tests/go_parser.rs` compares it with go/parser's: committed golden dumps in CI, and, with Go installed, every file of the Go distribution (`cargo test --release --test go_parser go_distribution -- --ignored`), where all 7,804 files go/parser accepts give identical trees. Like go/parser it recovers from syntax errors (a broken statement or declaration is skipped and parsing resumes), never panics on damaged input and caps nesting so deep input cannot overflow the stack. The standard library (2M lines) parses in about 0.65 s natively; `tests/go_syntax.rs` covers the syntax of each release on a small module. prometheus (about 390k lines, with its vendored JavaScript) analyzes in about 3.4 s natively.

## Limits

This is static analysis: arrows show likely dataflow and dependencies, not a runtime trace. A call through a contract (an abstract method, a TypeScript interface, a Rust `dyn Trait`, a Go interface) goes to the implementation when the project has just one, and names the candidates when it has several; a method overridden without being abstract is still the base class's. Code passed through strings and monkey-patching are invisible. TypeScript types are read only as far as naming a class; longer types are shown elided by their brackets (`{ [K in keyof T as `…`]: () => T[K] }`). A generic parameter is the trait or interface it is bounded by (`M: Matcher`, `[S Store]`, `<R extends Repo>`), but generic types are not instantiated (a field typed by a parameter of another type stays unknown), and macros are read by their name and arguments, not expanded. Go calls through interface values are resolved only as far as the program shows which types go in (not through values a library hands back), and what a library function returns is known only for a few (`gin.Default()`), so a method called on such a value is a plain step. A branch with more than 24 arms (an option parser's `switch arg`) is drawn as a table of its arms. Very large repositories work but get heavy in the browser (the full CPython standard library, about 330k lines, produces a 64 MB scene).
