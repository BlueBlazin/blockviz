//! Docker Compose files as a diagram of services: what each one is built from
//! or runs (an image), what it depends on, and what a reverse proxy routes to.
//! Only the structure that matters for that is read, with a small
//! indentation-based reader rather than a full YAML parser.

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Service {
    pub name: String,
    pub image: String,
    /// Build context and Dockerfile, as written.
    pub context: Option<String>,
    pub dockerfile: Option<String>,
    pub depends_on: Vec<String>,
    /// Exposed through a reverse proxy (`traefik.enable=true`).
    pub routed: bool,
}

#[derive(Debug, Default)]
struct Y {
    key: String,
    value: String,
    list: Vec<String>,
    kids: Vec<Y>,
}

impl Y {
    fn kid(&self, key: &str) -> Option<&Y> {
        self.kids.iter().find(|k| k.key == key)
    }

    /// A key's values: a block list, a flow list `[a, b]`, or mapping keys.
    fn items(&self) -> Vec<String> {
        if !self.list.is_empty() {
            return self.list.clone();
        }
        if self.value.starts_with('[') {
            return self.value.trim_matches(['[', ']']).split(',').map(|v| unquote(v.trim())).filter(|v| !v.is_empty()).collect();
        }
        self.kids.iter().map(|k| if k.value.is_empty() { k.key.clone() } else { format!("{}={}", k.key, k.value) }).collect()
    }
}

fn unquote(s: &str) -> String {
    s.trim().trim_matches(['"', '\'']).to_string()
}

fn strip_comment(line: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut prev = ' ';
    for (i, c) in line.char_indices() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => {}
            None if c == '"' || c == '\'' => quote = Some(c),
            None if c == '#' && prev.is_whitespace() => return &line[..i],
            None => {}
        }
        prev = c;
    }
    line
}

fn block(lines: &[(usize, &str)], i: &mut usize, indent: usize) -> Vec<Y> {
    let mut out = vec![];
    while *i < lines.len() {
        let (ind, t) = lines[*i];
        if ind < indent {
            break;
        }
        if ind > indent || t.starts_with('-') {
            *i += 1;
            continue;
        }
        let (key, value) = match t.split_once(": ") {
            Some((k, v)) => (k, v),
            None => (t.trim_end_matches(':'), ""),
        };
        let mut node = Y { key: unquote(key), value: unquote(value), ..Y::default() };
        *i += 1;
        if *i < lines.len() && lines[*i].0 > indent {
            let ci = lines[*i].0;
            if lines[*i].1.starts_with('-') {
                while *i < lines.len() && lines[*i].0 >= ci {
                    if lines[*i].0 == ci {
                        if let Some(item) = lines[*i].1.strip_prefix('-') {
                            node.list.push(unquote(item));
                        }
                    }
                    *i += 1;
                }
            } else {
                node.kids = block(lines, i, ci);
            }
        }
        out.push(node);
    }
    out
}

pub fn services(text: &str) -> Vec<Service> {
    let lines: Vec<(usize, &str)> = text
        .lines()
        .map(strip_comment)
        .filter(|l| !l.trim().is_empty())
        .map(|l| (l.len() - l.trim_start().len(), l.trim().trim_end()))
        .collect();
    let mut i = 0;
    let top = block(&lines, &mut i, 0);
    let Some(services) = top.iter().find(|y| y.key == "services") else { return vec![] };
    services
        .kids
        .iter()
        .map(|s| {
            let build = s.kid("build");
            let (context, dockerfile) = match build {
                Some(b) if !b.value.is_empty() => (Some(b.value.clone()), None),
                Some(b) => (b.kid("context").map(|c| c.value.clone()), b.kid("dockerfile").map(|d| d.value.clone())),
                None => (None, None),
            };
            let labels = s.kid("labels").map(Y::items).unwrap_or_default();
            Service {
                name: s.key.clone(),
                image: s.kid("image").map(|x| x.value.clone()).unwrap_or_default(),
                context,
                dockerfile,
                depends_on: s.kid("depends_on").map(Y::items).unwrap_or_default().into_iter().map(|d| d.split('=').next().unwrap_or(&d).to_string()).collect(),
                routed: labels.iter().any(|l| l.replace(' ', "").replace(':', "=") == "traefik.enable=true"),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_services() {
        let text = "services:\n\n  proxy:\n    image: traefik:v3.7\n    command:\n      # comment\n      - --providers.docker\n\n  db:\n    image: postgres:18\n    healthcheck:\n      test: [\"CMD\", \"pg_isready\"]\n\n  backend:\n    image: backend:latest\n    depends_on:\n      db:\n        condition: service_healthy\n    build:\n      context: .\n      dockerfile: backend/Dockerfile\n    labels:\n      - traefik.enable=true\n\n  frontend:\n    build: ./frontend\n    depends_on: [backend]\n\nvolumes:\n  data:\n";
        let s = services(text);
        assert_eq!(s.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["proxy", "db", "backend", "frontend"]);
        assert_eq!(s[1].image, "postgres:18");
        assert_eq!(s[2].depends_on, ["db"]);
        assert_eq!(s[2].dockerfile.as_deref(), Some("backend/Dockerfile"));
        assert!(s[2].routed);
        assert_eq!(s[3].context.as_deref(), Some("./frontend"));
        assert_eq!(s[3].depends_on, ["backend"]);
    }
}
