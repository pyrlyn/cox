// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! D11 and A90 as a test: `cox-ffi` holds no logic. Every function and
//! method body in `lib.rs`, `session.rs`, `host.rs` and `remote.rs` —
//! exported through `#[uniffi::export]` or not — is one forward expression;
//! anything that decides something belongs in `cox-app`, where it is tested
//! once, and type mapping belongs in `types.rs`, which this test does not
//! read. Where a
//! forward lands is `deps.rs`'s job (the crate depends only on `cox-app` and
//! `cox-protocol`); this test checks the shape, parsing the files with `syn`.
//!
//! One forward expression: a body of exactly one statement, which is an
//! expression (with or without a trailing `;`) built only from paths,
//! literals, calls, method calls, field access, `?`, `.await`, `&`,
//! parentheses, tuples, struct literals, and closures or `async` blocks
//! whose own body is again one such expression. So a method chain,
//! `.map(Into::into)`, `?` and a struct literal of conversions pass. A
//! `let`, a nested item, a macro, `if`, `match`, a loop, `return`, an
//! operator, an `as` cast, indexing, a range or an assignment fails —
//! anything outside the list fails, so a new construct is logic until this
//! list says otherwise. `#[cfg(test)]` modules are not read.
//!
//! Exempt, by `impl` and reason — keep it short:
//! - `impl From<OwnerError> for AppError`: folds `cox_app::app::AppError`'s
//!   nested errors into the flat enum Swift sees; one `match` over variants
//!   that only picks the Swift case.

use std::fs;
use std::path::Path;

use syn::{Attribute, Block, Expr, GenericArgument, ImplItem, Item, Meta, PathArguments, Stmt};
use syn::{TraitItem, Type};

/// `(self type, trait)` of the impls whose bodies are not checked.
const EXEMPT: &[(&str, &str)] = &[("AppError", "From<OwnerError>")];

/// The files the rule covers.
const FILES: &[&str] = &["lib.rs", "session.rs", "host.rs", "remote.rs"];

/// `None` when `e` is a forward, else the construct that makes it logic.
fn logic_in(e: &Expr) -> Option<String> {
    match e {
        Expr::Path(_) | Expr::Lit(_) => None,
        Expr::Call(c) => logic_in(&c.func).or_else(|| all(&c.args)),
        Expr::MethodCall(m) => logic_in(&m.receiver).or_else(|| all(&m.args)),
        Expr::Field(f) => logic_in(&f.base),
        Expr::Try(t) => logic_in(&t.expr),
        Expr::Await(a) => logic_in(&a.base),
        Expr::Reference(r) => logic_in(&r.expr),
        Expr::Paren(p) => logic_in(&p.expr),
        Expr::Tuple(t) => all(&t.elems),
        Expr::Struct(s) => {
            all(s.fields.iter().map(|f| &f.expr)).or_else(|| s.rest.as_deref().and_then(logic_in))
        }
        Expr::Closure(c) => logic_in(&c.body),
        Expr::Async(a) => logic_in_block(&a.block),
        Expr::Block(b) if b.label.is_none() => logic_in_block(&b.block),
        Expr::If(_) => Some("an `if`".into()),
        Expr::Match(_) => Some("a `match`".into()),
        Expr::ForLoop(_) | Expr::While(_) | Expr::Loop(_) => Some("a loop".into()),
        Expr::Macro(_) => Some("a macro".into()),
        Expr::Binary(_) | Expr::Unary(_) => Some("an operator".into()),
        Expr::Cast(_) => Some("an `as` cast".into()),
        Expr::Return(_) => Some("a `return`".into()),
        _ => Some("an expression outside the forward list".into()),
    }
}

/// The first construct in `es` that is logic.
fn all<'a>(es: impl IntoIterator<Item = &'a Expr>) -> Option<String> {
    es.into_iter().find_map(logic_in)
}

/// `None` when `block` holds one forward expression.
fn logic_in_block(block: &Block) -> Option<String> {
    match block.stmts.as_slice() {
        [Stmt::Expr(e, _)] => logic_in(e),
        [Stmt::Local(_)] => Some("a `let`".into()),
        [Stmt::Item(_)] => Some("a nested item".into()),
        [Stmt::Macro(_)] => Some("a macro".into()),
        [] => Some("an empty body".into()),
        stmts => Some(format!("{} statements", stmts.len())),
    }
}

/// `Name` or `Name<Arg>` from a path's last segment, for the exempt list.
fn name(path: &syn::Path) -> String {
    let Some(last) = path.segments.last() else {
        return String::new();
    };
    let arg = match &last.arguments {
        PathArguments::AngleBracketed(a) => a.args.first().and_then(|g| match g {
            GenericArgument::Type(Type::Path(t)) => Some(format!("<{}>", name(&t.path))),
            _ => None,
        }),
        _ => None,
    };
    format!("{}{}", last.ident, arg.unwrap_or_default())
}

fn is_cfg_test(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|a| match &a.meta {
        Meta::List(l) => l.path.is_ident("cfg") && l.tokens.to_string().contains("test"),
        _ => false,
    })
}

/// Every body in `items` that is not one forward, as `file: fn — why`.
fn violations(file: &str, items: &[Item], out: &mut Vec<String>) {
    let check = |out: &mut Vec<String>, name: &str, block: &Block| {
        if let Some(why) = logic_in_block(block) {
            out.push(format!("{file}: fn {name} — {why}"));
        }
    };
    for item in items {
        match item {
            Item::Fn(f) => check(out, &f.sig.ident.to_string(), &f.block),
            Item::Impl(i) => {
                let this = match i.self_ty.as_ref() {
                    Type::Path(p) => name(&p.path),
                    _ => "?".to_string(),
                };
                let tr = i.trait_.as_ref().map(|(p, _)| name(p));
                if EXEMPT
                    .iter()
                    .any(|(t, r)| *t == this && tr.as_deref() == Some(*r))
                {
                    continue;
                }
                for f in i.items.iter().filter_map(|it| match it {
                    ImplItem::Fn(f) => Some(f),
                    _ => None,
                }) {
                    check(out, &format!("{this}::{}", f.sig.ident), &f.block);
                }
            }
            Item::Trait(t) => {
                for f in t.items.iter().filter_map(|it| match it {
                    TraitItem::Fn(f) => f.default.as_ref().map(|b| (&f.sig.ident, b)),
                    _ => None,
                }) {
                    check(out, &format!("{}::{}", t.ident, f.0), f.1);
                }
            }
            Item::Mod(m) if !is_cfg_test(&m.attrs) => {
                if let Some((_, inner)) = &m.content {
                    violations(file, inner, out);
                }
            }
            _ => {}
        }
    }
}

fn check_source(file: &str, source: &str) -> Vec<String> {
    let mut out = Vec::new();
    match syn::parse_file(source) {
        Ok(parsed) => violations(file, &parsed.items, &mut out),
        Err(e) => out.push(format!("{file}: does not parse — {e}")),
    }
    out
}

#[test]
fn every_ffi_body_is_one_forward_expression() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    for file in FILES {
        let source = fs::read_to_string(src.join(file)).expect("read a cox-ffi source");
        found.extend(check_source(file, &source));
    }
    assert!(
        found.is_empty(),
        "cox-ffi holds logic (D11, A90) — move it into cox-app or types.rs:\n{}",
        found.join("\n")
    );
}

#[test]
fn a_forward_chain_with_try_await_and_conversions_passes() {
    let source = r"
        impl App {
            pub fn projects(&self, limit: u32) -> Result<Vec<Project>, AppError> {
                Ok(self.owner.workspace().projects(i64::from(limit))?.into_iter().map(Into::into).collect())
            }
            pub async fn open(self: Arc<Self>, r: OpenRequest) -> Result<Arc<S>, AppError> {
                Ok(S::new(on_runtime(async move { self.owner.open(r.cwd.into()).await }).await??))
            }
            pub fn new(home: Option<String>) -> Arc<Self> {
                Arc::new(Self { owner: Owner::new(home.map(PathBuf::from)) })
            }
            pub fn close(&self) {
                self.live.close();
            }
        }";
    assert_eq!(check_source("x.rs", source), Vec::<String>::new());
}

#[test]
fn an_if_a_match_or_a_second_statement_is_logic() {
    let source = r"
        impl App {
            pub fn a(&self) -> u32 { if self.on { 1 } else { 2 } }
            pub fn b(&self) -> u32 { match self.n { 0 => 1, _ => 2 } }
            pub fn c(&self) -> u32 { let n = self.n; self.owner.get(n) }
            pub async fn d(&self) { on_runtime(async move { let x = 1; go(x).await }).await; }
            pub fn e(&self) -> u32 { self.n + 1 }
        }";
    let found = check_source("x.rs", source);
    assert_eq!(
        found,
        [
            "x.rs: fn App::a — an `if`",
            "x.rs: fn App::b — a `match`",
            "x.rs: fn App::c — 2 statements",
            "x.rs: fn App::d — 2 statements",
            "x.rs: fn App::e — an operator",
        ]
    );
}

#[test]
fn only_the_listed_impl_is_exempt() {
    let source = r"
        impl From<OwnerError> for AppError { fn from(e: OwnerError) -> Self { match e { _ => x } } }
        impl From<Other> for AppError { fn from(e: Other) -> Self { match e { _ => x } } }";
    assert_eq!(
        check_source("x.rs", source),
        ["x.rs: fn AppError::from — a `match`"]
    );
}
