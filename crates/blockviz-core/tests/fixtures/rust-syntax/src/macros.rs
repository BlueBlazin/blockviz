//! Macros: declarative definitions, standard macros, and macros whose
//! bodies are not Rust.
use std::io::Write;

macro_rules! square {
    ($x:expr) => {
        $x * $x
    };
}

thread_local! {
    static COUNTER: std::cell::Cell<u32> = std::cell::Cell::new(0);
}

lazy_static::lazy_static! {
    static ref NAMES: Vec<&'static str> = vec!["a", "b"];
}

pub fn standard(name: &str, out: &mut impl Write) -> std::io::Result<String> {
    let greeting = format!("Hello, {name}! You are {} years old.", 42);
    println!("{greeting}");
    eprintln!("debug: {:?}", name);
    writeln!(out, "{}", greeting)?;
    let zeros = vec![0u8; 16];
    let listed = vec![1, 2, 3];
    assert!(zeros.len() > listed.len(), "zeros {} vs {}", zeros.len(), listed.len());
    assert_eq!(square!(3), 9);
    let kind = if matches!(name, "admin" | "root") { "super" } else { "user" };
    if name.is_empty() {
        panic!("no name given");
    }
    let path = concat!("a", "/", "b");
    let home = env!("CARGO_MANIFEST_DIR");
    let _ = dbg!(kind);
    Ok(format!("{kind}:{path}:{home}"))
}

pub fn not_rust(id: u32, title: &str) -> String {
    let body = serde_json::json!({ "id": id, "title": title, "tags": ["x"] });
    body.to_string()
}

pub fn todo_later() -> u32 {
    todo!()
}

pub fn unreachable_arm(x: Option<u32>) -> u32 {
    match x {
        Some(v) => v,
        None => unreachable!("checked by the caller"),
    }
}
