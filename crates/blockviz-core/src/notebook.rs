//! Jupyter notebooks become Python modules: code cells in order, each
//! introduced by a comment taken from the markdown cell before it, so cells
//! turn into titled sections. IPython magics and shell escapes are dropped.

use serde_json::Value;

pub fn to_python(json: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json).ok()?;
    let cells = v.get("cells")?.as_array()?;
    let mut out = String::new();
    let mut pending: Option<String> = None;
    for cell in cells {
        let kind = cell.get("cell_type").and_then(|k| k.as_str()).unwrap_or("");
        let source = match cell.get("source") {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Array(lines)) => lines.iter().filter_map(|l| l.as_str()).collect::<String>(),
            _ => String::new(),
        };
        match kind {
            "markdown" => {
                let title = source
                    .lines()
                    .map(|l| l.trim().trim_start_matches('#').trim())
                    .find(|l| !l.is_empty())
                    .map(|l| l.replace(['*', '`', '[', ']'], ""));
                if title.is_some() {
                    pending = title;
                }
            }
            "code" => {
                let code: Vec<String> = source.lines().map(crate::python::clean_notebook_line).collect();
                if code.iter().all(|l| l.trim().is_empty()) {
                    continue;
                }
                out.push_str("\n\n");
                if let Some(t) = pending.take() {
                    out.push_str("# ");
                    out.push_str(&t.chars().take(120).collect::<String>());
                    out.push('\n');
                }
                for l in code {
                    out.push_str(&l);
                    out.push('\n');
                }
            }
            _ => {}
        }
    }
    Some(out.trim_start_matches('\n').to_string())
}

#[cfg(test)]
mod tests {
    #[test]
    fn cells_become_titled_paragraphs() {
        let nb = r###"{"cells":[{"cell_type":"markdown","source":["## params\n","First some text"]},{"cell_type":"code","source":["%matplotlib inline\n","x = 1\n"]}]}"###;
        let py = super::to_python(nb).unwrap();
        assert!(py.starts_with("# params\n"));
        assert!(!py.contains("%matplotlib"));
        assert!(py.contains("x = 1"));
    }
}
