//! Commands that run project code, found in shell scripts, Dockerfiles, CI
//! workflows, Makefiles, compose files, Procfiles and package manifests:
//! `python -m scripts.train`, `torchrun --nproc_per_node=8 train.py`,
//! `bash scripts/prestart.sh`, `uvicorn app.main:app`, `fastapi run`.
//! Each becomes a "runs" edge from the runner to the program, and makes the
//! program an entry point. Only commands count: a file merely mentioned
//! (`COPY pyproject.toml .`) is not run.

use regex_lite::Regex;
use std::sync::OnceLock;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A file path, relative to the working directory.
    Path(String),
    /// A dotted module (`python -m pkg.mod`, `uvicorn pkg.mod:app`).
    Module(String),
    /// The first of these paths that exists (`fastapi run` looks for main.py, app/main.py, …).
    Any(Vec<String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub target: Target,
    /// Directory the command runs in, relative to the runner's directory
    /// (from `cd` in scripts and `WORKDIR` in Dockerfiles), or "".
    pub cwd: String,
    /// What the edge says: "runs", or the command a console script installs.
    pub label: String,
    /// The command as written.
    pub command: String,
    /// The launcher and the arguments the program gets: `torchrun --depth=20`.
    pub summary: String,
}

macro_rules! rx {
    ($re:expr) => {{
        static CELL: OnceLock<Regex> = OnceLock::new();
        CELL.get_or_init(|| Regex::new($re).expect("valid invocation pattern"))
    }};
}

/// Every invocation of project code in a runner file.
pub fn runs(lang: &str, path: &str, text: &str) -> Vec<Run> {
    let base = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    let mut out = vec![];
    match lang {
        "Shell" | "PowerShell" => script(&logical_lines(text), &mut out),
        "Makefile" => {
            let recipes: Vec<String> = logical_lines(text).into_iter().filter(|l| l.starts_with('\t')).collect();
            script(&recipes, &mut out);
        }
        "Dockerfile" => dockerfile(text, &mut out),
        "YAML" if path.contains(".github/workflows/") || base == ".gitlab-ci.yml" || base.contains("pipeline") => {
            let cmds = yaml_commands(text, &["run", "script", "before_script", "after_script"]);
            script(&cmds, &mut out);
        }
        "YAML" if base.contains("compose") => {
            let cmds = yaml_commands(text, &["command", "entrypoint"]);
            for c in cmds {
                script(&[c], &mut out);
            }
        }
        "JSON" if base == "package.json" => {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
                if let Some(scripts) = v.get("scripts").and_then(|s| s.as_object()) {
                    let cmds: Vec<String> = scripts.values().filter_map(|s| s.as_str().map(str::to_string)).collect();
                    script(&cmds, &mut out);
                }
                // Commands the package installs: `"bin": {"tool": "./bin/tool.js"}`.
                let name = v.get("name").and_then(|n| n.as_str()).unwrap_or("").rsplit('/').next().unwrap_or("").to_string();
                let bins: Vec<(String, String)> = match v.get("bin") {
                    Some(serde_json::Value::String(p)) => vec![(name.clone(), p.clone())],
                    Some(serde_json::Value::Object(m)) => m.iter().filter_map(|(k, p)| p.as_str().map(|p| (k.clone(), p.to_string()))).collect(),
                    _ => vec![],
                };
                for (cmd, p) in bins {
                    let p = p.trim_start_matches("./").to_string();
                    out.push(Run { target: Target::Path(p), cwd: String::new(), label: format!("installs `{cmd}`"), command: cmd.clone(), summary: format!("command `{cmd}`") });
                }
            }
        }
        "TOML" if base == "pyproject.toml" => console_scripts(text, &mut out),
        "INI" if base == "setup.cfg" => setup_cfg_scripts(text, &mut out),
        _ if base == "procfile" => {
            let cmds: Vec<String> = text.lines().filter_map(|l| l.split_once(':').map(|(_, c)| c.trim().to_string())).collect();
            script(&cmds, &mut out);
        }
        _ => {}
    }
    out
}

/// Lines with `\` continuations joined and comments removed.
fn logical_lines(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    for line in text.lines() {
        let l = strip_comment(line);
        if let Some(head) = l.trim_end().strip_suffix('\\') {
            cur.push_str(head);
            cur.push(' ');
            continue;
        }
        cur.push_str(&l);
        out.push(std::mem::take(&mut cur));
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Removes a `#` comment that starts a word, outside quotes.
fn strip_comment(line: &str) -> String {
    let mut quote: Option<char> = None;
    let mut prev = ' ';
    for (i, c) in line.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '#' && prev.is_whitespace() => return line[..i].to_string(),
            None => {}
        }
        prev = c;
    }
    line.to_string()
}

/// Splits a command line into simple commands of words, honoring quotes.
fn commands(line: &str) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = vec![vec![]];
    let mut word = String::new();
    let mut quote: Option<char> = None;
    let mut chars = line.chars().peekable();
    let flush = |word: &mut String, out: &mut Vec<Vec<String>>| {
        if !word.is_empty() {
            out.last_mut().unwrap().push(std::mem::take(word));
        }
    };
    while let Some(c) = chars.next() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => word.push(c),
            None => match c {
                '"' | '\'' => quote = Some(c),
                ' ' | '\t' | '\n' => flush(&mut word, &mut out),
                ';' | '|' | '&' | '(' | ')' | '`' => {
                    flush(&mut word, &mut out);
                    while chars.peek().is_some_and(|n| matches!(n, '|' | '&')) {
                        chars.next();
                    }
                    out.push(vec![]);
                }
                _ => word.push(c),
            },
        }
    }
    flush(&mut word, &mut out);
    out.retain(|c| !c.is_empty());
    out
}

/// Commands of a script, tracking `cd` for the directory later commands run in.
fn script(lines: &[String], out: &mut Vec<Run>) {
    let mut cwd = String::new();
    for line in lines {
        for words in commands(line) {
            if words[0] == "cd" {
                if let Some(d) = words.get(1) {
                    cwd = join(&cwd, d);
                }
                continue;
            }
            if let Some(target) = program(&words) {
                let summary = summarize(&words, &target);
                out.push(Run { target, cwd: cwd.clone(), label: "runs".into(), command: words.join(" "), summary });
            }
        }
    }
}

/// The launcher and what follows the program: `torchrun --depth=20`.
fn summarize(words: &[String], target: &Target) -> String {
    let key = match target {
        Target::Path(p) | Target::Module(p) => p.as_str(),
        Target::Any(_) => "",
    };
    let wrapper = |w: &str| w.contains('=') || matches!(w, "sudo" | "exec" | "time" | "nohup" | "env" | "uv" | "run" | "poetry" | "pipenv" | "pdm" | "hatch" | "rye" | "pixi" | "docker" | "compose" | "docker-compose" | "npx" | "bunx");
    let launcher = words.iter().find(|w| !wrapper(w)).map(|w| w.rsplit('/').next().unwrap_or(w)).unwrap_or("");
    let after: Vec<&str> = match words.iter().position(|w| w == key || w.starts_with(&format!("{key}:"))) {
        Some(k) => words[k + 1..].iter().map(String::as_str).filter(|w| *w != "--").collect(),
        None => vec![],
    };
    if after.is_empty() || launcher == key {
        launcher.to_string()
    } else {
        format!("{launcher} {}", after.join(" "))
    }
}

fn join(cwd: &str, d: &str) -> String {
    if d.starts_with('/') || d.starts_with('$') || d.starts_with('~') {
        return String::new();
    }
    let mut parts: Vec<&str> = cwd.split('/').filter(|p| !p.is_empty()).collect();
    for p in d.split('/') {
        match p {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return String::new();
                }
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

/// Skips options (`-x`, `--flag=v`) and returns the index of the next word.
fn after_options(words: &[String], mut i: usize, valued: &[&str]) -> usize {
    while i < words.len() && words[i].starts_with('-') && words[i] != "--" {
        let flag = words[i].as_str();
        i += 1;
        if !flag.contains('=') && valued.contains(&flag) {
            i += 1;
        }
    }
    if words.get(i).is_some_and(|w| w == "--") {
        i += 1;
    }
    i
}

/// What project code a simple command runs, if any.
fn program(words: &[String]) -> Option<Target> {
    let mut i = 0;
    // Environment assignments and wrappers that run the rest of the line.
    loop {
        let w = words.get(i)?.as_str();
        let next = words.get(i + 1).map(String::as_str);
        if rx!(r"^[A-Za-z_][A-Za-z0-9_]*=").is_match(w) {
            // FLASK_APP=app flask run
            if let Some(v) = w.strip_prefix("FLASK_APP=") {
                if words[i + 1..].iter().any(|x| x == "flask") {
                    return Some(module_or_path(v));
                }
            }
            i += 1;
        } else if matches!(w, "sudo" | "exec" | "time" | "nohup" | "env" | "nice" | "command" | "xvfb-run" | "stdbuf" | "caffeinate" | "npx" | "bunx" | "cross-env" | "env-cmd" | "dotenv") {
            i = after_options(words, i + 1, &["-u", "-n"]);
        } else if w == "timeout" {
            i = after_options(words, i + 1, &["-s", "-k"]) + 1;
        } else if matches!(w, "uv" | "poetry" | "pipenv" | "pdm" | "hatch" | "rye" | "pixi" | "micromamba" | "conda" | "coverage" | "pnpm") && matches!(next, Some("run" | "exec")) {
            i = after_options(words, i + 2, &["-n", "--name", "-p", "--prefix", "--with", "--python", "-e", "--env", "--package", "--group", "--extra"]);
        } else if w == "docker" && matches!(next, Some("compose")) || w == "docker-compose" {
            let sub = if w == "docker" { i + 2 } else { i + 1 };
            if !matches!(words.get(sub).map(String::as_str), Some("exec" | "run")) {
                return None;
            }
            // Options, then the service, then the command it runs.
            i = after_options(words, sub + 1, &["-e", "--env", "-w", "--workdir", "-u", "--user", "--entrypoint", "--name", "-p", "--publish", "-v", "--volume", "--index"]) + 1;
        } else if w == "docker" && matches!(next, Some("exec")) {
            i = after_options(words, i + 2, &["-e", "--env", "-w", "--workdir", "-u", "--user"]) + 1;
        } else if matches!(w, "srun" | "mpirun" | "mpiexec") {
            i = after_options(words, i + 1, &["-n", "-np", "-N", "--ntasks", "-c", "--gpus", "-p", "--partition", "-t", "--time"]);
        } else {
            break;
        }
    }
    let prog = words.get(i)?.as_str();
    let name = prog.rsplit('/').next().unwrap_or(prog);
    let rest = &words[i + 1..];
    let first_with = |exts: &[&str]| rest.iter().find(|w| !w.starts_with('-') && exts.iter().any(|e| w.ends_with(e))).map(|w| Target::Path(w.clone()));
    if rx!(r"^(python|python3|python3\.\d+|py|pypy3?|ipython3?)$").is_match(name) {
        let mut k = 0;
        while let Some(w) = rest.get(k) {
            match w.as_str() {
                "-m" => {
                    let m = rest.get(k + 1)?;
                    // Tools run as modules are not project code.
                    let tool = matches!(m.split('.').next().unwrap_or(m), "pip" | "pytest" | "venv" | "http" | "build" | "twine" | "coverage" | "unittest" | "uv" | "ensurepip" | "pre_commit" | "mypy" | "ruff" | "black" | "isort" | "flake8" | "pylint" | "sphinx" | "mkdocs" | "jupyter" | "notebook" | "ipykernel" | "torch" | "spacy" | "nltk" | "playwright" | "grpc_tools" | "alembic" | "uvicorn" | "gunicorn" | "celery" | "flask" | "streamlit");
                    return if tool { None } else { Some(Target::Module(m.clone())) };
                }
                "-c" => return None,
                "-W" | "-X" | "-Q" => k += 2,
                w if w.starts_with('-') => k += 1,
                w => return (w.ends_with(".py") || w.ends_with(".pyw")).then(|| Target::Path(w.into())),
            }
        }
        return None;
    }
    if matches!(name, "torchrun" | "deepspeed" | "horovodrun" | "accelerate" | "lightning" | "composer") {
        if let Some(k) = rest.iter().position(|w| w == "-m" || w == "--module") {
            return rest.get(k + 1).map(|m| Target::Module(m.clone()));
        }
        return first_with(&[".py"]);
    }
    if matches!(name, "bash" | "sh" | "zsh" | "source" | "." | "ksh" | "dash") {
        let j = after_options(rest, 0, &["-c"]);
        return rest.get(j).filter(|w| !w.contains(' ')).map(|w| Target::Path(w.clone()));
    }
    if matches!(name, "node" | "bun" | "deno" | "ts-node" | "tsx" | "esno" | "nodemon" | "node-dev" | "ts-node-dev" | "vite-node" | "jiti") {
        return first_with(&[".js", ".mjs", ".cjs", ".ts", ".mts"]);
    }
    if matches!(name, "uvicorn" | "gunicorn" | "hypercorn" | "daphne" | "granian") {
        return rest.iter().find(|w| !w.starts_with('-') && rx!(r"^[A-Za-z_][\w.]*:[A-Za-z_][\w.()]*$").is_match(w)).map(|w| Target::Module(w.split(':').next().unwrap().to_string())).or_else(|| {
            // Django: `gunicorn project.wsgi`.
            rest.iter().rev().find(|w| !w.starts_with('-') && (w.ends_with(".wsgi") || w.ends_with(".asgi"))).map(|w| Target::Module(w.clone()))
        });
    }
    if name == "fastapi" && matches!(rest.first().map(String::as_str), Some("run" | "dev")) {
        let j = after_options(rest, 1, &["--host", "--port", "--workers", "--root-path", "--app", "--entrypoint"]);
        return Some(match rest.get(j) {
            Some(p) => Target::Path(p.clone()),
            None => Target::Any(["main.py", "app.py", "api.py", "app/main.py", "app/app.py", "app/api.py"].iter().map(|s| s.to_string()).collect()),
        });
    }
    if name == "flask" {
        let app = rest.iter().position(|w| w == "--app" || w == "-A").and_then(|k| rest.get(k + 1)).cloned().or_else(|| rest.iter().find_map(|w| w.strip_prefix("--app=").map(str::to_string)));
        return app.map(|a| module_or_path(&a));
    }
    if matches!(name, "celery" | "rq" | "dramatiq" | "huey_consumer") {
        let app = rest.iter().position(|w| w == "-A" || w == "--app").and_then(|k| rest.get(k + 1)).cloned().or_else(|| rest.iter().find_map(|w| w.strip_prefix("--app=").map(str::to_string)));
        return app.map(|a| module_or_path(a.split(':').next().unwrap_or(&a)));
    }
    if matches!(name, "streamlit" | "chainlit" | "marimo" | "panel" | "gradio" | "papermill" | "solara") {
        return first_with(&[".py", ".ipynb"]);
    }
    if name == "jupyter" && rest.iter().any(|w| w == "--execute" || w == "execute") {
        return first_with(&[".ipynb"]);
    }
    if name == "alembic" && !rest.is_empty() {
        return Some(Target::Any(vec!["alembic/env.py".into(), "migrations/env.py".into()]));
    }
    // A script run directly: `./train.sh`, `scripts/run.py`.
    if (prog.contains('/') || prog.ends_with(".sh") || prog.ends_with(".py")) && rx!(r"\.(sh|bash|py|zsh)$").is_match(prog) {
        return Some(Target::Path(prog.into()));
    }
    None
}

/// `app.main`, `app/main.py` or `flaskr` (a package).
fn module_or_path(v: &str) -> Target {
    let v = v.split(':').next().unwrap_or(v).trim_end_matches("()");
    if v.ends_with(".py") || v.contains('/') {
        Target::Path(v.into())
    } else {
        Target::Module(v.into())
    }
}

/// `RUN`, `CMD` and `ENTRYPOINT` commands, with `WORKDIR` as their directory.
fn dockerfile(text: &str, out: &mut Vec<Run>) {
    let mut cwd = String::new();
    let mut absolute_workdir = false;
    for line in logical_lines(text) {
        let t = line.trim();
        let Some((instr, rest)) = t.split_once(char::is_whitespace) else { continue };
        let rest = rest.trim();
        match instr.to_ascii_uppercase().as_str() {
            "WORKDIR" => {
                // Container paths rarely mirror the repository; only relative ones are kept.
                absolute_workdir = rest.starts_with('/');
                cwd = if absolute_workdir { String::new() } else { join(&cwd, rest) };
            }
            "RUN" | "CMD" | "ENTRYPOINT" => {
                let rest = rx!(r"^(?:--\S+\s+)*").replace(rest, "").to_string();
                let words: Vec<String> = match serde_json::from_str::<Vec<String>>(&rest) {
                    Ok(v) => v,
                    Err(_) => {
                        script(&[rest.clone()], out);
                        continue;
                    }
                };
                if let Some(target) = program(&words) {
                    let summary = summarize(&words, &target);
                    out.push(Run { target, cwd: cwd.clone(), label: "runs".into(), command: words.join(" "), summary });
                }
            }
            _ => {}
        }
    }
    let _ = absolute_workdir;
}

/// Values of the given keys in a YAML file: inline strings, `|` blocks, and
/// flow or block lists (`command: ["python", "x.py"]`).
fn yaml_commands(text: &str, keys: &[&str]) -> Vec<String> {
    let mut out = vec![];
    let lines: Vec<&str> = text.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let indent = line.len() - line.trim_start().len();
        let t = line.trim_start().trim_start_matches("- ").trim_start();
        let Some((key, value)) = t.split_once(':') else {
            i += 1;
            continue;
        };
        if !keys.contains(&key.trim()) {
            i += 1;
            continue;
        }
        let value = strip_comment(value).trim().to_string();
        i += 1;
        if value.starts_with('|') || value.starts_with('>') || value.is_empty() {
            // A block scalar, or a block list of commands.
            let mut block = vec![];
            while i < lines.len() && (lines[i].trim().is_empty() || lines[i].len() - lines[i].trim_start().len() > indent) {
                let l = lines[i].trim();
                if !l.is_empty() {
                    block.push(l.strip_prefix("- ").unwrap_or(l).trim_matches(['"', '\'']).to_string());
                }
                i += 1;
            }
            if value.is_empty() && block.iter().all(|l| !l.contains(' ')) && block.len() > 1 {
                out.push(block.join(" "));
            } else {
                out.extend(block);
            }
        } else if value.starts_with('[') {
            let words: Vec<String> = value.trim_matches(['[', ']']).split(',').map(|w| w.trim().trim_matches(['"', '\'']).to_string()).filter(|w| !w.is_empty()).collect();
            out.push(words.join(" "));
        } else {
            out.push(value.trim_matches(['"', '\'']).to_string());
        }
    }
    out
}

/// `[project.scripts] http = "httpie.__main__:main"` installs a command.
fn console_scripts(text: &str, out: &mut Vec<Run>) {
    let mut in_scripts = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            in_scripts = matches!(t, "[project.scripts]" | "[project.gui-scripts]" | "[tool.poetry.scripts]" | "[tool.flit.scripts]");
            continue;
        }
        if !in_scripts {
            continue;
        }
        if let Some(c) = rx!(r#"^["']?([\w.\-]+)["']?\s*=\s*["']([\w.]+)(?::[\w.]+)?["']"#).captures(t) {
            out.push(Run { target: Target::Module(c[2].to_string()), cwd: String::new(), label: format!("installs `{}`", &c[1]), command: c[1].to_string(), summary: format!("command `{}`", &c[1]) });
        }
    }
}

/// `[options.entry_points] console_scripts = http = httpie.__main__:main`.
fn setup_cfg_scripts(text: &str, out: &mut Vec<Run>) {
    let mut section = String::new();
    let mut in_scripts = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            section = t.to_string();
            in_scripts = false;
            continue;
        }
        if section != "[options.entry_points]" {
            continue;
        }
        if !line.starts_with([' ', '\t']) {
            in_scripts = t.starts_with("console_scripts") || t.starts_with("gui_scripts");
            continue;
        }
        if let (true, Some(c)) = (in_scripts, rx!(r"^([\w.\-]+)\s*=\s*([\w.]+)(?::[\w.]+)?").captures(t)) {
            out.push(Run { target: Target::Module(c[2].to_string()), cwd: String::new(), label: format!("installs `{}`", &c[1]), command: c[1].to_string(), summary: format!("command `{}`", &c[1]) });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targets(lang: &str, path: &str, text: &str) -> Vec<Target> {
        runs(lang, path, text).into_iter().map(|r| r.target).collect()
    }

    #[test]
    fn shell_pipelines() {
        let sh = "#!/bin/bash\nset -e\npython -m nanochat.dataset -n 8 &\ntorchrun --standalone --nproc_per_node=8 -m scripts.base_train -- --depth=20\nuv run python scripts/tok_eval.py  # eval\nbash runs/miniseries.sh\ncd backend && python app/initial_data.py\npip install -r requirements.txt\npython -m pytest tests\necho done\n";
        assert_eq!(
            targets("Shell", "runs/speedrun.sh", sh),
            vec![
                Target::Module("nanochat.dataset".into()),
                Target::Module("scripts.base_train".into()),
                Target::Path("scripts/tok_eval.py".into()),
                Target::Path("runs/miniseries.sh".into()),
                Target::Path("app/initial_data.py".into()),
            ]
        );
        assert_eq!(runs("Shell", "x.sh", sh)[4].cwd, "backend");
    }

    #[test]
    fn servers_and_containers() {
        let df = "FROM python:3.12\nWORKDIR /app/\nCOPY pyproject.toml .\nRUN --mount=type=cache,target=/root/.cache uv sync\nCMD [\"fastapi\", \"run\", \"--workers\", \"4\"]\n";
        assert!(matches!(&targets("Dockerfile", "backend/Dockerfile", df)[..], [Target::Any(c)] if c.contains(&"app/main.py".to_string())));
        assert_eq!(targets("Shell", "s.sh", "gunicorn -k uvicorn.workers.UvicornWorker app.main:app"), vec![Target::Module("app.main".into())]);
        assert_eq!(targets("Shell", "s.sh", "docker compose exec -T backend bash scripts/tests-start.sh \"$@\""), vec![Target::Path("scripts/tests-start.sh".into())]);
        assert_eq!(targets("Shell", "s.sh", "flask --app flaskr run --debug"), vec![Target::Module("flaskr".into())]);
        let compose = "services:\n  web:\n    command: [\"python\", \"manage.py\", \"runserver\"]\n  worker:\n    command: celery -A proj worker\n";
        assert_eq!(targets("YAML", "docker-compose.yml", compose), vec![Target::Path("manage.py".into()), Target::Module("proj".into())]);
    }

    #[test]
    fn workflows_and_manifests() {
        let wf = "jobs:\n  test:\n    steps:\n      - run: |\n          pip install .\n          python scripts/check.py --strict\n      - run: python -m tools.lint\n";
        assert_eq!(targets("YAML", ".github/workflows/ci.yml", wf), vec![Target::Path("scripts/check.py".into()), Target::Module("tools.lint".into())]);
        let toml = "[project]\nname = \"httpie\"\n\n[project.scripts]\nhttp = \"httpie.__main__:main\"\nhttps = 'httpie.__main__:main_https'\n";
        let r = runs("TOML", "pyproject.toml", toml);
        assert_eq!(r[0].target, Target::Module("httpie.__main__".into()));
        assert_eq!(r[0].label, "installs `http`");
        assert_eq!(targets("JSON", "package.json", r#"{"scripts": {"dev": "vite", "seed": "node scripts/seed.mjs"}}"#), vec![Target::Path("scripts/seed.mjs".into())]);
        assert_eq!(targets("Makefile", "Makefile", "run:\n\tpython train.py config/x.py\n"), vec![Target::Path("train.py".into())]);
        let cfg = "[metadata]\nname = httpie\n\n[options.entry_points]\nconsole_scripts =\n    http = httpie.__main__:main\n    httpie = httpie.manager.__main__:main\n";
        assert_eq!(targets("INI", "setup.cfg", cfg), vec![Target::Module("httpie.__main__".into()), Target::Module("httpie.manager.__main__".into())]);
    }
}
