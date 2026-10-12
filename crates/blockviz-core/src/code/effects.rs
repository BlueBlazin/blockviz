//! Side-effect classification of calls, and file names they touch.
//!
//! These are syntactic heuristics: they recognise common library entry points
//! (torch/numpy/pickle I/O, requests, subprocess, ...) and literal file names
//! so the repo view can connect a writer of `train.bin` to its readers.

use super::source::Source;
use super::syntax::{self as ast, Expr, TemplatePart};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Io {
    Read,
    Write,
    Network,
}

const FILE_EXTS: &[&str] = &[
    "bin", "pt", "pth", "ckpt", "pkl", "pickle", "npy", "npz", "json", "jsonl", "txt", "csv", "tsv", "parquet", "h5", "hdf5", "safetensors", "yaml", "yml", "toml", "png", "jpg", "jpeg", "gif", "svg", "pdf", "log", "zip", "tar", "gz", "onnx", "model", "vocab", "wav", "mp3", "mp4", "db", "sqlite", "xml", "html", "py", "sh", "cfg", "ini", "arrow", "feather", "msgpack", "tfrecord", "idx", "dat", "md",
];

/// A literal that names a file (`'ckpt.pt'`, `'data/train.bin'`) → its base name.
pub fn file_name(s: &str) -> Option<String> {
    let s = s.trim();
    if s.is_empty() || s.len() > 200 || s.contains("://") || s.contains(char::is_whitespace) || s.starts_with('.') && !s.starts_with("./") {
        return None;
    }
    let base = s.rsplit('/').next()?;
    let (stem, ext) = base.rsplit_once('.')?;
    if stem.is_empty() || !FILE_EXTS.contains(&ext.to_ascii_lowercase().as_str()) {
        return None;
    }
    if !stem.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '*' | '.')) {
        return None;
    }
    Some(base.to_string())
}

/// `f'{split}.bin'` → `*.bin`.
pub fn fstring_file(j: &ast::ExprTemplate) -> Option<String> {
    if j.kind != ast::TemplateKind::Format {
        return None;
    }
    let mut pattern = String::new();
    for part in &j.parts {
        match part {
            TemplatePart::Literal(l) => pattern.push_str(l),
            TemplatePart::Hole { .. } => pattern.push('*'),
        }
    }
    let base = pattern.rsplit('/').next()?.to_string();
    if base.chars().filter(|c| *c != '*').count() < 3 {
        return None;
    }
    file_name(&base)
}

fn mode_of(c: &ast::ExprCall, positional: usize) -> Option<String> {
    let kw = c.arguments.keywords.iter().find(|k| k.arg.as_ref().map(|a| a.as_str()) == Some("mode")).map(|k| &k.value);
    let e = kw.or_else(|| c.arguments.args.get(positional))?;
    if let Expr::StringLiteral(k) = e {
        return Some(k.value.clone());
    }
    None
}

pub fn open_mode(c: &ast::ExprCall) -> Io {
    match mode_of(c, 1) {
        Some(m) if m.contains('w') || m.contains('a') || m.contains('x') || m.contains('+') => Io::Write,
        _ => Io::Read,
    }
}

/// Go's standard library and common clients, by import path and function.
fn go_io(path: &str) -> Option<Io> {
    let (pkg, f) = path.rsplit_once('.')?;
    Some(match (pkg, f) {
        ("os", "ReadFile" | "Open" | "ReadDir" | "Stat" | "Lstat") | ("io/ioutil", "ReadFile" | "ReadDir") | ("embed", _) => Io::Read,
        ("os", "WriteFile" | "Create" | "OpenFile" | "Mkdir" | "MkdirAll" | "Remove" | "RemoveAll" | "Rename" | "Truncate" | "Symlink" | "Chmod") | ("io/ioutil", "WriteFile") => Io::Write,
        ("net", "Dial" | "DialTimeout" | "Listen" | "ListenTCP" | "ListenUDP" | "DialTCP" | "DialUDP" | "ListenPacket") | ("net/http", "Get" | "Post" | "PostForm" | "Head" | "NewRequest" | "NewRequestWithContext" | "ListenAndServe" | "ListenAndServeTLS" | "Serve") | ("crypto/tls", "Dial" | "Listen") | ("net/rpc", _) | ("net/smtp", _) => Io::Network,
        ("google.golang.org/grpc", "Dial" | "NewClient" | "DialContext") => Io::Network,
        // Methods of library objects: `client.Do(req)`, `conn.Write(b)`, `f.ReadAt(…)`.
        ("net/http.Client", "Do" | "Get" | "Post" | "PostForm" | "Head") | ("net/http.Server", "ListenAndServe" | "ListenAndServeTLS" | "Serve") | ("net.Conn" | "net.TCPConn" | "net.UDPConn" | "net.Listener", _) => Io::Network,
        ("os.File", "Write" | "WriteString" | "WriteAt" | "Truncate" | "Sync") | ("bufio.Writer", "Flush") => Io::Write,
        ("os.File", "Read" | "ReadAt" | "ReadDir" | "Readdir" | "Readdirnames") => Io::Read,
        // Web frameworks' servers.
        ("github.com/gin-gonic/gin.Engine" | "github.com/gin-gonic/gin.Default" | "github.com/gin-gonic/gin.New", "Run" | "RunTLS" | "RunUnix") | ("github.com/labstack/echo.Echo" | "github.com/labstack/echo/v4.Echo", "Start" | "StartTLS") | ("github.com/gofiber/fiber/v2.App" | "github.com/gofiber/fiber/v3.App", "Listen" | "ListenTLS") => Io::Network,
        _ => return None,
    })
}

/// File or network I/O performed by an external function, by dotted path.
pub fn io_of(path: &str, c: &ast::ExprCall, src: &Source) -> Option<(String, Io)> {
    let last = path.rsplit('.').next().unwrap_or(path);
    let root = path.split('.').next().unwrap_or(path);
    // Each ecosystem's libraries: Node's `http` is not Python's `http.cookiejar`.
    let io = match src.lang {
        ast::Lang::JavaScript | ast::Lang::TypeScript => js_io(root.strip_prefix("node:").unwrap_or(root), last),
        ast::Lang::Rust => rust_io(path),
        ast::Lang::Go => go_io(path),
        ast::Lang::Python => None,
    };
    let io = io.or_else(|| match path {
        "torch.save" | "numpy.save" | "numpy.savez" | "numpy.savez_compressed" | "numpy.savetxt" | "pickle.dump" | "json.dump" | "yaml.dump" | "yaml.safe_dump" | "torch.jit.save" | "toml.dump" | "joblib.dump" | "matplotlib.pyplot.savefig" | "safetensors.torch.save_file" | "torch.onnx.export" => Some(Io::Write),
        "torch.load" | "numpy.load" | "numpy.fromfile" | "numpy.loadtxt" | "numpy.genfromtxt" | "pickle.load" | "json.load" | "yaml.load" | "yaml.safe_load" | "toml.load" | "tomllib.load" | "joblib.load" | "safetensors.torch.load_file" | "torch.jit.load" | "PIL.Image.open" => Some(Io::Read),
        "numpy.memmap" => Some(match mode_of(c, 2) {
            Some(m) if m == "r" || m == "c" => Io::Read,
            _ => Io::Write,
        }),
        _ if root == "pandas" && last.starts_with("read_") => Some(Io::Read),
        _ if matches!(root, "requests" | "httpx" | "aiohttp" | "urllib3" | "socket" | "boto3" | "wget" | "huggingface_hub") => Some(Io::Network),
        // `http.client` and `urllib.request` talk to servers; `http.cookiejar`
        // and `urllib.parse` do not.
        _ if path.starts_with("http.client") || path.starts_with("urllib.request") => Some(Io::Network),
        _ if last == "from_pretrained" || last == "load_dataset" || last == "hf_hub_download" || path.starts_with("torch.hub.") => Some(Io::Network),
        _ => None,
    })?;
    let text = match io {
        Io::Read => "reads file",
        Io::Write => "writes file",
        Io::Network => "network",
    };
    Some((text.into(), io))
}

/// I/O by Rust crates, by dotted path: `std::fs` and its async twins, sockets
/// and HTTP clients.
fn rust_io(path: &str) -> Option<Io> {
    let segs: Vec<&str> = path.split('.').collect();
    let runtime = matches!(segs[0], "std" | "tokio" | "async_std" | "smol");
    let last = segs.last().copied().unwrap_or("");
    if runtime && segs.get(1) == Some(&"fs") {
        return if matches!(last, "read" | "read_to_string" | "read_dir" | "metadata" | "symlink_metadata" | "canonicalize" | "read_link" | "try_exists" | "open" | "File") {
            Some(Io::Read)
        } else if matches!(last, "write" | "create" | "OpenOptions") {
            Some(Io::Write)
        } else {
            None
        };
    }
    if runtime && segs.get(1) == Some(&"net") {
        return Some(Io::Network);
    }
    match segs[0] {
        "reqwest" | "ureq" | "hyper" | "surf" | "isahc" | "attohttpc" | "tonic" | "tungstenite" | "tokio_tungstenite" | "awc" | "curl" | "lettre" | "rumqttc" | "lapin" | "rdkafka" | "async_nats" => Some(Io::Network),
        _ => None,
    }
}

/// I/O by JavaScript modules and globals: Node's `fs`, HTTP clients, `fetch`.
fn js_io(module: &str, last: &str) -> Option<Io> {
    match module {
        "fs" | "fs/promises" | "fs-extra" | "graceful-fs" => {
            let l = last.trim_end_matches("Sync");
            if matches!(l, "readFile" | "createReadStream" | "readdir" | "readJson" | "readJSON" | "stat" | "lstat" | "exists" | "access" | "opendir" | "readlink" | "glob") {
                Some(Io::Read)
            } else if matches!(l, "writeFile" | "appendFile" | "createWriteStream" | "mkdir" | "mkdtemp" | "rm" | "rmdir" | "unlink" | "copyFile" | "cp" | "rename" | "outputFile" | "writeJson" | "writeJSON" | "outputJson" | "remove" | "ensureDir" | "ensureFile" | "move" | "copy" | "emptyDir" | "symlink" | "truncate" | "chmod") {
                Some(Io::Write)
            } else {
                None
            }
        }
        "axios" | "got" | "node-fetch" | "ky" | "superagent" | "undici" | "https" | "ws" | "socket.io-client" | "cross-fetch" | "isomorphic-fetch" | "ofetch" | "graphql-request" => Some(Io::Network),
        "http" if last != "createServer" => Some(Io::Network),
        "globals" if matches!(last, "fetch" | "WebSocket" | "EventSource" | "XMLHttpRequest" | "sendBeacon") => Some(Io::Network),
        _ => None,
    }
}

/// Non-I/O effects worth surfacing.
pub fn ext_effect(path: &str) -> Option<&'static str> {
    let last = path.rsplit('.').next().unwrap_or(path);
    let root = path.split('.').next().unwrap_or(path);
    let root = root.strip_prefix("node:").unwrap_or(root);
    match path {
        "globals.process.exit" => return Some("exits process"),
        _ if path.starts_with("globals.localStorage.") || path.starts_with("globals.sessionStorage.") => return Some("browser storage"),
        _ => {}
    }
    if matches!(root, "child_process" | "execa" | "cross-spawn" | "zx") {
        return Some("spawns process");
    }
    // Rust's standard library and runtimes.
    if matches!(root, "std" | "tokio" | "async_std") {
        let rest = &path[root.len() + 1.min(path.len() - root.len())..];
        match rest {
            "process.Command" | "process.Command.new" => return Some("spawns process"),
            "process.exit" | "process.abort" => return Some("exits process"),
            "thread.sleep" | "time.sleep" | "time.sleep_until" | "task.sleep" => return Some("sleeps"),
            "env.var" | "env.var_os" | "env.vars" | "env.args" | "env.args_os" => return Some("reads environment"),
            "fs.create_dir" | "fs.create_dir_all" | "fs.remove_file" | "fs.remove_dir" | "fs.remove_dir_all" | "fs.rename" | "fs.copy" | "fs.hard_link" | "fs.set_permissions" => return Some("filesystem"),
            _ => {}
        }
    }
    match path {
        "random.seed" | "numpy.random.seed" | "torch.manual_seed" | "torch.cuda.manual_seed" | "torch.cuda.manual_seed_all" | "tensorflow.random.set_seed" => return Some("seeds RNG"),
        "os.makedirs" | "os.mkdir" | "os.remove" | "os.rename" | "os.unlink" | "os.rmdir" | "os.replace" => return Some("filesystem"),
        "os.system" | "os.popen" => return Some("spawns process"),
        "sys.exit" | "os._exit" => return Some("exits process"),
        "time.sleep" => return Some("sleeps"),
        _ => {}
    }
    match root {
        "subprocess" => Some("spawns process"),
        "shutil" => Some("filesystem"),
        "wandb" | "mlflow" | "neptune" | "comet_ml" | "tensorboard" => Some("logs metrics (network)"),
        "logging" => Some("logs"),
        _ if last == "init_process_group" || last == "destroy_process_group" || path.starts_with("torch.distributed.") => Some("distributed"),
        _ => None,
    }
}

/// I/O performed by a method on a value (`f.read()`, `arr.tofile(path)`).
pub fn method_io(m: &str) -> Option<Io> {
    match m {
        "read" | "readlines" | "readline" | "read_text" | "read_bytes" => Some(Io::Read),
        "write" | "writelines" | "write_text" | "write_bytes" | "tofile" | "to_csv" | "to_parquet" | "to_json" | "to_pickle" | "savefig" | "save_pretrained" | "save" | "flush" => Some(Io::Write),
        _ => None,
    }
}
