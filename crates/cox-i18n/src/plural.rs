// Copyright (c) 2026 Ivan Tugay
// SPDX-License-Identifier: GPL-3.0-or-later
// Licensed under GPL-3.0 or later; see https://www.gnu.org/licenses/gpl-3.0.html

//! gettext `Plural-Forms` expressions: the C subset GNU gettext accepts
//! (`n`, unsigned integer literals, `!`, `* / %`, `+ -`, `< <= > >=`,
//! `== !=`, `&&`, `||`, `?:` and parentheses), parsed once per catalog and
//! evaluated per lookup. Own code rather than a crate: the pure-Rust gettext
//! crates either keep their evaluator private (`tr`, `gettext`) or have none
//! (`polib`), and the grammar is small enough to test exhaustively.

use std::fmt;

/// A parsed `Plural-Forms` rule: how many forms a plural message has and
/// which one a count selects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluralRule {
    nplurals: usize,
    expr: Expr,
}

/// Why a `Plural-Forms` expression did not parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluralError(pub String);

impl fmt::Display for PluralError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PluralError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
    Rem,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Expr {
    N,
    Num(u64),
    Not(Box<Expr>),
    Bin(Op, Box<Expr>, Box<Expr>),
    Cond(Box<Expr>, Box<Expr>, Box<Expr>),
}

impl PluralRule {
    /// `nplurals` and the `plural=` expression, as a `.po` header holds them.
    pub fn new(nplurals: usize, expr: &str) -> Result<Self, PluralError> {
        if nplurals == 0 {
            return Err(PluralError("nplurals must be at least 1".into()));
        }
        let tokens = tokenize(expr)?;
        let mut p = Parser { tokens, pos: 0 };
        let expr = p.ternary()?;
        if let Some(t) = p.tokens.get(p.pos) {
            return Err(PluralError(format!(
                "unexpected `{t}` after the expression"
            )));
        }
        Ok(Self { nplurals, expr })
    }

    /// The number of plural forms (`msgstr[0]` … `msgstr[nplurals - 1]`).
    pub fn nplurals(&self) -> usize {
        self.nplurals
    }

    /// The form index for `n`. An index outside `0..nplurals` gives 0, as GNU
    /// gettext does.
    pub fn index(&self, n: u64) -> usize {
        let i = eval(&self.expr, n);
        usize::try_from(i)
            .ok()
            .filter(|&i| i < self.nplurals)
            .unwrap_or(0)
    }
}

fn eval(e: &Expr, n: u64) -> u64 {
    match e {
        Expr::N => n,
        Expr::Num(v) => *v,
        Expr::Not(a) => u64::from(eval(a, n) == 0),
        Expr::Cond(c, a, b) => {
            if eval(c, n) != 0 {
                eval(a, n)
            } else {
                eval(b, n)
            }
        }
        Expr::Bin(op, a, b) => {
            let x = eval(a, n);
            // `&&` and `||` short-circuit like C.
            match op {
                Op::And => return u64::from(x != 0 && eval(b, n) != 0),
                Op::Or => return u64::from(x != 0 || eval(b, n) != 0),
                _ => {}
            }
            let y = eval(b, n);
            match op {
                Op::Eq => u64::from(x == y),
                Op::Ne => u64::from(x != y),
                Op::Lt => u64::from(x < y),
                Op::Le => u64::from(x <= y),
                Op::Gt => u64::from(x > y),
                Op::Ge => u64::from(x >= y),
                Op::Add => x.wrapping_add(y),
                Op::Sub => x.wrapping_sub(y),
                Op::Mul => x.wrapping_mul(y),
                // Division by zero would trap in C; here it selects form 0.
                Op::Div => x.checked_div(y).unwrap_or(0),
                Op::Rem => x.checked_rem(y).unwrap_or(0),
                Op::And | Op::Or => unreachable!("handled above"),
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    N,
    Num(u64),
    Sym(&'static str),
}

impl fmt::Display for Tok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tok::N => f.write_str("n"),
            Tok::Num(v) => write!(f, "{v}"),
            Tok::Sym(s) => f.write_str(s),
        }
    }
}

const SYMBOLS: [&str; 19] = [
    "||", "&&", "==", "!=", "<=", ">=", "<", ">", "!", "+", "-", "*", "/", "%", "?", ":", "(", ")",
    "n",
];

fn tokenize(src: &str) -> Result<Vec<Tok>, PluralError> {
    let mut out = Vec::new();
    let mut rest = src.trim_start();
    while !rest.is_empty() {
        let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 {
            let v = rest[..digits]
                .parse()
                .map_err(|_| PluralError(format!("number `{}` is too large", &rest[..digits])))?;
            out.push(Tok::Num(v));
            rest = &rest[digits..];
        } else if let Some(sym) = SYMBOLS.iter().find(|s| rest.starts_with(**s)) {
            out.push(if *sym == "n" { Tok::N } else { Tok::Sym(sym) });
            rest = &rest[sym.len()..];
        } else {
            let c = rest.chars().next().unwrap_or_default();
            return Err(PluralError(format!("unexpected character `{c}`")));
        }
        rest = rest.trim_start();
    }
    Ok(out)
}

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn eat(&mut self, sym: &str) -> bool {
        if matches!(self.tokens.get(self.pos), Some(Tok::Sym(s)) if *s == sym) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn ternary(&mut self) -> Result<Expr, PluralError> {
        let cond = self.binary(0)?;
        if !self.eat("?") {
            return Ok(cond);
        }
        let a = self.ternary()?;
        if !self.eat(":") {
            return Err(PluralError("`?` without a matching `:`".into()));
        }
        let b = self.ternary()?;
        Ok(Expr::Cond(Box::new(cond), Box::new(a), Box::new(b)))
    }

    /// Precedence climbing over the binary operators, loosest level first.
    fn binary(&mut self, level: usize) -> Result<Expr, PluralError> {
        const LEVELS: [&[(&str, Op)]; 6] = [
            &[("||", Op::Or)],
            &[("&&", Op::And)],
            &[("==", Op::Eq), ("!=", Op::Ne)],
            &[("<=", Op::Le), (">=", Op::Ge), ("<", Op::Lt), (">", Op::Gt)],
            &[("+", Op::Add), ("-", Op::Sub)],
            &[("*", Op::Mul), ("/", Op::Div), ("%", Op::Rem)],
        ];
        let Some(ops) = LEVELS.get(level) else {
            return self.unary();
        };
        let mut lhs = self.binary(level + 1)?;
        'outer: loop {
            for (sym, op) in *ops {
                if self.eat(sym) {
                    let rhs = self.binary(level + 1)?;
                    lhs = Expr::Bin(*op, Box::new(lhs), Box::new(rhs));
                    continue 'outer;
                }
            }
            return Ok(lhs);
        }
    }

    fn unary(&mut self) -> Result<Expr, PluralError> {
        if self.eat("!") {
            return Ok(Expr::Not(Box::new(self.unary()?)));
        }
        if self.eat("(") {
            let e = self.ternary()?;
            if !self.eat(")") {
                return Err(PluralError("missing `)`".into()));
            }
            return Ok(e);
        }
        match self.tokens.get(self.pos).cloned() {
            Some(Tok::N) => {
                self.pos += 1;
                Ok(Expr::N)
            }
            Some(Tok::Num(v)) => {
                self.pos += 1;
                Ok(Expr::Num(v))
            }
            Some(t) => Err(PluralError(format!("unexpected `{t}`"))),
            None => Err(PluralError("the expression ends early".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLAVIC: &str =
        "(n%10==1 && n%100!=11 ? 0 : n%10>=2 && n%10<=4 && (n%100<10 || n%100>=20) ? 1 : 2)";

    /// CLDR's integer rule for ru and uk, written out by hand.
    fn cldr_slavic(n: u64) -> usize {
        let (m10, m100) = (n % 10, n % 100);
        if m10 == 1 && m100 != 11 {
            0
        } else if (2..=4).contains(&m10) && !(12..=14).contains(&m100) {
            1
        } else {
            2
        }
    }

    #[test]
    fn slavic_rule_matches_cldr_for_the_first_ten_thousand_counts() {
        let rule = PluralRule::new(3, SLAVIC).expect("parses");
        for n in 0..10_000 {
            assert_eq!(rule.index(n), cldr_slavic(n), "n = {n}");
        }
    }

    #[test]
    fn english_rule_and_precedence() {
        let en = PluralRule::new(2, "(n != 1)").expect("parses");
        assert_eq!([en.index(0), en.index(1), en.index(2)], [1, 0, 1]);
        // `*` binds tighter than `+`, `&&` tighter than `||`, `?:` is right-associative.
        let e =
            PluralRule::new(9, "1 + 2 * 3 == 7 || 0 && 0 ? n == 0 ? 8 : 5 : 4").expect("parses");
        assert_eq!(e.index(0), 8);
        assert_eq!(e.index(3), 5);
        assert_eq!(PluralRule::new(2, "!n").expect("parses").index(0), 1);
    }

    #[test]
    fn out_of_range_index_and_division_by_zero_select_form_zero() {
        assert_eq!(PluralRule::new(2, "5").expect("parses").index(1), 0);
        assert_eq!(
            PluralRule::new(2, "1 / (n - n)").expect("parses").index(3),
            0
        );
    }

    #[test]
    fn malformed_expressions_are_rejected() {
        for bad in ["", "n ==", "(n", "n ? 1", "n 1", "x", "n & 1"] {
            assert!(PluralRule::new(2, bad).is_err(), "`{bad}` parsed");
        }
        assert!(PluralRule::new(0, "0").is_err());
    }
}
