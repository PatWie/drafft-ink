//! The fence checker: reads Rust source with `syn` and reports every expression that can change the state of a
//! session, a tab, a document or a store.
//!
//! It is a syntax-level check, not a type checker, so it works from what the source says:
//!
//! * a name is **state** when it is a parameter whose type names a state type (`&mut Session`), a field that a
//!   struct of the scanned crates declares with such a type (`App::session`), a `let` that is initialised from a
//!   state expression, or the result of a call that hands out state (`Session::tab_mut`, derived from the engine);
//! * a **finding** is a method on a state expression that takes `&mut self` in the engine or is a collection
//!   mutator, an assignment to a place inside a state expression, a `&mut` borrow of a part of one, a state passed
//!   mutably to a function the scanned crates do not define, a `Type::method(&mut ..)` path call on a state type,
//!   or a macro that holds a state name and is not known to be pure.
//!
//! `execute`, `execute_from`, `execute_typed` and `execute_batch` are the commands and are never findings.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use syn::visit::{self, Visit};
use syn::{BinOp, Expr, FnArg, ImplItem, Item, Pat, ReturnType, Type};

/// The types whose values are the session, a tab, a document or a store. A rename must update this list; the test
/// `the_state_types_exist_in_the_engine` fails when a name is no longer declared there.
pub const STATE_TYPES: &[&str] = &[
    "Session",
    "Tab",
    "TabDoc",
    "WorkStore",
    "DocStore",
    "MemoryStore",
    "LoroStore",
    "ToolState",
    "Settings",
    "Ports",
    "Extensions",
    "Observed",
];

/// Traits that stand for a session: a parameter bounded by one (or `impl Trait`) is state.
pub const STATE_TRAITS: &[&str] = &["CommandSink"];

/// The ways to run a command. Never a finding.
pub const COMMANDS: &[&str] = &["execute", "execute_from", "execute_typed", "execute_batch"];

/// Methods of collections and options that change their receiver.
const COLLECTION_MUTATORS: &[&str] = &[
    "push",
    "push_str",
    "push_back",
    "push_front",
    "pop",
    "pop_back",
    "pop_front",
    "insert",
    "remove",
    "swap_remove",
    "truncate",
    "clear",
    "retain",
    "retain_mut",
    "drain",
    "extend",
    "append",
    "swap",
    "sort",
    "sort_by",
    "sort_by_key",
    "sort_unstable",
    "reverse",
    "dedup",
    "resize",
    "split_off",
    "rotate_left",
    "rotate_right",
    "fill",
    "take",
    "replace",
    "get_or_insert",
    "get_or_insert_with",
    "set",
    "iter_mut",
    "get_mut",
    "first_mut",
    "last_mut",
    "entry",
    "values_mut",
    "as_mut",
    "as_deref_mut",
    "borrow_mut",
    "make_mut",
];

/// Calls that pass a state expression through (`state.unwrap()` is still the state).
const PASS_THROUGH: &[&str] =
    &["unwrap", "expect", "as_mut", "as_ref", "as_deref", "as_deref_mut", "borrow_mut", "lock"];

/// Macros whose arguments are plain expressions that are only read, or that cannot touch a place.
const PURE_MACROS: &[&str] = &[
    "format",
    "format_args",
    "println",
    "eprintln",
    "print",
    "eprint",
    "write",
    "writeln",
    "vec",
    "json",
    "matches",
    "assert",
    "assert_eq",
    "assert_ne",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_ne",
    "unreachable",
    "stringify",
    "concat",
    "env",
    "cfg",
    "include_str",
    "include_bytes",
    "line",
    "file",
    "column",
    "module_path",
    "log",
    "trace",
    "debug",
    "info",
    "warn",
    "error",
    "compile_error",
    "thread_local",
    "dbg",
];

/// What a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// A method on a state expression.
    Method,
    /// `Type::method(..)` on a state type.
    PathCall,
    /// An assignment (or compound assignment) to a place inside a state expression.
    Assign,
    /// `&mut` of a place inside a state expression.
    MutBorrow,
    /// A state expression handed mutably to a function that the scanned crates do not define.
    HandedOut,
    /// A macro that holds a state name and is not known to be pure.
    Macro,
}

impl Kind {
    /// The word in the allowlist file.
    pub const fn word(self) -> &'static str {
        match self {
            Self::Method => "call",
            Self::PathCall => "path-call",
            Self::Assign => "assign",
            Self::MutBorrow => "mut-borrow",
            Self::HandedOut => "handed-out",
            Self::Macro => "macro",
        }
    }
}

/// One place where the source can change state.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Finding {
    /// The file, relative to `crates/`.
    pub file: String,
    /// What was found.
    pub kind: Kind,
    /// The method, function, field or macro name.
    pub name: String,
    /// The function it is in (`Type::method` or `method`).
    pub in_fn: String,
    /// True when that function (or an item around it) is behind `#[cfg(feature = "scenarios")]`.
    pub scenarios_only: bool,
}

/// What the engine says about state: which methods change it and which hand it out.
#[derive(Debug, Default, Clone)]
pub struct EngineFacts {
    /// Methods with `&mut self` on a state type.
    pub mutators: BTreeSet<String>,
    /// Methods on a state type that return a state type (`tab`, `tab_mut`, `store`).
    pub getters: BTreeSet<String>,
    /// The state types the engine declares.
    pub declared: BTreeSet<String>,
}

/// Reads the sources that declare the state types (the engine, and the crates under it that hold the stores).
pub fn engine_facts(dirs: &[PathBuf]) -> EngineFacts {
    let mut facts = EngineFacts::default();
    for file in dirs.iter().flat_map(|d| rust_files(d)) {
        let Ok(text) = fs::read_to_string(&file) else { continue };
        let Ok(ast) = syn::parse_file(&text) else { continue };
        collect_engine(&ast.items, &mut facts);
    }
    facts
}

fn collect_engine(items: &[Item], facts: &mut EngineFacts) {
    for item in items {
        match item {
            Item::Struct(s) if STATE_TYPES.contains(&s.ident.to_string().as_str()) => {
                facts.declared.insert(s.ident.to_string());
            }
            Item::Type(t) if STATE_TYPES.contains(&t.ident.to_string().as_str()) => {
                facts.declared.insert(t.ident.to_string());
            }
            Item::Impl(imp) => {
                let Some(name) = type_name(&imp.self_ty) else { continue };
                if !STATE_TYPES.contains(&name.as_str()) {
                    continue;
                }
                for it in &imp.items {
                    let ImplItem::Fn(f) = it else { continue };
                    let method = f.sig.ident.to_string();
                    if matches!(f.sig.inputs.first(), Some(FnArg::Receiver(r)) if r.mutability.is_some()) {
                        facts.mutators.insert(method.clone());
                    }
                    if let ReturnType::Type(_, t) = &f.sig.output
                        && names_state_type(t)
                    {
                        facts.getters.insert(method);
                    }
                }
            }
            Item::Mod(m) => {
                if let Some((_, inner)) = &m.content {
                    collect_engine(inner, facts);
                }
            }
            _ => {}
        }
    }
}

/// Every `.rs` file under `dir`, sorted.
pub fn rust_files(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out.sort();
    out
}

fn type_name(t: &Type) -> Option<String> {
    match t {
        Type::Path(p) => p.path.segments.last().map(|s| s.ident.to_string()),
        Type::Reference(r) => type_name(&r.elem),
        _ => None,
    }
}

/// True when the type mentions a state type anywhere (`&mut Session`, `Option<&Tab>`, `Vec<Tab>`).
fn names_state_type(t: &Type) -> bool {
    struct Find(bool);
    impl<'ast> Visit<'ast> for Find {
        fn visit_path_segment(&mut self, s: &'ast syn::PathSegment) {
            if STATE_TYPES.contains(&s.ident.to_string().as_str()) {
                self.0 = true;
            }
            visit::visit_path_segment(self, s);
        }
    }
    let mut f = Find(false);
    f.visit_type(t);
    f.0
}

/// True when a parameter of type `ty` in `sig` is a state: it names a state type, or it is a generic or `impl`
/// parameter bounded by a state trait (`S: CommandSink`).
fn param_is_state(sig: &syn::Signature, ty: &Type) -> bool {
    if names_state_type(ty) {
        return true;
    }
    let bounded_by_trait = |bounds: &syn::punctuated::Punctuated<syn::TypeParamBound, syn::Token![+]>| {
        bounds.iter().any(|b| {
            matches!(b, syn::TypeParamBound::Trait(t)
                if t.path.segments.last().is_some_and(|s| STATE_TRAITS.contains(&s.ident.to_string().as_str())))
        })
    };
    let mut inner = ty;
    while let Type::Reference(r) = inner {
        inner = &r.elem;
    }
    match inner {
        Type::ImplTrait(i) => bounded_by_trait(&i.bounds),
        Type::Path(p) => {
            let Some(id) = p.path.get_ident() else { return false };
            sig.generics
                .params
                .iter()
                .any(|g| matches!(g, syn::GenericParam::Type(t) if t.ident == *id && bounded_by_trait(&t.bounds)))
                || sig.generics.where_clause.as_ref().is_some_and(|w| {
                    w.predicates.iter().any(|p| match p {
                        syn::WherePredicate::Type(pt) => {
                            matches!(&pt.bounded_ty, Type::Path(b) if b.path.is_ident(id))
                                && bounded_by_trait(&pt.bounds)
                        }
                        _ => false,
                    })
                })
        }
        _ => false,
    }
}

fn is_mut_ref(t: &Type) -> bool {
    matches!(t, Type::Reference(r) if r.mutability.is_some())
}

/// What the scanned crates declare, gathered before the walk so that the order of files does not matter.
#[derive(Debug, Default)]
pub struct Universe {
    /// Struct fields declared with a state type.
    state_fields: BTreeSet<String>,
    /// Functions and methods of the scanned crates that hand out state (`App::session_mut`).
    state_fns: BTreeSet<String>,
    /// Functions and methods of the scanned crates that take a state (a parameter, or `self` of a state type). They
    /// are scanned themselves, so handing a state to one of them is not a finding.
    takers: BTreeSet<String>,
}

impl Universe {
    /// Reads `(file name, source)` pairs.
    pub fn of(sources: &[(String, String)]) -> Self {
        let mut u = Self::default();
        for (_, src) in sources {
            if let Ok(ast) = syn::parse_file(src) {
                u.collect(&ast.items);
            }
        }
        u
    }

    fn collect(&mut self, items: &[Item]) {
        for item in items {
            match item {
                Item::Struct(s) => {
                    for f in &s.fields {
                        if let Some(id) = &f.ident
                            && names_state_type(&f.ty)
                        {
                            self.state_fields.insert(id.to_string());
                        }
                    }
                }
                Item::Fn(f) => self.function(&f.sig, false),
                Item::Impl(imp) => {
                    let on_state = type_name(&imp.self_ty).is_some_and(|n| STATE_TYPES.contains(&n.as_str()));
                    for it in &imp.items {
                        if let ImplItem::Fn(f) = it {
                            self.function(&f.sig, on_state);
                        }
                    }
                }
                Item::Trait(t) => {
                    for it in &t.items {
                        if let syn::TraitItem::Fn(f) = it {
                            self.function(&f.sig, false);
                        }
                    }
                }
                Item::Mod(m) => {
                    if let Some((_, inner)) = &m.content {
                        self.collect(inner);
                    }
                }
                _ => {}
            }
        }
    }

    fn function(&mut self, sig: &syn::Signature, self_is_state: bool) {
        let name = sig.ident.to_string();
        if let ReturnType::Type(_, t) = &sig.output
            && names_state_type(t)
        {
            self.state_fns.insert(name.clone());
        }
        let takes = sig.inputs.iter().any(|a| match a {
            FnArg::Typed(t) => param_is_state(sig, &t.ty),
            FnArg::Receiver(_) => self_is_state,
        });
        if takes {
            self.takers.insert(name);
        }
    }
}

/// Scans one file.
pub fn scan(file: &str, src: &str, engine: &EngineFacts, universe: &Universe) -> Result<Vec<Finding>, syn::Error> {
    let ast = syn::parse_file(src)?;
    let mut v = Scan {
        file,
        engine,
        universe,
        out: Vec::new(),
        state: Vec::new(),
        mut_params: Vec::new(),
        fns: Vec::new(),
        gated: 0,
    };
    v.visit_file(&ast);
    v.out.sort();
    v.out.dedup();
    Ok(v.out)
}

struct Scan<'a> {
    file: &'a str,
    engine: &'a EngineFacts,
    universe: &'a Universe,
    out: Vec<Finding>,
    /// Names bound to state, innermost function last. One scope per function.
    state: Vec<BTreeSet<String>>,
    /// Names of state parameters that are `&mut`.
    mut_params: Vec<BTreeSet<String>>,
    /// The functions we are inside, outermost first.
    fns: Vec<String>,
    /// Depth of `#[cfg(feature = "scenarios")]` items around the current position.
    gated: usize,
}

fn is_scenarios_cfg(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        a.path().is_ident("cfg")
            && a.meta.require_list().is_ok_and(|l| {
                let text = l.tokens.to_string();
                text.contains("feature") && text.contains("\"scenarios\"") && !text.contains("not")
            })
    })
}

impl Scan<'_> {
    fn in_fn(&self) -> String {
        self.fns.join("::")
    }

    fn report(&mut self, kind: Kind, name: &str) {
        self.out.push(Finding {
            file: self.file.to_owned(),
            kind,
            name: name.to_owned(),
            in_fn: self.in_fn(),
            scenarios_only: self.gated > 0,
        });
    }

    fn is_state_name(&self, name: &str) -> bool {
        self.state.last().is_some_and(|s| s.contains(name))
    }

    /// True when the expression is, or is a part of, a state value.
    fn is_state(&self, e: &Expr) -> bool {
        match e {
            Expr::Path(p) => p.path.get_ident().is_some_and(|i| self.is_state_name(&i.to_string())),
            Expr::Field(f) => {
                matches!(&f.member, syn::Member::Named(n) if self.universe.state_fields.contains(&n.to_string()))
                    || self.is_state(&f.base)
            }
            Expr::Index(i) => self.is_state(&i.expr),
            Expr::Paren(p) => self.is_state(&p.expr),
            Expr::Reference(r) => self.is_state(&r.expr),
            Expr::Unary(u) => self.is_state(&u.expr),
            Expr::Try(t) => self.is_state(&t.expr),
            Expr::Cast(c) => self.is_state(&c.expr),
            Expr::MethodCall(m) => {
                let name = m.method.to_string();
                let hands_out = self.engine.getters.contains(&name)
                    || self.universe.state_fns.contains(&name)
                    || PASS_THROUGH.contains(&name.as_str())
                    || name.ends_with("_mut");
                hands_out && (self.is_state(&m.receiver) || self.universe.state_fns.contains(&name))
            }
            Expr::Call(c) => match &*c.func {
                Expr::Path(p) => {
                    p.path.segments.last().is_some_and(|s| self.universe.state_fns.contains(&s.ident.to_string()))
                }
                _ => false,
            },
            _ => false,
        }
    }

    /// True when the expression is a bare state name (not a field of it).
    fn is_bare_state(&self, e: &Expr) -> bool {
        match e {
            Expr::Path(p) => self.is_state(e) && p.path.get_ident().is_some(),
            Expr::Reference(r) => self.is_bare_state(&r.expr),
            Expr::Paren(p) => self.is_bare_state(&p.expr),
            Expr::Field(f) => {
                matches!(&f.member, syn::Member::Named(n) if self.universe.state_fields.contains(&n.to_string()))
                    && !self.is_state(&f.base)
            }
            _ => false,
        }
    }

    fn bind_pattern(&mut self, pat: &Pat) {
        match pat {
            Pat::Ident(i) => {
                if let Some(s) = self.state.last_mut() {
                    s.insert(i.ident.to_string());
                }
            }
            Pat::Type(t) => self.bind_pattern(&t.pat),
            Pat::Reference(r) => self.bind_pattern(&r.pat),
            Pat::Tuple(t) => t.elems.iter().for_each(|p| self.bind_pattern(p)),
            _ => {}
        }
    }

    fn enter_fn(&mut self, name: &str, sig: &syn::Signature, self_is_state: bool) {
        self.fns.push(name.to_owned());
        let mut names = BTreeSet::new();
        let mut muts = BTreeSet::new();
        if self_is_state {
            names.insert("self".to_owned());
            if matches!(sig.inputs.first(), Some(FnArg::Receiver(r)) if r.mutability.is_some() || r.reference.is_none())
            {
                muts.insert("self".to_owned());
            }
        }
        for arg in &sig.inputs {
            let FnArg::Typed(t) = arg else { continue };
            if !param_is_state(sig, &t.ty) {
                continue;
            }
            if let Pat::Ident(i) = &*t.pat {
                names.insert(i.ident.to_string());
                if is_mut_ref(&t.ty) {
                    muts.insert(i.ident.to_string());
                }
            }
        }
        self.state.push(names);
        self.mut_params.push(muts);
    }

    fn leave_fn(&mut self) {
        self.fns.pop();
        self.state.pop();
        self.mut_params.pop();
    }

    /// An argument that gives the callee a mutable state: `&mut s`, `&mut self.session`, or a `&mut` state parameter.
    fn hands_out_mutably(&self, arg: &Expr) -> bool {
        match arg {
            Expr::Reference(r) if r.mutability.is_some() => self.is_state(&r.expr),
            Expr::Path(p) => {
                p.path.get_ident().is_some_and(|i| self.mut_params.last().is_some_and(|m| m.contains(&i.to_string())))
            }
            Expr::Paren(p) => self.hands_out_mutably(&p.expr),
            _ => false,
        }
    }

    fn macro_words(tokens: &proc_macro2::TokenStream) -> Vec<String> {
        tokens
            .to_string()
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|w| !w.is_empty())
            .map(str::to_owned)
            .collect()
    }

    fn check_macro(&mut self, mac: &syn::Macro) {
        let Some(name) = mac.path.segments.last().map(|s| s.ident.to_string()) else { return };
        let words = Self::macro_words(&mac.tokens);
        let holds_state = words.iter().any(|w| self.is_state_name(w) || self.universe.state_fields.contains(w));
        if !holds_state {
            return;
        }
        if PURE_MACROS.contains(&name.as_str()) {
            // Look inside the arguments that parse as expressions: `format!("{}", s.tool.take())`.
            let parsed = syn::parse::Parser::parse2(
                syn::punctuated::Punctuated::<Expr, syn::Token![,]>::parse_terminated,
                mac.tokens.clone(),
            );
            if let Ok(args) = parsed {
                for a in &args {
                    self.visit_expr(a);
                }
            }
            return;
        }
        self.report(Kind::Macro, &name);
    }
}

impl<'ast> Visit<'ast> for Scan<'_> {
    fn visit_item_fn(&mut self, f: &'ast syn::ItemFn) {
        let gated = is_scenarios_cfg(&f.attrs);
        self.gated += usize::from(gated);
        self.enter_fn(&f.sig.ident.to_string(), &f.sig, false);
        visit::visit_item_fn(self, f);
        self.leave_fn();
        self.gated -= usize::from(gated);
    }

    fn visit_item_impl(&mut self, imp: &'ast syn::ItemImpl) {
        let gated = is_scenarios_cfg(&imp.attrs);
        self.gated += usize::from(gated);
        let ty = type_name(&imp.self_ty).unwrap_or_default();
        let self_is_state = STATE_TYPES.contains(&ty.as_str());
        for it in &imp.items {
            match it {
                ImplItem::Fn(f) => {
                    let g = is_scenarios_cfg(&f.attrs);
                    self.gated += usize::from(g);
                    let name = if ty.is_empty() { f.sig.ident.to_string() } else { format!("{ty}::{}", f.sig.ident) };
                    self.enter_fn(&name, &f.sig, self_is_state);
                    self.visit_block(&f.block);
                    self.leave_fn();
                    self.gated -= usize::from(g);
                }
                other => self.visit_impl_item(other),
            }
        }
        self.gated -= usize::from(gated);
    }

    fn visit_item_mod(&mut self, m: &'ast syn::ItemMod) {
        let gated = is_scenarios_cfg(&m.attrs);
        self.gated += usize::from(gated);
        visit::visit_item_mod(self, m);
        self.gated -= usize::from(gated);
    }

    fn visit_local(&mut self, l: &'ast syn::Local) {
        // The initialiser is read first: `let t = s.tab_mut();` makes `t` state.
        let init_is_state = l.init.as_ref().is_some_and(|i| self.is_state(&i.expr));
        visit::visit_local(self, l);
        let typed_state = matches!(&l.pat, Pat::Type(t) if names_state_type(&t.ty));
        if init_is_state || typed_state {
            self.bind_pattern(&l.pat);
        }
    }

    fn visit_expr_assign(&mut self, a: &'ast syn::ExprAssign) {
        if self.is_state(&a.left) {
            self.report(Kind::Assign, &place_name(&a.left));
        }
        visit::visit_expr_assign(self, a);
    }

    fn visit_expr_binary(&mut self, b: &'ast syn::ExprBinary) {
        let compound = matches!(
            b.op,
            BinOp::AddAssign(_)
                | BinOp::SubAssign(_)
                | BinOp::MulAssign(_)
                | BinOp::DivAssign(_)
                | BinOp::RemAssign(_)
                | BinOp::BitXorAssign(_)
                | BinOp::BitAndAssign(_)
                | BinOp::BitOrAssign(_)
                | BinOp::ShlAssign(_)
                | BinOp::ShrAssign(_)
        );
        if compound && self.is_state(&b.left) {
            self.report(Kind::Assign, &place_name(&b.left));
        }
        visit::visit_expr_binary(self, b);
    }

    fn visit_expr_reference(&mut self, r: &'ast syn::ExprReference) {
        // `&mut s` alone is fine here; it is judged where it is passed. A borrow of a part of the state is not.
        if r.mutability.is_some() && self.is_state(&r.expr) && !self.is_bare_state(&r.expr) {
            self.report(Kind::MutBorrow, &place_name(&r.expr));
        }
        visit::visit_expr_reference(self, r);
    }

    fn visit_expr_method_call(&mut self, m: &'ast syn::ExprMethodCall) {
        let name = m.method.to_string();
        if !COMMANDS.contains(&name.as_str()) {
            if self.is_state(&m.receiver)
                && (self.engine.mutators.contains(&name) || COLLECTION_MUTATORS.contains(&name.as_str()))
            {
                self.report(Kind::Method, &name);
            } else if !self.universe.takers.contains(&name) && m.args.iter().any(|a| self.hands_out_mutably(a)) {
                self.report(Kind::HandedOut, &name);
            }
        }
        visit::visit_expr_method_call(self, m);
    }

    fn visit_expr_call(&mut self, c: &'ast syn::ExprCall) {
        if let Expr::Path(p) = &*c.func {
            let segs: Vec<String> = p.path.segments.iter().map(|s| s.ident.to_string()).collect();
            if let [.., ty, name] = segs.as_slice() {
                let on_state_type = STATE_TYPES.contains(&ty.as_str()) || ty == "Self" && self.is_state_name("self");
                let takes_state = c.args.first().is_some_and(|a| self.hands_out_mutably(a));
                if !COMMANDS.contains(&name.as_str())
                    && on_state_type
                    && (self.engine.mutators.contains(name) && takes_state)
                {
                    self.report(Kind::PathCall, name);
                    visit::visit_expr_call(self, c);
                    return;
                }
            }
            if let Some(name) = segs.last()
                && !COMMANDS.contains(&name.as_str())
                && !self.universe.takers.contains(name)
                && c.args.iter().any(|a| self.hands_out_mutably(a))
            {
                self.report(Kind::HandedOut, name);
            }
        }
        visit::visit_expr_call(self, c);
    }

    fn visit_expr_macro(&mut self, m: &'ast syn::ExprMacro) {
        self.check_macro(&m.mac);
    }

    fn visit_stmt_macro(&mut self, m: &'ast syn::StmtMacro) {
        self.check_macro(&m.mac);
    }

    fn visit_expr_closure(&mut self, c: &'ast syn::ExprClosure) {
        // A closure parameter typed as state is state inside the closure.
        for p in &c.inputs {
            if let Pat::Type(t) = p
                && names_state_type(&t.ty)
            {
                self.bind_pattern(&t.pat);
            }
        }
        visit::visit_expr_closure(self, c);
    }
}

/// A short name for the place: the last field or method on the path (`tool.lock` gives `lock`).
fn place_name(e: &Expr) -> String {
    match e {
        Expr::Field(f) => match &f.member {
            syn::Member::Named(n) => n.to_string(),
            syn::Member::Unnamed(i) => i.index.to_string(),
        },
        Expr::Index(i) => place_name(&i.expr),
        Expr::Paren(p) => place_name(&p.expr),
        Expr::Unary(u) => place_name(&u.expr),
        Expr::Reference(r) => place_name(&r.expr),
        Expr::MethodCall(m) => m.method.to_string(),
        Expr::Path(p) => p.path.get_ident().map(ToString::to_string).unwrap_or_default(),
        _ => "<expr>".to_owned(),
    }
}
