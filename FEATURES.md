# blockviz features

Everything the app does today, grouped by what you'd use it for. The README explains the rules behind the diagrams in more detail.

## Opening code

- **GitHub repository** by URL: `owner/repo`, a branch or subfolder (`…/tree/main/src`), or a link with `#github=owner/repo`. The last URL is remembered.
- **Saved downloads**: the last few repositories opened from GitHub are kept in the browser. A reload, a link or Back opens them without downloading anything; after a day, or when a repository is opened from the Open dialog, GitHub is asked for the latest commit and only changed files are downloaded. If GitHub can't be reached or its rate limit is spent, the saved copy opens.
- **Saved analyses**: the last few diagrams are kept with the code they were made from, so reopening a project, switching "Tidy arrows" back, or reloading shows it in well under a second instead of analyzing again. A diagram is reused only for exactly the same files, settings and version of the analyzer.
- **Cancel** any download or analysis from the progress message; the project on screen stays. A download that fails is tried again; a file that still can't be fetched is listed without its text.
- **Bundled example**: karpathy/nanoGPT.
- **Local folder**, from the picker or by dropping it anywhere on the page.
- **Pasted code**: a single file, with its language guessed from the code (Python, JavaScript, TypeScript, JSX/TSX, Rust, Go).
- **Runs in the browser**: a Rust core compiled to WebAssembly analyzes the code in a background worker. Nothing is uploaded or run.
- **Skips what isn't the project's code**: `.git`, `node_modules`, `target`, `dist`, `build`, virtualenvs, caches and binary files. Limits are 4,000 files, 2 MB per file and 100 MB of text in all; a larger project is read code first, then tests, then documentation, and the footer says what was left out.
- **Command line**: `blockviz <dir> [--depth N] [--json] [--trunks]` prints the same analysis without a browser.

## Pull requests and comparisons

- **Open a change**: a pull request (`github.com/owner/repo/pull/123`, `…/pull/123/files`, `owner/repo#123`) or two branches, tags or commits compared (`…/compare/main...feature`, or one branch against the default branch), in the Open dialog or as an address (`#github=github.com/owner/repo/pull/123`, with the view and selection).
- **Both versions are analyzed**: the head and the commit it branched off its base, as GitHub's "Files changed" compares them. Files the two share, or that a saved copy of the repository has, are downloaded once; a pull request costs four GitHub API calls. Changes are saved like repositories.
- **What it touches, at any zoom**:
  - added blocks are green, changed ones amber; a block with changes inside shows how many on a badge, and a folder holding changes is ringed
  - what the change did not touch is faded
  - a call into changed code has a dashed ring and an arrowed count, even inside faded code, so what the change affects shows
  - new connections are green arrows, removed ones dashed red curves; a dependency still implied through others is neither
  - a new input or result is named in green
- **Walking the changes**: with nothing selected, the details panel lists them by file in the order the diagram reads, under the title, branches, state, counts and a link to GitHub; files changed without changing a block are listed after. `N` and `P` (or the arrows in the panel) step through them. The chip beside the project name (`#123 +3 ~5 −1`) brings the list back.
- **A block's change**: the panel says whether it was added or changed, how many changes are inside it, or that it leads into changed code, and lists what was removed from it. Its code is shown as it changed: the definition around it, unchanged stretches folded (click to unfold), its own lines marked. A removed block says where it was and shows its old code.
- **Matching the versions**: files by path (following renames), folders by name or by what they hold (a package moved under `src/`), blocks by kind and title among their siblings, identical code first. Code moved within a file (steps wrapped in a new `if`) is the same block in its new place. Only files the change touched have changes; a function renamed or moved to another file is one removed and one added.

## Moving around

- **Unlimited nested zoom**: repository → folders and files → classes and functions → the steps inside them → the code a step calls, drawn inside the call.
- **Zoom and pan**:
  - scroll or pinch to zoom; `+` and `−` keys
  - drag or the arrow keys to pan
  - buttons for zoom, fit and whole project
- **Open a block** (double-click, double-tap or Enter): zooms until the diagram inside is readable, starting from its top. A block with nothing inside is centred among its neighbours.
- **Go to a definition** by double-clicking or double-tapping a block that has nothing inside but is defined elsewhere.
- **Back out** with a breadcrumb or by scrolling out. Esc closes a dialog, or else the details panel; it never moves the view. `F` shows all of the block the breadcrumbs end at, its title included; `H`, `0` or the home button shows the whole project. Framing keeps blocks clear of the top bar and the details panel.
- **Breadcrumbs** show the containers you're in; click one to go back out to it. Long paths fold their middle into a "…" menu, as far as the width needs.
- **"Defined in" line**: inside code reached through a call (`… › Block › forward › attn`), it names where that code is defined (`CausalSelfAttention.forward · model.py:52`) and takes you there.
- **Back and forward**: every jump is a history step. Use the ← → buttons, Alt+←/→, Cmd+[ / Cmd+], or the browser's and mouse's back buttons. Back returns to the exact view and selection.
- **Links to a view**: the URL always names the current view and selection (`#at=model.py/GPT/forward&sel=…`). Reloading or sharing reopens it, and the link button copies it.
- **Search** (`/` or ⌘K): files, classes, functions and blocks, by name or by a name inside another (`Block.forward`, `block forward`, `api.py get`, `Db::get`), definitions before the steps that mention them. Picking one flies there and selects it.
- **Smooth flights** that zoom out, pan and zoom back in. Motion is reduced when the system asks for it.
- **Clicks land on what you can see**: a block too small to make out is passed over for the one around it.
- **Download** the current view as SVG, sharp at any size with its text kept as text, or as PNG. It is drawn by the same code as the screen, so it shows exactly what you see.
- **Download a block alone** as SVG from its details panel, at the size its names read at: a block, a block a change removed (as it was), or the definition a followed value runs through, with the value's path lit.

## Phones and tablets

- **Touch**: pinch to zoom, drag to pan, tap to select, double-tap to open a block, tap an arrow to follow its value, tap a line of source to select its block.
- **Layout**: on a phone the brand, history and tools share the top row and the breadcrumbs get the row below; the details panel is a bottom sheet you can drag taller.
- **Finger-sized** buttons, and fields large enough that the page never zooms into them. Pinching the diagram zooms the diagram, not the page.
- Help lists the touch gestures instead of keys. Phones can't pick folders, so "Local folder…" picks files.

## Details panel (click a block)

- **What it is**: kind, role (`script · entry point`, `GET /api/users`, `DB row`…), name and docstring.
- **Zoom in** and **Go to definition / class** buttons.
- **Used by**: every place that uses the block, each a link to it.
  - Callers of a function.
  - Constructions and components of a class.
  - The calls of a component (`c_attn` → `CausalSelfAttention.forward`).
  - Pointing at a row points out that use. While the block is in focus, its uses are outlined in the diagram.
- **Inputs, outputs, tags and side effects**, plus table rows (settings, fields, interface methods).
- **Source** with syntax highlighting:
  - A step is shown inside its function, with its own lines marked and scrolled into view.
  - Long lines wrap under their indentation.
  - Click a line to select its block (the view moves only if that block is off screen). Point at a line to point out its block; point at a block to tint its lines.
- **Resizable**: drag its edge, use the arrow keys on the edge, or double-click the edge to reset. The size is remembered; a smaller window shrinks the panel without forgetting it. On narrow screens it becomes a bottom sheet whose title stays in view.

## Following values

- **Hover an arrow** to highlight it. Hovering or selecting a block highlights its arrows.
- **Click an arrow** to follow its value through the function:
  - every edge carrying that name, through steps that re-bind it (`q = q.view(…)`) and into and out of branches and loops
  - the rest of the function fades
  - a panel lists where the value comes from and what reads it, each a link
- Every arrow is labelled with what it carries, including arrows into and inside branches and loops.

## Reading the diagram

- **Read top to bottom**: inputs enter at the top, results leave at the bottom, and arrows are labelled with variable names.
- **Arrow styles**:
  - solid: a value, labelled with its name (every value, none hidden)
  - dotted: a condition
  - dashed red: a file one program writes and another reads
  - dashed: code that runs other code
- **Marks**:
  - ⊕: residual adds
  - orange dot: a side effect
  - corner mark: there is more inside
- **Colours by role**: function, route or command, class, sub-module, library call, I/O, settings, file on disk. Architecture-figure colours (attention, norm, linear, embedding…) apply only in code that builds neural networks.
- **Dark mode** follows the system's setting, and switches with it while the page is open: the diagram keeps each role's hue, its lightness turned over, and the panels, code and downloads follow.
- **Text stays readable**: names stay legible at any zoom, long names wrap onto two lines, and table rows never grow past the size of names. A file, folder, class or file on disk keeps its name however narrow its box, as long as the name fits it legibly (except inside the thumbnail of a closed block).
- **Tidy arrows** (the switch beside the home button, off by default, remembered): all the values a block sends share one line that branches to each reader, each branch labelled where it arrives. Off, each value has its own line, labelled where it leaves.
- **Legend**, **help** (keys, how to read, analysis notes) and **footer stats** (files, lines, blocks, analysis time).

## What becomes a block (all languages)

- Calls into project code, calls on sub-modules (`self.attn(x)`), library operations and I/O. Glue such as reshapes, builtins and size arithmetic is folded into the step it feeds.
- Titles are names, not cut-off code: `view · transpose`, the variable a step produces, or the field a store sets.
- **Operators**: values combined by `+ − × ÷ @ // % **` are a circle (⊕, ×, …) in every language. `a + b + c` is one ⊕ of three. A constant (`T + 1`) or a longer formula stays a formula step.
- Prints, asserts, imports, docstrings and `super().__init__()` become side-effect marks, not blocks.
- **Loops and branches** are drawn inline with repeat counts (`× n_layer`). A lone guarded statement gets an `if …` tag; long switches become a table of their arms.
- **Every way out is visible**: guarded exits (`return None · if value is None`), errors (`returns error`) and every returned value.
- **Long scripts** are split into sections at comment headings, and runs of constants become settings tables.
- **Light type inference** names things: `block` is a `Block`, `model.configure_optimizers` reaches `GPT.configure_optimizers`, and `with Session() as s` makes `s` what entering returns (a `Session`, unless its `__enter__` says otherwise).
- **Calls through contracts** (abstract methods, TypeScript interfaces, Rust `dyn Trait`, Go interfaces) go to the implementation when there is one, and name the candidates when there are several. Generic parameters count as the trait or interface they're bound by.

## Repository level

- **Programs and the files they exchange**: `prepare.py → train.bin → train.py → ckpt.pt → sample.py`.
- **Dependencies** between files and folders, transitively reduced so the layering shows.
- **File roles**: app, routes, script, tests, config, module, binary, library, example, benchmark, build script, notebook, generated.
- **Entry points** come from main guards, `__main__.py`, app objects, Dockerfiles, CI workflows, Makefiles, Compose files and console scripts.
- **Shell scripts** that run several programs are pipelines.
- **Docker Compose** is drawn as the system: its services, what each depends on and routes to, and the code each is built from.
- **Folded or hidden**:
  - generated code (migrations, protobuf, "auto-generated") is one block
  - empty `__init__.py` files are hidden
  - READMEs describe their folder
  - external packages are listed in one table

## Frameworks recognised

- **Routes, with their full paths**:
  - Python: FastAPI, Flask, Django `urlpatterns`
  - JavaScript: Express, Next.js, NestJS
  - Rust: axum, actix-web, Rocket
  - Go: gin, echo, chi, fiber, gorilla, net/http
- **Commands**: click, Typer, clap, cobra, urfave/cli.
- **Other roles**:
  - Celery tasks and request hooks
  - React components and hooks
  - Next.js pages and layouts
- **Data**:
  - schemas (zod, yup, joi, valibot, TypeBox, Mongoose) and DTOs are field tables
  - ORM models are tagged as tables
  - queries are database steps (SQLAlchemy, Django, Prisma, sqlx, diesel, sea-orm, gorm, pgx and others)

## Python

- Ruff's parser, which handles syntax from 3.8 to 3.15. A file with a syntax error still renders around the error.
- Notebooks are analyzed as Python; cells become sections titled by the markdown above them.

## JavaScript and TypeScript

- oxc's parser, which handles ES2025, current TypeScript and JSX. On a syntax error the broken statement is blanked out and the rest still renders.
- **Functions in every spelling**: declarations, arrows, `exports.f`, methods, getters and setters.
- **Objects that own functions** are drawn like classes: constructor functions with prototypes, objects with attached functions, and object literals of methods.
- **Callbacks** are drawn inside the call that takes them (`router.get(…)`, `addEventListener("click", …)`).
- **Promise chains** pass values on the way `await` does.
- **Imports resolve the way bundlers and `tsc` do**: tsconfig paths, `@/`, workspaces, `index` files, `require()` and `import()`.

## Rust

- syn's parser, editions 2015–2024. On a syntax error the item holding it is blanked out.
- **Types**: a struct or enum is drawn with its `impl` methods, and its subtitle lists the traits it implements. Inline modules hold their types.
- **Fields and macros**:
  - fields typed by project types are components, with wrappers looked through (`Arc<Mutex<Db>>` is a `Db`)
  - `format!`, `vec!`, `assert!`, `panic!` and logging macros read as what they do
- **Control flow and glue**:
  - `match` arms are their patterns
  - `?`, `Ok(x)` and `Err` are passed on or shown as exits
  - borrows, clones and conversions are left out
- **Modules resolve the way Cargo and rustc do**: manifests, workspaces, `mod` declarations with `#[path]`, `crate::`, `super::` and `self::`.
- Derives say what a type is for (`command line`, `DB row`, `error type`), and `#[cfg]` variants are tagged.

## Go

- A hand-written port of Go's own parser, up to Go 1.24. It recovers from syntax errors.
- **Packages**:
  - a package is its directory's files
  - imports resolve through `go.mod`, `replace` directives and `go.work`
  - build constraints tag files; `testdata` and `vendor` are skipped
- **Types and interfaces**:
  - methods are drawn with their type
  - interfaces are tables of method signatures
  - a type lists the project interfaces it satisfies, matched by signature
- **Errors**: `if err != nil { return err }` is passed on, not drawn. Errors made here are exits named by their message.
- **Goroutines and channels**: `go`/`defer` are marked; `select`, type switches and channel sends and receives are drawn.

## Other files

- Other languages get an outline: definitions, calls between definitions, and C/C++ imports.
- Config files become key/value tables, with `.env` values masked.
- Docs become heading lists, and binary files are assets.

## Limits

- **Static analysis**: arrows show likely dataflow, not a runtime trace.
- **Not seen**: code passed as strings, monkey-patching, and macros (read by name and arguments, not expanded).
- **Partly seen**:
  - generic types are not instantiated
  - a Go interface call is resolved only as far as the program shows which types go in
- **Size**: very large repositories work but are heavy in the browser (CPython's standard library is a 64 MB scene).
