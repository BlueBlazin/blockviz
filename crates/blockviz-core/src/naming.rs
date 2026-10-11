//! Block titles, independent of the source language.
//!
//! A front end describes a step by its shape (`Step`) and where its result
//! goes (`Dest`); these rules turn that into what a developer would write on
//! a whiteboard:
//!
//! * A title is a name: the operation for calls, the developer's own name for
//!   a value it produces (`token`, `headers`), the field a store sets. A step
//!   with neither is named by a short phrase (`check scheme, port`).
//! * Source code appears only when it fits whole (`T + 1`, `data[idx]`);
//!   code is never cut mid-token. Text templates are prose and may be
//!   shortened at a word boundary.
//! * The caption says how: the formula, the receiver, or the kind of step.

/// Longest source text shown whole as a title or caption.
pub const TITLE_MAX: usize = 28;
pub const CAPTION_MAX: usize = 32;
/// Developer-chosen names are worth a slightly wider box.
const NAMES_MAX: usize = 32;

/// Where a step's result goes.
#[derive(Debug, Clone, PartialEq)]
pub enum Dest {
    /// Not stored: returned, passed on, or discarded.
    None,
    /// Bound to local names (`x = …`, `a, b = …`).
    Names(Vec<String>),
    /// Stored in a named field of an object or map (`obj.k = …`, `obj["k"] = …`).
    Field { owner: String, key: String, item: bool },
    /// Stored at a computed position (`buf[i] = …`).
    Item { owner: String, index: String },
}

impl Dest {
    /// The field a store sets, or the place it writes: `tls`, `[i]`.
    pub fn field_key(&self) -> String {
        match self {
            Dest::Field { key, .. } => key.clone(),
            Dest::Item { index, .. } if index.is_empty() => "[…]".into(),
            Dest::Item { index, .. } => format!("[{index}]"),
            Dest::Names(ns) => list(ns, TITLE_MAX),
            Dest::None => String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Collection {
    Map,
    List,
    Tuple,
    Set,
}

/// What a step computes, by shape rather than syntax.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// A chain of calls, innermost first (`x.view(…).transpose(…)` → view,
    /// transpose), applied to `receiver`; `reads` are the variables it reads.
    Calls { names: Vec<String>, receiver: String, reads: Vec<String> },
    /// Text built from a template, with holes shown as `{name}`.
    Format { template: String },
    /// A collection display; `keys` are a mapping's keys or the elements' names.
    Pack { kind: Collection, keys: Vec<String> },
    /// Each element of `over`, transformed and/or filtered.
    Map { over: String, filtered: bool },
    /// A comparison or other test of `subjects`.
    Test { subjects: Vec<String> },
    /// Arithmetic over `subjects`.
    Compute { subjects: Vec<String> },
    /// One of several values (`a if c else b`, `a or b`).
    Choose { subjects: Vec<String> },
    /// Part of a value (`x[k]`, `x.k`).
    Pick { from: String },
    /// An in-place update (`x += …`).
    Update { op: String, operand: String },
    /// Leaving early: `return`, `continue`, `break`; `value` is what is returned.
    Exit { verb: String, value: String },
    Other,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Name {
    pub title: String,
    pub caption: String,
}

fn fits(s: &str, max: usize) -> bool {
    !s.is_empty() && s.chars().count() <= max
}

/// Joins names into a title: `a, b, c +2`.
pub fn list(names: &[String], max: usize) -> String {
    join(names, ", ", max)
}

/// Joins names with `sep` up to `max` characters, counting the rest: `add → commit +2`.
pub fn join(names: &[String], sep: &str, max: usize) -> String {
    let mut out = String::new();
    for (i, n) in names.iter().enumerate() {
        let next = if out.is_empty() { n.clone() } else { format!("{out}{sep}{n}") };
        if next.chars().count() > max && i > 0 {
            return format!("{out} +{}", names.len() - i);
        }
        out = next;
    }
    out
}

/// Shortens prose at a word boundary.
pub fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max.saturating_sub(1)).collect();
    let at = cut.rfind([' ', '/', '-', ',', '.', '{', '(']).filter(|&i| i >= max / 2).unwrap_or(cut.len());
    format!("{}…", cut[..at].trim_end_matches([' ', ',', '-']))
}

/// The receiver of a call or owner of a field, short enough for a caption:
/// `self.env.rich_error_console` → `env.rich_error_console`.
pub fn qualifier(path: &str, max: usize) -> String {
    if path.chars().count() <= max {
        return path.to_string();
    }
    let parts: Vec<&str> = path.split('.').collect();
    for k in (1..parts.len()).rev() {
        let tail = parts[parts.len() - k..].join(".");
        if tail.chars().count() <= max {
            return tail;
        }
    }
    String::new()
}

/// A short phrase for a step that has no name of its own.
fn phrase(step: &Step) -> String {
    let some = |xs: &[String]| list(xs, 22);
    match step {
        Step::Calls { names, .. } => calls(names),
        Step::Format { template } => shorten(template, TITLE_MAX),
        Step::Pack { kind: Collection::Map, keys } if !keys.is_empty() => format!("{{{}}}", list(keys, TITLE_MAX - 2)),
        Step::Pack { keys, .. } if !keys.is_empty() => {
            let mut uniq: Vec<String> = vec![];
            for k in keys {
                if !uniq.contains(k) {
                    uniq.push(k.clone());
                }
            }
            list(&uniq, TITLE_MAX)
        }
        Step::Pack { kind, .. } => match kind {
            Collection::Map => "dict",
            Collection::List => "list",
            Collection::Tuple => "tuple",
            Collection::Set => "set",
        }
        .to_string(),
        Step::Map { over, filtered: true } => format!("filter {over}"),
        Step::Map { over, .. } => format!("map {over}"),
        Step::Test { subjects } if !subjects.is_empty() => format!("check {}", some(subjects)),
        Step::Test { .. } => "check".into(),
        Step::Compute { subjects } if !subjects.is_empty() => format!("compute {}", some(subjects)),
        Step::Choose { subjects } if !subjects.is_empty() => format!("choose {}", some(subjects)),
        Step::Pick { from } if !from.is_empty() => format!("from {from}"),
        Step::Update { op, .. } => op.clone(),
        Step::Exit { verb, .. } => verb.clone(),
        _ => "compute".into(),
    }
}

/// The kind of a named step, for its caption when its formula is too long.
fn kind(step: &Step) -> String {
    match step {
        Step::Format { template } => shorten(template, CAPTION_MAX),
        Step::Pack { kind: Collection::Map, keys } if !keys.is_empty() => format!("{{{}}}", list(keys, CAPTION_MAX - 2)),
        Step::Map { .. } | Step::Pick { .. } | Step::Pack { .. } | Step::Calls { .. } => phrase(step),
        Step::Test { .. } => "test".into(),
        Step::Compute { .. } => "arithmetic".into(),
        Step::Choose { .. } => "choice".into(),
        _ => String::new(),
    }
}

/// Call names as a title: `view · transpose`.
pub fn calls(names: &[String]) -> String {
    let mut uniq: Vec<&str> = vec![];
    for n in names {
        if !uniq.contains(&n.as_str()) {
            uniq.push(n);
        }
    }
    match uniq.len() {
        0 => String::new(),
        1..=3 => uniq.join(" · "),
        n => format!("{} +{}", uniq[..2].join(" · "), n - 2),
    }
}

/// Names a step. `code` is its source shortened to about `CAPTION_MAX`
/// characters by eliding whole sub-expressions as `…`.
pub fn name(step: &Step, dest: &Dest, code: &str) -> Name {
    let whole = !code.contains('…');
    // A short value reads as part of the exit: `return None`, `return value`.
    if let Step::Exit { verb, value } = step {
        if value.is_empty() || fits(value, 14) && !value.contains('…') {
            return Name { title: format!("{verb} {value}").trim_end().to_string(), caption: String::new() };
        }
        return Name { title: verb.clone(), caption: value.clone() };
    }
    if let Step::Update { op, operand } = step {
        let title = match dest {
            Dest::Names(ns) => list(ns, NAMES_MAX),
            Dest::Field { key, .. } => key.clone(),
            Dest::Item { owner, .. } => owner.clone(),
            Dest::None => String::new(),
        };
        let index = match dest {
            Dest::Item { index, .. } => format!("[{index}] "),
            _ => String::new(),
        };
        let full = format!("{index}{op} {operand}");
        return Name { title, caption: if fits(&full, CAPTION_MAX) { full } else { op.clone() } };
    }
    match dest {
        Dest::None => {
            // Calls are named by the operation; other glue by its formula when it
            // fits, else by a phrase.
            if let Step::Calls { names, receiver, .. } = step {
                return Name { title: calls(names), caption: qualifier(receiver, CAPTION_MAX) };
            }
            let title = match step {
                Step::Format { .. } | Step::Pack { kind: Collection::Map, .. } => phrase(step),
                _ if whole && fits(code, TITLE_MAX) => code.to_string(),
                _ => phrase(step),
            };
            let caption = if matches!(step, Step::Format { .. }) { "text".to_string() } else { String::new() };
            Name { title, caption }
        }
        Dest::Names(ns) => {
            // A step that transforms a value in place is named by what it does
            // (`k = k.view(…).transpose(…)`); one that produces a new value, by
            // the developer's name for it (`userpass = b":".join(…)`).
            if let Step::Calls { names, reads, receiver } = step {
                if ns.iter().any(|n| reads.contains(n)) {
                    return Name { title: calls(names), caption: String::new() };
                }
                // A call on a module or object keeps its qualifier: `os.path.join`.
                let qualified = format!("{receiver}.{}", calls(names));
                let plain = receiver.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.');
                let caption = if names.len() == 1 && !receiver.is_empty() && plain && fits(&qualified, CAPTION_MAX) { qualified } else { calls(names) };
                return Name { title: list(ns, NAMES_MAX), caption };
            }
            // The caption shows how: the formula, with long parts elided.
            let informative = fits(code, CAPTION_MAX + 8) && !code.starts_with('…');
            let caption = match step {
                Step::Format { .. } | Step::Pack { .. } => kind(step),
                _ if informative => code.to_string(),
                _ => kind(step),
            };
            Name { title: list(ns, NAMES_MAX), caption }
        }
        Dest::Field { owner, key, item } => {
            let owner = qualifier(owner, 24);
            let caption = if owner.is_empty() { "set".to_string() } else { format!("set {} {owner}", if *item { "in" } else { "on" }) };
            Name { title: key.clone(), caption }
        }
        Dest::Item { owner, index } => {
            let at = if index.is_empty() { "set slice".to_string() } else { format!("set [{index}]") };
            Name { title: qualifier(owner, TITLE_MAX), caption: if fits(&at, CAPTION_MAX) { at } else { "set item".into() } }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn calls_keep_their_names_unless_they_make_a_new_value() {
        let chain = Step::Calls { names: s(&["view", "transpose"]), receiver: "k".into(), reads: s(&["k", "B", "T"]) };
        assert_eq!(name(&chain, &Dest::Names(s(&["k"])), "k.view(B, T).transpose(1, 2)").title, "view · transpose");
        let join = Step::Calls { names: s(&["join"]), receiver: "b\":\"".into(), reads: s(&["username", "password"]) };
        let n = name(&join, &Dest::Names(s(&["userpass"])), "b\":\".join((to_bytes(username), to_bytes(password)))");
        assert_eq!((n.title.as_str(), n.caption.as_str()), ("userpass", "join"));
        let path = Step::Calls { names: s(&["join"]), receiver: "os.path".into(), reads: s(&["os", "root", "name"]) };
        assert_eq!(name(&path, &Dest::Names(s(&["path"])), "os.path.join(root, name)").caption, "os.path.join");
        let n = name(&Step::Calls { names: s(&["authenticators"]), receiver: "self._netrc_info".into(), reads: s(&["self"]) }, &Dest::None, "");
        assert_eq!((n.title.as_str(), n.caption.as_str()), ("authenticators", "self._netrc_info"));
    }

    #[test]
    fn values_are_named_by_their_names() {
        let test = Step::Test { subjects: s(&["trust_env", "transport"]) };
        let n = name(&test, &Dest::Names(s(&["allow_env_proxies"])), "trust_env and transport is None");
        assert_eq!((n.title.as_str(), n.caption.as_str()), ("allow_env_proxies", "trust_env and transport is None"));
        let long = "url.scheme == \"http\" and …";
        let n = name(&Step::Test { subjects: s(&["url.scheme", "_port_or_default"]) }, &Dest::None, long);
        assert_eq!(n.title, "check url.scheme +1");
        assert_eq!(name(&Step::Compute { subjects: s(&["T"]) }, &Dest::None, "T + 1").title, "T + 1");
    }

    #[test]
    fn stores_name_the_field() {
        let n = name(&Step::Other, &Dest::Field { owner: "request.headers".into(), key: "Authorization".into(), item: true }, "");
        assert_eq!((n.title.as_str(), n.caption.as_str()), ("Authorization", "set in request.headers"));
        let n = name(&Step::Update { op: "+=".into(), operand: "1".into() }, &Dest::Names(s(&["base_idx"])), "");
        assert_eq!((n.title.as_str(), n.caption.as_str()), ("base_idx", "+= 1"));
    }

    #[test]
    fn templates_are_prose() {
        let n = name(&Step::Format { template: "Basic {token}".into() }, &Dest::None, "f\"Basic {token}\"");
        assert_eq!(n.title, "Basic {token}");
        assert_eq!(shorten("There are no blog posts tagged with {tag}", 28), "There are no blog posts…");
        assert!(!list(&s(&["alpha", "beta", "gamma", "delta", "epsilon"]), 16).contains('…'));
    }
}
