//! Contracts and the types that implement them: Go interfaces, satisfied by
//! having their methods, and (declared) traits, interfaces and abstract
//! classes. A call through a value of a contract type runs one of the
//! implementations; when the project shows which, the call is that call.
//!
//! Go types satisfy interfaces without saying so, and a type can get its
//! methods from a library type it embeds (`sqlx.DB` from `*sql.DB`), so a
//! Go call is resolved from what the project puts where the interface is
//! expected (an argument, a field, a variable, a result, `var _ I = &T{}`),
//! as the compiler sees it, not from method names alone: a type that only
//! happens to have the methods (`echo.Context.Validate`) is not taken for
//! the validator a program registers.

use super::index::{docstring, ClassInfo, Index, Ty};
use super::syntax::{self as ast, Expr, Lang, ParamKind, Stmt};
use super::walk;
use crate::text::squash;
use std::collections::{HashMap, HashSet};

/// What a call through a contract may run.
pub struct Implementations {
    /// The contract that leaves the method to its implementations.
    pub contract: usize,
    /// The types it may be, in project order.
    pub types: Vec<usize>,
    /// Their methods, one per distinct function.
    pub funcs: Vec<usize>,
    /// Some type runs a method the project does not define (a Go type's
    /// method from a library type it embeds).
    pub open: bool,
    /// Some implementations are not known by name.
    pub partial: bool,
}

impl<'p> Index<'p> {
    pub(super) fn find_contracts(&mut self) {
        self.implementers = vec![vec![]; self.classes.len()];
        self.implements = vec![vec![]; self.classes.len()];
        self.go_needs = vec![None; self.classes.len()];
        self.unresolved = vec![false; self.classes.len()];
        self.flows = vec![vec![]; self.classes.len()];
        self.satisfy_go_interfaces();
        self.declared_implementations();
        if self.go_needs.iter().any(Option::is_some) {
            self.flows = self.go_flows();
        }
    }

    /// A Go type satisfies the project's interfaces whose methods it has,
    /// without saying so.
    fn satisfy_go_interfaces(&mut self) {
        let go = |c: &ClassInfo| self.modules[c.module].lang == Lang::Go;
        let contracts: Vec<(usize, HashSet<String>)> = (0..self.classes.len())
            .filter(|&k| self.classes[k].def.kind == ast::ClassKind::Interface && go(&self.classes[k]))
            .map(|k| (k, self.go_contract(k).into_iter().collect::<HashSet<String>>()))
            .filter(|(_, need)| !need.is_empty())
            .collect();
        if contracts.is_empty() {
            return;
        }
        for c in (0..self.classes.len()).filter(|&c| self.classes[c].def.kind == ast::ClassKind::Class && go(&self.classes[c])) {
            let have = self.go_method_set(c);
            for (k, need) in &contracts {
                if need.iter().all(|m| have.contains(m)) {
                    self.implementers[*k].push(c);
                    self.implements[c].push(*k);
                }
            }
        }
        for (k, need) in contracts {
            self.go_needs[k] = Some(need);
        }
    }

    /// What other languages say they implement: subclasses of a class
    /// (Python, JS/TS `extends`), traits a Rust type implements (in its own
    /// module or in an `impl Trait for Type` elsewhere).
    fn declared_implementations(&mut self) {
        let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
        for (k, c) in self.classes.iter().enumerate() {
            by_name.entry(c.name.as_str()).or_default().push(k);
        }
        // A base written with a contract's name that did not resolve to it
        // (an `impl` in an inline module whose `use` is not followed): some
        // implementation is not known, so none is taken for the only one.
        let mut unresolved: Vec<usize> = vec![];
        for c in &self.classes {
            for (b, text) in c.bases.iter().zip(&c.base_text) {
                if matches!(b, Ty::Class(_)) {
                    continue;
                }
                let head = text.split('<').next().unwrap_or(text).trim_start_matches(['&', ' ']).trim_start_matches("dyn ");
                let name = head.rsplit(['.', ':']).next().unwrap_or(head);
                unresolved.extend(by_name.get(name).into_iter().flatten().copied());
            }
        }
        for k in unresolved {
            self.unresolved[k] = true;
        }
        for c in 0..self.classes.len() {
            if self.modules[self.classes[c].module].lang == Lang::Go {
                continue;
            }
            let class_bases = |k: usize| self.classes[k].bases.iter().filter_map(|b| if let Ty::Class(p) = b { Some(*p) } else { None }).collect::<Vec<usize>>();
            let (implementer, mut stack) = match (self.classes[c].def.kind, self.classes[c].bases.first()) {
                (ast::ClassKind::Extension, Some(Ty::Class(t))) => (*t, class_bases(c).into_iter().skip(1).collect()),
                (ast::ClassKind::Extension, _) => continue,
                _ => (c, class_bases(c)),
            };
            if self.contract_kind(implementer) {
                continue;
            }
            let mut seen: Vec<usize> = vec![];
            while let Some(p) = stack.pop() {
                if p == implementer || seen.contains(&p) || seen.len() > 32 {
                    continue;
                }
                seen.push(p);
                if !self.implementers[p].contains(&implementer) {
                    self.implementers[p].push(implementer);
                }
                stack.extend(class_bases(p));
            }
        }
    }

    /// A Go method by name and signature: parameter and result types as
    /// written, without package qualifiers or pointers.
    fn go_signature(&self, name: &str, params: &[ast::Param], returns: Option<&Expr>, module: usize) -> String {
        let src = &self.modules[module].src;
        let norm = |e: &Expr| {
            let t = squash(src.of(e));
            // `interface{}` is `any`.
            t.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.' || c == '[' || c == ']')).filter(|w| !w.is_empty()).map(|w| w.rsplit('.').next().unwrap_or(w)).map(|w| if w == "interface" { "any" } else { w }).collect::<Vec<_>>().join(" ")
        };
        let params: Vec<String> = params.iter().map(|a| a.annotation.as_ref().map(norm).unwrap_or_default()).collect();
        // Results by type, named or not: `(peer ProtoGetter, ok bool)` is `(ProtoGetter, bool)`.
        let results = match returns {
            Some(Expr::Tuple(t)) => t.elts.iter().map(norm).collect::<Vec<_>>().join(", "),
            Some(e) => norm(e),
            None => String::new(),
        };
        format!("{name}({})->{results}", params.join(", "))
    }

    /// A Go interface's methods: the signatures in its body and in the
    /// interfaces it embeds.
    fn go_contract(&self, k: usize) -> Vec<String> {
        let mut out = vec![];
        let mut stack = vec![k];
        let mut seen = vec![];
        while let Some(k) = stack.pop() {
            if seen.contains(&k) || seen.len() > 16 {
                continue;
            }
            seen.push(k);
            let c = &self.classes[k];
            out.extend(c.def.body.iter().filter_map(|s| if let Stmt::FunctionDef(f) = s { Some(self.go_signature(&f.name, &f.params, f.returns.as_deref(), c.module)) } else { None }));
            stack.extend(c.bases.iter().filter_map(|b| if let Ty::Class(p) = b { Some(*p) } else { None }).filter(|&p| self.classes[p].def.kind == ast::ClassKind::Interface));
        }
        out
    }

    /// The signatures of a Go type's methods, its own and those of the types
    /// it embeds.
    fn go_method_set(&self, cid: usize) -> HashSet<String> {
        let mut have = HashSet::new();
        let mut stack = vec![cid];
        while let Some(k) = stack.pop() {
            let c = &self.classes[k];
            if c.def.kind == ast::ClassKind::Interface {
                have.extend(self.go_contract(k));
            } else {
                have.extend(c.methods.iter().chain(&c.extra).map(|(_, f)| {
                    let info = &self.funcs[*f];
                    self.go_signature(&info.name, info.def.args.get(1..).unwrap_or(&[]), info.def.returns, info.module)
                }));
            }
            stack.extend(c.bases.iter().filter_map(|b| if let Ty::Class(p) = b { Some(*p) } else { None }).filter(|p| *p != k).take(4));
            if stack.len() > 16 {
                break;
            }
        }
        have
    }

    /// What a call of `name` on a value of type `k` may run when `k` leaves
    /// it to the types implementing a contract (an interface or trait
    /// declaring it without a body, a class whose `name` is abstract, an
    /// interface a Go struct embeds). `None` when the call is not through a
    /// contract.
    ///
    /// Outside tests, test doubles do not count; and a lone declared
    /// implementation the project never creates or names is not taken for
    /// what runs (a library's sample of what its users write).
    pub fn implementations(&self, k: usize, name: &str, from: usize) -> Option<Implementations> {
        let contract = self.contract_for(k, name, 0)?;
        let tests = crate::project::is_test_path(&self.modules[from].path);
        let go = self.modules[self.classes[contract].module].lang == Lang::Go;
        let candidates = if go { self.go_candidates(contract, tests) } else { self.implementers[contract].iter().copied().filter(|&c| tests || !self.is_double(c)).collect() };
        let mut out = Implementations { contract, types: vec![], funcs: vec![], open: false, partial: false };
        for c in candidates {
            match self.method(c, name).filter(|&f| !self.is_abstract(f) && self.funcs[f].class.is_some_and(|o| !self.contract_kind(o))) {
                Some(f) => {
                    if !out.funcs.contains(&f) {
                        out.funcs.push(f);
                    }
                }
                // Go: from a library type or an interface it embeds.
                None if go => out.open = true,
                // Declared: an implementation that is abstract itself.
                None => continue,
            }
            out.types.push(c);
        }
        if !go && !tests && out.types.len() == 1 && !self.named(out.types[0]) {
            out.types.clear();
            out.funcs.clear();
        }
        if !go && self.unresolved[contract] && !out.types.is_empty() {
            out.open = true;
            out.partial = true;
        }
        Some(out)
    }

    /// Go: the types the project puts where the contract (or one that
    /// includes it) is expected, then those that satisfy it and that the
    /// project uses, though not seen going in. Nothing when nothing goes in:
    /// the value comes from elsewhere (a library's caller).
    fn go_candidates(&self, k: usize, tests: bool) -> Vec<usize> {
        let Some(need) = &self.go_needs[k] else { return vec![] };
        let mut out: Vec<usize> = vec![];
        for (j, flows) in self.flows.iter().enumerate().filter(|(_, f)| !f.is_empty()) {
            if j != k && !self.go_needs[j].as_ref().is_some_and(|nj| need.is_subset(nj)) {
                continue;
            }
            for &(t, in_test) in flows {
                if (tests || !in_test && !self.is_double(t)) && !out.contains(&t) {
                    out.push(t);
                }
            }
        }
        if out.is_empty() {
            return out;
        }
        for &c in &self.implementers[k] {
            if !out.contains(&c) && (tests || !self.is_double(c)) && self.named(c) {
                out.push(c);
            }
        }
        out.sort_unstable();
        out
    }

    /// The contract that leaves `name` to its implementations when it is
    /// called on a value of type `k`: `k` itself, or an interface `k` embeds.
    fn contract_for(&self, k: usize, name: &str, depth: u32) -> Option<usize> {
        if depth > 8 {
            return None;
        }
        match self.method(k, name) {
            Some(f) => self.is_abstract(f).then_some(k),
            None if self.declares(k, name, depth) => Some(k),
            None => self.classes[k].bases.iter().find_map(|b| match b {
                Ty::Class(p) if *p != k => self.contract_for(*p, name, depth + 1),
                _ => None,
            }),
        }
    }

    /// Interfaces and traits: types that are only a contract.
    pub fn contract_kind(&self, k: usize) -> bool {
        matches!(self.classes[k].def.kind, ast::ClassKind::Interface | ast::ClassKind::Trait)
    }

    /// Whether interface or trait `k` declares `name` without a body, itself
    /// or through those it extends.
    fn declares(&self, k: usize, name: &str, depth: u32) -> bool {
        if !self.contract_kind(k) || depth > 8 {
            return false;
        }
        let c = &self.classes[k];
        c.def.body.iter().any(|s| match s {
            Stmt::FunctionDef(f) => f.name == name && f.body.is_empty(),
            Stmt::AnnAssign(a) => a.value.is_none() && matches!(&*a.target, Expr::Name(n) if n.id == name),
            _ => false,
        }) || c.bases.iter().any(|b| matches!(b, Ty::Class(p) if *p != k && self.declares(*p, name, depth + 1)))
    }

    /// A method with nothing of its own to run: `@abstractmethod`, or a body
    /// that only raises NotImplementedError or is `...`.
    pub fn is_abstract(&self, f: usize) -> bool {
        let d = &self.funcs[f].def;
        if d.decorated("abstractmethod") || d.decorated("abstractproperty") {
            return true;
        }
        let body = if docstring(d.body).is_some() { &d.body[1..] } else { d.body };
        let not_implemented = |e: &Expr| match e {
            Expr::Name(n) => n.id == "NotImplementedError",
            Expr::Call(c) => matches!(&*c.func, Expr::Name(n) if n.id == "NotImplementedError"),
            _ => false,
        };
        match body {
            [Stmt::Raise(r)] => r.exc.as_deref().is_some_and(not_implemented),
            [Stmt::Expr(e)] => matches!(&*e.value, Expr::EllipsisLiteral(_)),
            _ => false,
        }
    }

    /// A method that only hands back what its value holds: one `return` of
    /// a field, a constant or arithmetic on them, no calls (`Width()`
    /// returning `w.width`). Called, it reads like the field.
    pub fn is_accessor(&self, f: usize) -> bool {
        let d = &self.funcs[f].def;
        let body = if docstring(d.body).is_some() { &d.body[1..] } else { d.body };
        let [Stmt::Return(r)] = body else { return false };
        let mut plain = true;
        if let Some(v) = &r.value {
            walk::walk(v, &mut |e| {
                if matches!(e, Expr::Call(_) | Expr::Function(_) | Expr::Lambda(_) | Expr::Await(_) | Expr::Yield(_) | Expr::Comp(_)) {
                    plain = false;
                }
            });
        }
        plain
    }

    /// A type standing in for a real one in tests: declared in test code or
    /// a test helper (`mocks/`, `testutil.rs`), or named as a double
    /// (`MockStore`, `fakeClock`).
    fn is_double(&self, c: usize) -> bool {
        let path = &self.modules[self.classes[c].module].path;
        let helper = |p: &str| {
            let stem = p.split('.').next().unwrap_or(p);
            matches!(stem, "mock" | "mocks" | "fake" | "fakes" | "testutil" | "testutils" | "test_util" | "test_utils" | "testhelpers" | "test_helpers")
        };
        crate::project::is_test_path(path) || path.split('/').any(helper) || crate::naming::is_test_double(&self.classes[c].name)
    }

    /// Whether non-test code names class `c` outside its own definition and
    /// methods: creates it, converts to it, declares something of its type.
    fn named(&self, c: usize) -> bool {
        self.named.get_or_init(|| self.find_named())[c]
    }

    fn find_named(&self) -> Vec<bool> {
        let mut by_name: HashMap<&str, Vec<usize>> = HashMap::new();
        for (k, c) in self.classes.iter().enumerate() {
            by_name.entry(c.name.as_str()).or_default().push(k);
        }
        let own = |k: usize, mi: usize, at: ast::TextSize| {
            let c = &self.classes[k];
            (c.module == mi && c.def.range.contains_inclusive(at)) || c.methods.iter().chain(&c.extra).any(|(_, f)| self.funcs[*f].module == mi && self.funcs[*f].def.range.contains_inclusive(at))
        };
        let mut named = vec![false; self.classes.len()];
        for (mi, m) in self.modules.iter().enumerate().filter(|(_, m)| !crate::project::is_test_path(&m.path)) {
            each_name(&m.ast, &mut |id, at| {
                for &k in by_name.get(id).into_iter().flatten() {
                    if !named[k] && !own(k, mi, at) {
                        named[k] = true;
                    }
                }
            });
        }
        named
    }

    /// Whether Go type `t` can be an interface `k` value: it has `k`'s
    /// methods (by name: signatures written differently still match), or it
    /// embeds something whose methods are not all known (a library type, an
    /// interface). Otherwise the type was inferred wrongly (a type assertion
    /// read as the value it asserts on).
    fn plausible(&self, k: usize, t: usize) -> bool {
        if self.implementers[k].contains(&t) {
            return true;
        }
        let names = |sigs: &mut dyn Iterator<Item = &String>| -> HashSet<String> { sigs.map(|s| s.split('(').next().unwrap_or(s).to_string()).collect() };
        let have = self.go_method_set(t);
        let have = names(&mut have.iter());
        let need = self.go_needs[k].as_ref().map(|n| names(&mut n.iter())).unwrap_or_default();
        need.is_subset(&have) || self.go_opaque(t, 0)
    }

    /// A Go type with methods from something it embeds that the project does
    /// not define: a library type (locks aside) or an interface.
    fn go_opaque(&self, t: usize, depth: u32) -> bool {
        let c = &self.classes[t];
        depth < 4
            && c.bases.iter().zip(&c.base_text).any(|(b, text)| match b {
                Ty::Class(p) if *p != t => self.contract_kind(*p) || self.go_opaque(*p, depth + 1),
                Ty::Class(_) => false,
                _ => !(text.starts_with("sync.") || text.starts_with("atomic.")),
            })
    }

    /// Where Go code puts a value of a project type in a slot of a project
    /// interface type, by interface: (type, in test code).
    fn go_flows(&self) -> Vec<Vec<(usize, bool)>> {
        let mut scan = Flows { ix: self, mi: 0, test: false, out: vec![vec![]; self.classes.len()] };
        for (mi, m) in self.modules.iter().enumerate().filter(|(_, m)| m.lang == Lang::Go) {
            scan.mi = mi;
            scan.test = crate::project::is_test_path(&m.path);
            scan.module(&m.ast);
        }
        scan.out
    }
}

/// Every name and attribute in a body, type annotations included, with
/// where it is.
fn each_name<'a>(stmts: &'a [Stmt], f: &mut dyn FnMut(&'a str, ast::TextSize)) {
    let mut roots: Vec<&'a Expr> = vec![];
    walk::stmt_exprs(stmts, true, &mut |e| roots.push(e));
    annotations(stmts, &mut roots);
    let mut bodies: Vec<&'a [Stmt]> = vec![];
    while let Some(e) = roots.pop() {
        let mut more: Vec<&'a Expr> = vec![];
        walk::walk(e, &mut |x| match x {
            Expr::Name(n) => f(&n.id, n.range.start()),
            Expr::Attribute(a) => f(&a.attr, a.range.start()),
            Expr::Function(func) => {
                more.extend(func.params.iter().filter_map(|p| p.annotation.as_ref()));
                more.extend(func.returns.as_deref());
                bodies.push(&func.body);
            }
            _ => {}
        });
        roots.extend(more);
    }
    for b in bodies {
        each_name(b, f);
    }
}

/// The type annotations of a body's definitions and declarations.
fn annotations<'a>(stmts: &'a [Stmt], out: &mut Vec<&'a Expr>) {
    for s in stmts {
        match s {
            Stmt::FunctionDef(f) => {
                out.extend(f.params.iter().filter_map(|p| p.annotation.as_ref()));
                out.extend(f.returns.as_deref());
                annotations(&f.body, out);
            }
            Stmt::ClassDef(c) => annotations(&c.body, out),
            Stmt::AnnAssign(a) => out.push(&a.annotation),
            Stmt::If(x) => {
                annotations(&x.body, out);
                for c in &x.elif_else_clauses {
                    annotations(&c.body, out);
                }
            }
            Stmt::For(x) => {
                annotations(&x.body, out);
                annotations(&x.orelse, out);
            }
            Stmt::While(x) => {
                annotations(&x.body, out);
                annotations(&x.orelse, out);
            }
            Stmt::With(x) => annotations(&x.body, out),
            Stmt::Match(x) => {
                for c in &x.cases {
                    annotations(&c.body, out);
                }
            }
            Stmt::Try(x) => {
                annotations(&x.body, out);
                for h in &x.handlers {
                    annotations(&h.body, out);
                }
                annotations(&x.orelse, out);
                annotations(&x.finalbody, out);
            }
            _ => {}
        }
    }
}

/// Go code putting values where a project interface is expected.
struct Flows<'i, 'p> {
    ix: &'i Index<'p>,
    mi: usize,
    test: bool,
    out: Vec<Vec<(usize, bool)>>,
}

type Locals = HashMap<String, Ty>;

impl Flows<'_, '_> {
    /// A value of type `value` goes into a slot of type `slot`.
    fn put(&mut self, slot: &Ty, value: &Ty) {
        let (Ty::Inst(k), Ty::Inst(t)) = (slot, value) else { return };
        if self.ix.go_needs[*k].is_some() && !self.ix.contract_kind(*t) && !self.out[*k].contains(&(*t, self.test)) && self.ix.plausible(*k, *t) {
            self.out[*k].push((*t, self.test));
        }
    }

    fn ty(&self, e: &Expr, locals: &Locals) -> Ty {
        match self.ix.infer(self.mi, e, locals) {
            // `p.getters[key]`: an element of a field declared `map[string]*httpGetter`.
            Ty::Unknown => match e {
                Expr::Subscript(s) => self.element(&s.value, locals),
                _ => Ty::Unknown,
            },
            t => t,
        }
    }

    /// The type of the elements of a container field, from its declaration.
    fn element(&self, container: &Expr, locals: &Locals) -> Ty {
        if let Ty::List(t, _) = self.ix.infer(self.mi, container, locals) {
            return *t;
        }
        let Expr::Attribute(a) = container else { return Ty::Unknown };
        let Ty::Inst(c) = self.ix.infer(self.mi, &a.value, locals) else { return Ty::Unknown };
        let declared = self.ix.classes[c].def.body.iter().find_map(|s| match s {
            Stmt::AnnAssign(x) if matches!(&*x.target, Expr::Attribute(t) if t.attr == a.attr) => Some(&*x.annotation),
            _ => None,
        });
        let element = match declared {
            Some(Expr::Subscript(s)) => match (&*s.value, &*s.slice) {
                (Expr::Name(n), Expr::Tuple(kv)) if n.id == "map" => kv.elts.get(1),
                (Expr::Name(n), e) if n.id == "Vec" || n.id == "chan" => Some(e),
                _ => None,
            },
            _ => None,
        };
        element.map(|e| self.ix.annotated(self.ix.classes[c].module, e, &Locals::new())).unwrap_or(Ty::Unknown)
    }

    fn module(&mut self, body: &[Stmt]) {
        let mut locals = Locals::new();
        for s in body {
            // `var _ Router = (*Mux)(nil)`: the compiler is told Mux is a Router.
            if let Stmt::AnnAssign(a) = s {
                if matches!(&*a.target, Expr::Name(n) if n.id == "_") {
                    let slot = self.ix.annotated(self.mi, &a.annotation, &locals);
                    let text = self.ix.modules[self.mi].src.of(a);
                    if let Some(t) = text.split_once('=').and_then(|(_, v)| v.trim().strip_prefix("(*")).and_then(|v| v.split_once(')')).map(|(t, _)| t.trim()) {
                        let ty = match t.split_once('.') {
                            Some((pkg, name)) => match self.ix.sym_ty(self.mi, pkg, 0) {
                                Ty::Module(m) => self.ix.module_member(m, name),
                                _ => Ty::Unknown,
                            },
                            None => self.ix.sym_ty(self.mi, t, 0),
                        };
                        if let Ty::Class(c) = ty {
                            self.put(&slot, &Ty::Inst(c));
                        }
                    }
                }
            }
            self.stmt(s, &mut locals, &[]);
        }
    }

    fn function(&mut self, params: &[ast::Param], returns: Option<&Expr>, body: &[Stmt], outer: &Locals) {
        let mut locals = outer.clone();
        for p in params {
            if let Some(a) = &p.annotation {
                let t = self.ix.annotated(self.mi, a, &locals);
                if t != Ty::Unknown {
                    locals.insert(p.name.clone(), t);
                }
            }
        }
        let results: Vec<Ty> = match returns {
            Some(Expr::Tuple(t)) => t.elts.iter().map(|e| self.ix.annotated(self.mi, e, &locals)).collect(),
            Some(e) => vec![self.ix.annotated(self.mi, e, &locals)],
            None => vec![],
        };
        self.block(body, &mut locals, &results);
    }

    fn block(&mut self, body: &[Stmt], locals: &mut Locals, results: &[Ty]) {
        for s in body {
            self.stmt(s, locals, results);
        }
    }

    fn stmt(&mut self, s: &Stmt, locals: &mut Locals, results: &[Ty]) {
        match s {
            Stmt::Assign(a) => {
                self.exprs(&a.value, locals);
                let value = self.ty(&a.value, locals);
                for t in &a.targets {
                    self.assign(t, &value, locals);
                }
            }
            Stmt::AnnAssign(a) => {
                let slot = self.ix.annotated(self.mi, &a.annotation, locals);
                if let Some(v) = &a.value {
                    self.exprs(v, locals);
                    let value = self.ty(v, locals);
                    self.put(&slot, &value);
                }
                if let (Expr::Name(n), false) = (&*a.target, slot == Ty::Unknown) {
                    locals.insert(n.id.clone(), slot);
                }
            }
            Stmt::Return(r) => {
                let Some(v) = &r.value else { return };
                self.exprs(v, locals);
                let values: Vec<&Expr> = match &**v {
                    Expr::Tuple(t) => t.elts.iter().collect(),
                    v => vec![v],
                };
                for (e, slot) in values.into_iter().zip(results) {
                    let ty = self.ty(e, locals);
                    self.put(slot, &ty);
                }
            }
            Stmt::FunctionDef(f) => self.function(&f.params, f.returns.as_deref(), &f.body, locals),
            Stmt::ClassDef(c) => {
                for s in &c.body {
                    if let Stmt::FunctionDef(f) = s {
                        self.function(&f.params, f.returns.as_deref(), &f.body, locals);
                    }
                }
            }
            Stmt::If(x) => {
                self.exprs(&x.test, locals);
                self.block(&x.body, locals, results);
                for c in &x.elif_else_clauses {
                    if let Some(t) = &c.test {
                        self.exprs(t, locals);
                    }
                    self.block(&c.body, locals, results);
                }
            }
            Stmt::For(x) => {
                self.exprs(&x.iter, locals);
                // `for _, h := range s.handlers`: the value is an element.
                if let Expr::Tuple(t) = &*x.target {
                    if let [_, Expr::Name(v)] = &t.elts[..] {
                        let element = self.element(&x.iter, locals);
                        if element != Ty::Unknown {
                            locals.insert(v.id.clone(), element);
                        }
                    }
                }
                self.block(&x.body, locals, results);
                self.block(&x.orelse, locals, results);
            }
            Stmt::While(x) => {
                self.exprs(&x.test, locals);
                self.block(&x.body, locals, results);
                self.block(&x.orelse, locals, results);
            }
            Stmt::With(x) => {
                for i in &x.items {
                    self.exprs(&i.context_expr, locals);
                }
                self.block(&x.body, locals, results);
            }
            Stmt::Match(x) => {
                self.exprs(&x.subject, locals);
                for c in &x.cases {
                    self.block(&c.body, locals, results);
                }
            }
            Stmt::Try(x) => {
                self.block(&x.body, locals, results);
                for h in &x.handlers {
                    self.block(&h.body, locals, results);
                }
                self.block(&x.orelse, locals, results);
                self.block(&x.finalbody, locals, results);
            }
            s => {
                let mut roots = vec![];
                walk::stmt_expr(s, false, &mut |e| roots.push(e));
                for e in roots {
                    self.exprs(e, locals);
                }
            }
        }
    }

    fn assign(&mut self, target: &Expr, value: &Ty, locals: &mut Locals) {
        match target {
            Expr::Name(n) => {
                // A variable declared of an interface type (`var s Store`, a
                // parameter) keeps it.
                if let Some(slot @ Ty::Inst(k)) = locals.get(&n.id) {
                    if self.ix.go_needs[*k].is_some() {
                        let slot = slot.clone();
                        self.put(&slot, value);
                        return;
                    }
                }
                // One that takes an interface value from a call is not a slot:
                // `conf := …` in a loop below it is another variable.
                let interface = matches!(value, Ty::Inst(k) if self.ix.go_needs[*k].is_some());
                if *value != Ty::Unknown && !interface {
                    locals.insert(n.id.clone(), value.clone());
                }
            }
            // `x, err := open()`: the value is the first result.
            Expr::Tuple(t) => {
                if let Some(first) = t.elts.first() {
                    self.assign(first, value, locals);
                }
            }
            // `hs[i] = h`: an element of a list or map of an interface type.
            Expr::Subscript(_) => {
                let slot = self.ty(target, locals);
                self.put(&slot, value);
            }
            // `s.store = pg`: a field of an interface type.
            Expr::Attribute(a) => {
                if let Ty::Inst(c) = self.ty(&a.value, locals) {
                    if let Some(slot) = self.ix.field(c, &a.attr).map(|f| f.ty.clone()) {
                        self.put(&slot, value);
                    }
                }
            }
            _ => {}
        }
    }

    /// Calls in an expression, then the function literals in it.
    fn exprs(&mut self, e: &Expr, locals: &Locals) {
        let mut calls = vec![];
        let mut funcs = vec![];
        walk::walk(e, &mut |x| match x {
            Expr::Call(c) => calls.push(c),
            Expr::Function(f) => funcs.push(f),
            _ => {}
        });
        for c in calls {
            self.call(c, locals);
        }
        for f in funcs {
            self.function(&f.params, f.returns.as_deref(), &f.body, locals);
        }
    }

    /// Arguments go into parameters; a struct literal fills its fields.
    fn call(&mut self, c: &ast::ExprCall, locals: &Locals) {
        let (callee, method) = match &*c.func {
            Expr::Name(n) => (self.ix.sym_ty(self.mi, &n.id, 0), false),
            Expr::Attribute(a) => match self.ty(&a.value, locals) {
                // `hs = append(hs, h)`, lowered `hs.append(h)`.
                Ty::List(element, _) if a.attr == "append" => {
                    for arg in &c.arguments.args {
                        let value = self.ty(arg, locals);
                        self.put(&element, &value);
                    }
                    return;
                }
                Ty::Module(m) => (self.ix.module_member(m, &a.attr), false),
                Ty::Inst(k) => (self.ix.method(k, &a.attr).map(Ty::Func).unwrap_or(Ty::Unknown), true),
                _ => (Ty::Unknown, false),
            },
            _ => (Ty::Unknown, false),
        };
        match callee {
            Ty::Class(k) => {
                for kw in &c.arguments.keywords {
                    let Some(key) = &kw.arg else { continue };
                    if let Some(slot) = self.ix.field(k, key).map(|f| f.ty.clone()) {
                        let value = self.ty(&kw.value, locals);
                        self.put(&slot, &value);
                    }
                }
            }
            Ty::Func(f) => {
                let params = self.ix.funcs[f].def.args;
                // A method's receiver is its first parameter.
                let skip = usize::from(method && self.ix.funcs[f].class.is_some());
                for (i, arg) in c.arguments.args.iter().enumerate() {
                    if matches!(arg, Expr::Starred(_)) {
                        break;
                    }
                    let p = params.get(i + skip).or_else(|| params.last().filter(|p| p.kind == ParamKind::Variadic));
                    if let Some(a) = p.and_then(|p| p.annotation.as_ref()) {
                        let slot = self.ix.annotated(self.mi, a, locals);
                        let value = self.ty(arg, locals);
                        self.put(&slot, &value);
                    }
                }
            }
            _ => {}
        }
    }
}
