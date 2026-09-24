//! A one-line expression language for shapes a panel has no data for, like a
//! strip pointed at a radio stream. Numbers, `x`, `t`, `pi`, `+ - * / ^`,
//! parentheses, and a dozen functions; one number out, meaning left to the
//! caller.
//!
//! Parse on text change, never while painting. Evaluation walks a postfix run
//! over a fixed stack with no allocation: a strip calls it a few hundred times
//! a frame. Errors carry a byte offset. Evaluation never fails but can return
//! infinity or NaN, which the caller sanitizes.

use std::fmt;

/// Every nesting path funnels through `unary`, so the cap counts there. Without
/// it a hand-edited config full of open parens overflows the thread's stack.
const NEST: usize = 32;

/// The parse checks the run against this so [`Expr::eval`] can use a fixed
/// array. Four per nesting level covers a three-argument call nested to the
/// floor.
const STACK: usize = NEST * 4;

/// Keeps a pathological expression in a shared layout from slowing a panel.
pub const MAX_LEN: usize = 512;

pub struct Expr {
    ops: Vec<Op>,
}

impl Expr {
    pub fn parse(src: &str) -> Result<Expr, ParseError> {
        if src.len() > MAX_LEN {
            return Err(ParseError {
                at: MAX_LEN,
                what: Problem::TooLong,
            });
        }

        let toks = lex(src)?;
        let mut parser = Parser {
            toks,
            at: 0,
            end: src.len(),
            ops: Ops::default(),
            nest: 0,
        };
        parser.expr()?;

        if parser.at < parser.toks.len() {
            return Err(ParseError {
                at: parser.here(),
                what: Problem::Trailing,
            });
        }
        if parser.ops.max > STACK {
            return Err(ParseError {
                at: 0,
                what: Problem::TooDeep,
            });
        }

        Ok(Expr {
            ops: parser.ops.ops,
        })
    }

    pub fn eval(&self, x: f32, t: f32) -> f32 {
        // The parse proved the run never stacks deeper than `STACK`.
        let mut stack = [0.0f32; STACK];
        let mut top = 0usize;

        for op in &self.ops {
            match *op {
                Op::Num(value) => {
                    stack[top] = value;
                    top += 1;
                }

                Op::X => {
                    stack[top] = x;
                    top += 1;
                }

                Op::T => {
                    stack[top] = t;
                    top += 1;
                }

                Op::Neg => stack[top - 1] = -stack[top - 1],

                Op::Add => {
                    top -= 1;
                    stack[top - 1] += stack[top];
                }

                Op::Sub => {
                    top -= 1;
                    stack[top - 1] -= stack[top];
                }

                Op::Mul => {
                    top -= 1;
                    stack[top - 1] *= stack[top];
                }

                Op::Div => {
                    top -= 1;
                    stack[top - 1] /= stack[top];
                }

                Op::Pow => {
                    top -= 1;
                    stack[top - 1] = stack[top - 1].powf(stack[top]);
                }

                Op::Call(func) => {
                    let arity = func.arity();
                    top -= arity;
                    let value = func.apply(&stack[top..top + arity]);
                    stack[top] = value;
                    top += 1;
                }
            }
        }

        stack[0]
    }
}

/// `at` is a byte offset into the parsed source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ParseError {
    pub at: usize,
    pub what: Problem,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {}", self.what.text(), self.at)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Problem {
    Stray,
    Unknown,
    Arity,
    Unclosed,
    Ended,
    Trailing,
    TooDeep,
    TooLong,
}

impl Problem {
    fn text(self) -> &'static str {
        match self {
            Problem::Stray => "unexpected character",
            Problem::Unknown => "unknown name",
            Problem::Arity => "wrong number of arguments",
            Problem::Unclosed => "unclosed bracket",
            Problem::Ended => "expression ends early",
            Problem::Trailing => "leftover text",
            Problem::TooDeep => "nested too deep",
            Problem::TooLong => "too long",
        }
    }
}

#[derive(Clone, Copy)]
enum Op {
    Num(f32),
    X,
    T,
    Neg,
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Call(Func),
}

impl Op {
    fn pops(self) -> usize {
        match self {
            Op::Num(_) | Op::X | Op::T => 0,
            Op::Neg => 1,
            Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Pow => 2,
            Op::Call(func) => func.arity(),
        }
    }
}

#[derive(Clone, Copy)]
enum Func {
    Sin,
    Cos,
    Tan,
    Abs,
    Sqrt,
    Exp,
    Ln,
    Floor,
    Min,
    Max,
    Clamp,
    Mix,
}

impl Func {
    fn named(name: &str) -> Option<Func> {
        Some(match name {
            "sin" => Func::Sin,
            "cos" => Func::Cos,
            "tan" => Func::Tan,
            "abs" => Func::Abs,
            "sqrt" => Func::Sqrt,
            "exp" => Func::Exp,
            "ln" => Func::Ln,
            "floor" => Func::Floor,
            "min" => Func::Min,
            "max" => Func::Max,
            "clamp" => Func::Clamp,
            "mix" => Func::Mix,
            _ => return None,
        })
    }

    fn arity(self) -> usize {
        match self {
            Func::Min | Func::Max => 2,
            Func::Clamp | Func::Mix => 3,
            _ => 1,
        }
    }

    /// `args` is exactly [`Func::arity`] long; the parse counted them.
    fn apply(self, args: &[f32]) -> f32 {
        match self {
            Func::Sin => args[0].sin(),
            Func::Cos => args[0].cos(),
            Func::Tan => args[0].tan(),
            Func::Abs => args[0].abs(),
            Func::Sqrt => args[0].sqrt(),
            Func::Exp => args[0].exp(),
            Func::Ln => args[0].ln(),
            Func::Floor => args[0].floor(),
            Func::Min => args[0].min(args[1]),
            Func::Max => args[0].max(args[1]),
            // Not f32::clamp, which panics on reversed bounds and would take
            // the window down from a paint pass.
            Func::Clamp => args[0].max(args[1]).min(args[2]),
            Func::Mix => args[0] + (args[1] - args[0]) * args[2],
        }
    }
}

#[derive(Default)]
struct Ops {
    ops: Vec<Op>,
    depth: usize,
    max: usize,
}

impl Ops {
    fn push(&mut self, op: Op) {
        self.depth = self.depth.saturating_sub(op.pops()) + 1;
        self.max = self.max.max(self.depth);
        self.ops.push(op);
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Tok<'a> {
    Num(f32),
    Name(&'a str),
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    Open,
    Close,
    Comma,
}

/// Everything the grammar accepts is ASCII, so offsets are byte offsets.
fn lex(src: &str) -> Result<Vec<(usize, Tok<'_>)>, ParseError> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        let c = bytes[i];
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }

        // Two dots in one run fail in the parse, at the start of the run.
        if c.is_ascii_digit() || c == b'.' {
            let from = i;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            let value = src[from..i].parse::<f32>().map_err(|_| ParseError {
                at: from,
                what: Problem::Stray,
            })?;
            out.push((from, Tok::Num(value)));
            continue;
        }

        if c.is_ascii_alphabetic() {
            let from = i;
            while i < bytes.len() && bytes[i].is_ascii_alphanumeric() {
                i += 1;
            }
            out.push((from, Tok::Name(&src[from..i])));
            continue;
        }

        let tok = match c {
            b'+' => Tok::Plus,
            b'-' => Tok::Minus,
            b'*' => Tok::Star,
            b'/' => Tok::Slash,
            b'^' => Tok::Caret,
            b'(' => Tok::Open,
            b')' => Tok::Close,
            b',' => Tok::Comma,
            _ => {
                return Err(ParseError {
                    at: i,
                    what: Problem::Stray,
                });
            }
        };
        out.push((i, tok));
        i += 1;
    }

    Ok(out)
}

struct Parser<'a> {
    toks: Vec<(usize, Tok<'a>)>,
    at: usize,
    end: usize,
    ops: Ops,
    nest: usize,
}

impl<'a> Parser<'a> {
    fn peek(&self) -> Option<Tok<'a>> {
        self.toks.get(self.at).map(|(_, tok)| *tok)
    }

    fn here(&self) -> usize {
        self.toks
            .get(self.at)
            .map(|(at, _)| *at)
            .unwrap_or(self.end)
    }

    fn eat(&mut self, want: Tok<'a>) -> bool {
        if self.peek() == Some(want) {
            self.at += 1;
            return true;
        }

        false
    }

    fn expr(&mut self) -> Result<(), ParseError> {
        self.term()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Plus) => Op::Add,
                Some(Tok::Minus) => Op::Sub,
                _ => return Ok(()),
            };
            self.at += 1;
            self.term()?;
            self.ops.push(op);
        }
    }

    fn term(&mut self) -> Result<(), ParseError> {
        self.unary()?;
        loop {
            let op = match self.peek() {
                Some(Tok::Star) => Op::Mul,
                Some(Tok::Slash) => Op::Div,
                _ => return Ok(()),
            };
            self.at += 1;
            self.unary()?;
            self.ops.push(op);
        }
    }

    /// Every recursive path comes back through here, so the nesting cap does too.
    fn unary(&mut self) -> Result<(), ParseError> {
        if self.nest >= NEST {
            return Err(ParseError {
                at: self.here(),
                what: Problem::TooDeep,
            });
        }

        self.nest += 1;
        let out = self.signed();
        self.nest -= 1;

        out
    }

    fn signed(&mut self) -> Result<(), ParseError> {
        if self.eat(Tok::Minus) {
            self.unary()?;
            self.ops.push(Op::Neg);
            return Ok(());
        }

        self.power()
    }

    /// `-2^2` is `-4` and `2^-1` is a half. The right side is a unary rather
    /// than a power, which makes a chain associate to the right.
    fn power(&mut self) -> Result<(), ParseError> {
        self.atom()?;
        if self.eat(Tok::Caret) {
            self.unary()?;
            self.ops.push(Op::Pow);
        }

        Ok(())
    }

    fn atom(&mut self) -> Result<(), ParseError> {
        let at = self.here();
        match self.peek() {
            Some(Tok::Num(value)) => {
                self.at += 1;
                self.ops.push(Op::Num(value));
                Ok(())
            }

            Some(Tok::Name(name)) => {
                self.at += 1;
                self.name(name, at)
            }

            Some(Tok::Open) => {
                self.at += 1;
                self.expr()?;
                if !self.eat(Tok::Close) {
                    // Point at the bracket that never closed, where the fix goes.
                    return Err(ParseError {
                        at,
                        what: Problem::Unclosed,
                    });
                }

                Ok(())
            }

            None => Err(ParseError {
                at,
                what: Problem::Ended,
            }),

            Some(_) => Err(ParseError {
                at,
                what: Problem::Stray,
            }),
        }
    }

    fn name(&mut self, name: &str, at: usize) -> Result<(), ParseError> {
        match name {
            "x" => {
                self.ops.push(Op::X);
                return Ok(());
            }

            "t" => {
                self.ops.push(Op::T);
                return Ok(());
            }

            "pi" => {
                self.ops.push(Op::Num(std::f32::consts::PI));
                return Ok(());
            }

            _ => {}
        }

        let Some(func) = Func::named(name) else {
            return Err(ParseError {
                at,
                what: Problem::Unknown,
            });
        };
        if !self.eat(Tok::Open) {
            return Err(ParseError {
                at: self.here(),
                what: Problem::Stray,
            });
        }

        let mut count = 0;
        loop {
            self.expr()?;
            count += 1;
            if !self.eat(Tok::Comma) {
                break;
            }
        }
        if !self.eat(Tok::Close) {
            return Err(ParseError {
                at,
                what: Problem::Unclosed,
            });
        }

        // Checked after the parse, so `min(1)` blames the call, not the
        // bracket after it.
        if count != func.arity() {
            return Err(ParseError {
                at,
                what: Problem::Arity,
            });
        }

        self.ops.push(Op::Call(func));

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(src: &str, x: f32, t: f32) -> f32 {
        Expr::parse(src)
            .unwrap_or_else(|e| panic!("{src:?} should parse: {e}"))
            .eval(x, t)
    }

    fn value(src: &str) -> f32 {
        at(src, 0.0, 0.0)
    }

    fn error(src: &str) -> ParseError {
        Expr::parse(src)
            .err()
            .unwrap_or_else(|| panic!("{src:?} should not parse"))
    }

    #[test]
    fn precedence_and_association_follow_arithmetic() {
        assert_eq!(value("1 + 2 * 3"), 7.0);
        assert_eq!(value("(1 + 2) * 3"), 9.0);
        assert_eq!(value("10 - 3 - 4"), 3.0);
        assert_eq!(value("16 / 4 / 2"), 2.0);
        assert_eq!(value("1 + 2 - 3 * 4 / 2"), -3.0);
    }

    #[test]
    fn the_exponent_associates_right_and_outranks_a_leading_minus() {
        assert_eq!(value("2 ^ 3 ^ 2"), 512.0);
        assert_eq!(value("-2 ^ 2"), -4.0);
        assert_eq!(value("2 ^ -1"), 0.5);
        assert_eq!(value("2 * 3 ^ 2"), 18.0);
    }

    #[test]
    fn a_unary_minus_negates_what_follows_it() {
        assert_eq!(value("-3"), -3.0);
        assert_eq!(value("--3"), 3.0);
        assert_eq!(value("1 - -2"), 3.0);
        assert_eq!(at("-x", 2.0, 0.0), -2.0);
        assert_eq!(value("-(1 + 2)"), -3.0);
    }

    #[test]
    fn x_and_t_and_pi_read_their_values() {
        assert_eq!(at("x", 0.25, 9.0), 0.25);
        assert_eq!(at("t", 0.25, 9.0), 9.0);
        assert!((value("pi") - std::f32::consts::PI).abs() < 1e-6);
        assert_eq!(at("x * t", 3.0, 4.0), 12.0);
    }

    #[test]
    fn every_function_computes_its_own_thing() {
        let near = |src: &str, want: f32| {
            let got = value(src);
            assert!((got - want).abs() < 1e-5, "{src} gave {got}, wanted {want}");
        };

        near("sin(0)", 0.0);
        near("sin(pi / 2)", 1.0);
        near("cos(0)", 1.0);
        near("tan(0)", 0.0);
        near("abs(0 - 4)", 4.0);
        near("sqrt(9)", 3.0);
        near("exp(0)", 1.0);
        near("ln(1)", 0.0);
        near("floor(2.7)", 2.0);
        near("min(3, 5)", 3.0);
        near("max(3, 5)", 5.0);
        near("clamp(9, 0, 1)", 1.0);
        near("clamp(0 - 9, 0, 1)", 0.0);
        near("mix(0, 10, 0.25)", 2.5);
    }

    #[test]
    fn a_parse_error_names_the_byte_it_stopped_at() {
        assert_eq!(
            error("1 + $"),
            ParseError {
                at: 4,
                what: Problem::Stray
            }
        );
        assert_eq!(
            error("wobble(x)"),
            ParseError {
                at: 0,
                what: Problem::Unknown
            }
        );
        assert_eq!(
            error("1 + sin(x"),
            ParseError {
                at: 4,
                what: Problem::Unclosed
            }
        );
        assert_eq!(
            error("min(1)"),
            ParseError {
                at: 0,
                what: Problem::Arity
            }
        );
        assert_eq!(
            error("1 +"),
            ParseError {
                at: 3,
                what: Problem::Ended
            }
        );
        assert_eq!(
            error("1 2"),
            ParseError {
                at: 2,
                what: Problem::Trailing
            }
        );
        assert_eq!(error("").what, Problem::Ended);
    }

    /// Kept in step with the waveform panel's literal by eye.
    #[test]
    fn the_default_shape_moves_with_time() {
        const DEFAULT: &str = "0.5 * sin(6.28 * x - 1.2 * t) + 0.25 * sin(12.6 * x + 0.7 * t)";

        let expr = Expr::parse(DEFAULT).expect("the default parses");
        let across = |t: f32| {
            (0..64)
                .map(|i| expr.eval(i as f32 / 63.0, t))
                .collect::<Vec<_>>()
        };

        let still = across(0.0);
        let later = across(1.0);
        assert!(
            still.iter().all(|v| v.is_finite() && v.abs() <= 1.0),
            "the default stays inside the strip it draws in"
        );
        assert!(
            still.iter().zip(&later).any(|(a, b)| (a - b).abs() > 0.05),
            "a second on is a different shape"
        );
        assert!(
            still.iter().any(|v| *v > 0.1) && still.iter().any(|v| *v < -0.1),
            "and it's a wave rather than a flat line"
        );
    }

    #[test]
    fn nesting_past_the_cap_is_refused() {
        let deep = format!("{}1{}", "(".repeat(200), ")".repeat(200));
        assert_eq!(error(&deep).what, Problem::TooDeep);
    }

    #[test]
    fn an_expression_past_the_cap_is_refused() {
        let long = "1+".repeat(MAX_LEN) + "1";
        let Err(err) = Expr::parse(&long) else {
            panic!("a {}-byte expression parsed", long.len());
        };
        assert_eq!(err.what, Problem::TooLong);
        assert_eq!(err.at, MAX_LEN);

        let fits = "1+".repeat(MAX_LEN / 2 - 1) + "1";
        assert!(Expr::parse(&fits).is_ok());
    }
}
