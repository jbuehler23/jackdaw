//! Recipe parameter expressions: numbers, `[..]` vectors, `$input` names,
//! `+ - * /`, `min`, `max`, `clamp`, `lerp` and a seeded `rand(lo, hi)`.
//!
//! A `$N.path` result reference is kept as it is, for the caller to fill once
//! call `N` has run. It stands alone, as the whole value or one list item;
//! arithmetic on it is refused, since an outcome holds ids rather than lengths.

use std::collections::BTreeMap;

/// What an expression evaluates to.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Number(f64),
    List(Vec<Value>),
    Bool(bool),
    Text(String),
    /// A `$N.path` reference, left for the caller to fill.
    Ref(String),
}

impl Value {
    fn kind(&self) -> &'static str {
        match self {
            Self::Number(_) => "a number",
            Self::List(_) => "a vector",
            Self::Bool(_) => "true or false",
            Self::Text(_) => "text",
            Self::Ref(_) => "a result reference",
        }
    }
}

/// The names an expression may read, and where it sits, for `rand`.
pub struct Scope<'a> {
    pub inputs: &'a BTreeMap<String, Value>,
    pub seed: u64,
    /// Names the parameter being evaluated, e.g. `3.size`, so each `rand` in a
    /// recipe draws its own number and adding a call elsewhere changes none.
    pub site: &'a str,
}

/// Whether parameter text is an expression rather than literal text: it
/// starts with `=`, or it reads as a whole expression in which every name is a
/// function call, `true` or `false`. So `4 * 2`, `-1 + 3`, `$width` and
/// `[1, 2]` are expressions, while `cylinder`, `Inf`, `#roof`, `$$5` and the
/// comma list `1, 2, 3` stay text.
pub fn is_expression(text: &str) -> bool {
    let source = text.trim_start();
    if source.starts_with('=') {
        return true;
    }
    let inputs = BTreeMap::new();
    let scope = Scope {
        inputs: &inputs,
        seed: 0,
        site: "",
    };
    parse(source, &scope, true).is_ok()
}

/// Evaluate `text`. A leading `=` is dropped.
pub fn eval(text: &str, scope: &Scope) -> Result<Value, String> {
    let source = text.trim_start();
    let source = source.strip_prefix('=').unwrap_or(source);
    parse(source, scope, false).map_err(|why| match why {
        Unexpected(token) => format!("`{text}`: unexpected {token} after the expression"),
        Other(why) => why,
    })
}

enum ParseError {
    Unexpected(String),
    Other(String),
}

use ParseError::{Other, Unexpected};

/// Read `source` as one expression. `check` reads its shape alone: names are
/// not looked up and nothing is computed.
fn parse(source: &str, scope: &Scope, check: bool) -> Result<Value, ParseError> {
    let tokens = lex(source).map_err(Other)?;
    if tokens.is_empty() {
        return Err(Other("the expression is empty".to_string()));
    }
    let mut parser = Parser {
        tokens,
        at: 0,
        scope,
        draws: 0,
        check,
    };
    let value = parser.expr().map_err(Other)?;
    if parser.at != parser.tokens.len() {
        return Err(Unexpected(parser.tokens[parser.at].describe()));
    }
    Ok(value)
}

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Number(f64),
    Input(String),
    Ref(String),
    Ident(String),
    Symbol(char),
}

impl Token {
    fn describe(&self) -> String {
        match self {
            Self::Number(number) => format!("`{number}`"),
            Self::Input(name) => format!("`${name}`"),
            Self::Ref(text) | Self::Ident(text) => format!("`{text}`"),
            Self::Symbol(c) => format!("`{c}`"),
        }
    }
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn lex(text: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut tokens = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        if c.is_whitespace() {
            at += 1;
        } else if c.is_ascii_digit()
            || (c == '.' && chars.get(at + 1).is_some_and(char::is_ascii_digit))
        {
            let start = at;
            while at < chars.len() && (chars[at].is_ascii_digit() || chars[at] == '.') {
                at += 1;
            }
            if at < chars.len() && matches!(chars[at], 'e' | 'E') {
                let mut end = at + 1;
                if end < chars.len() && matches!(chars[end], '+' | '-') {
                    end += 1;
                }
                if end < chars.len() && chars[end].is_ascii_digit() {
                    at = end;
                    while at < chars.len() && chars[at].is_ascii_digit() {
                        at += 1;
                    }
                }
            }
            let spelled: String = chars[start..at].iter().collect();
            let number = spelled
                .parse::<f64>()
                .map_err(|_| format!("`{spelled}` is not a number"))?;
            tokens.push(Token::Number(number));
        } else if c == '$' {
            let start = at;
            at += 1;
            if chars.get(at).is_some_and(char::is_ascii_digit) {
                while at < chars.len() && (is_name_char(chars[at]) || chars[at] == '.') {
                    at += 1;
                }
                tokens.push(Token::Ref(chars[start..at].iter().collect()));
            } else {
                let name_start = at;
                while at < chars.len() && is_name_char(chars[at]) {
                    at += 1;
                }
                if at == name_start {
                    return Err("`$` names no input; write `$name`".to_string());
                }
                tokens.push(Token::Input(chars[name_start..at].iter().collect()));
            }
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = at;
            while at < chars.len() && is_name_char(chars[at]) {
                at += 1;
            }
            tokens.push(Token::Ident(chars[start..at].iter().collect()));
        } else if "+-*/()[],".contains(c) {
            tokens.push(Token::Symbol(c));
            at += 1;
        } else {
            return Err(format!("`{c}` has no meaning in an expression"));
        }
    }
    Ok(tokens)
}

struct Parser<'a> {
    tokens: Vec<Token>,
    at: usize,
    scope: &'a Scope<'a>,
    draws: u64,
    check: bool,
}

impl Parser<'_> {
    fn arith(&self, a: Value, b: Value, op: char) -> Result<Value, String> {
        if self.check {
            return Ok(Value::Number(0.0));
        }
        arith(a, b, op)
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn eat(&mut self, symbol: char) -> bool {
        if self.peek() == Some(&Token::Symbol(symbol)) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, symbol: char) -> Result<(), String> {
        if self.eat(symbol) {
            return Ok(());
        }
        Err(match self.peek() {
            Some(token) => format!("expected `{symbol}`, found {}", token.describe()),
            None => format!("expected `{symbol}` before the end"),
        })
    }

    fn expr(&mut self) -> Result<Value, String> {
        let mut value = self.term()?;
        loop {
            if self.eat('+') {
                let next = self.term()?;
                value = self.arith(value, next, '+')?;
            } else if self.eat('-') {
                let next = self.term()?;
                value = self.arith(value, next, '-')?;
            } else {
                return Ok(value);
            }
        }
    }

    fn term(&mut self) -> Result<Value, String> {
        let mut value = self.unary()?;
        loop {
            if self.eat('*') {
                let next = self.unary()?;
                value = self.arith(value, next, '*')?;
            } else if self.eat('/') {
                let next = self.unary()?;
                value = self.arith(value, next, '/')?;
            } else {
                return Ok(value);
            }
        }
    }

    fn unary(&mut self) -> Result<Value, String> {
        if self.eat('-') {
            let value = self.unary()?;
            return self.arith(Value::Number(0.0), value, '-');
        }
        if self.eat('+') {
            return self.unary();
        }
        self.primary()
    }

    fn primary(&mut self) -> Result<Value, String> {
        let Some(token) = self.peek().cloned() else {
            return Err("the expression ends early".to_string());
        };
        self.at += 1;
        match token {
            Token::Number(number) => Ok(Value::Number(number)),
            Token::Input(_) if self.check => Ok(Value::Number(0.0)),
            Token::Input(name) => self
                .scope
                .inputs
                .get(&name)
                .cloned()
                .ok_or_else(|| format!("`${name}` is not an input of this recipe")),
            Token::Ref(text) => Ok(Value::Ref(text)),
            Token::Symbol('(') => {
                let value = self.expr()?;
                self.expect(')')?;
                Ok(value)
            }
            Token::Symbol('[') => {
                let mut items = Vec::new();
                if !self.eat(']') {
                    loop {
                        items.push(self.expr()?);
                        if self.eat(']') {
                            break;
                        }
                        self.expect(',')?;
                    }
                }
                Ok(Value::List(items))
            }
            Token::Ident(name) => self.call(&name),
            Token::Symbol(c) => Err(format!("unexpected `{c}`")),
        }
    }

    fn call(&mut self, name: &str) -> Result<Value, String> {
        if name == "true" || name == "false" {
            return Ok(Value::Bool(name == "true"));
        }
        if !self.eat('(') {
            return Err(format!(
                "`{name}` is not a function or a value; an input is written `${name}`"
            ));
        }
        let mut args = Vec::new();
        if !self.eat(')') {
            loop {
                args.push(self.expr()?);
                if self.eat(')') {
                    break;
                }
                self.expect(',')?;
            }
        }
        if self.check {
            return Ok(Value::Number(0.0));
        }
        let arity = |want: usize| {
            if args.len() == want {
                Ok(())
            } else {
                Err(format!("`{name}` takes {want} values, not {}", args.len()))
            }
        };
        match name {
            "min" | "max" => {
                if args.len() < 2 {
                    return Err(format!("`{name}` takes two or more values"));
                }
                let op = if name == "min" { 'm' } else { 'M' };
                let mut args = args.into_iter();
                let first = args.next().unwrap_or(Value::Number(0.0));
                args.try_fold(first, |acc, next| arith(acc, next, op))
            }
            "clamp" => {
                arity(3)?;
                let [x, lo, hi]: [Value; 3] = args.try_into().map_err(|_| "clamp".to_string())?;
                arith(arith(x, lo, 'M')?, hi, 'm')
            }
            "lerp" => {
                arity(3)?;
                let [a, b, t]: [Value; 3] = args.try_into().map_err(|_| "lerp".to_string())?;
                let span = arith(b, a.clone(), '-')?;
                arith(a, arith(span, t, '*')?, '+')
            }
            "rand" => {
                arity(2)?;
                let (Value::Number(lo), Value::Number(hi)) = (&args[0], &args[1]) else {
                    return Err("`rand` takes two numbers".to_string());
                };
                let unit = draw(self.scope.seed, self.scope.site, self.draws);
                self.draws += 1;
                Ok(Value::Number(lo + (hi - lo) * unit))
            }
            other => Err(format!(
                "`{other}` is not a function; there are min, max, clamp, lerp and rand"
            )),
        }
    }
}

/// `a op b` over numbers, element by element over vectors of one length, a
/// number spread over every element of a vector. `m` and `M` are min and max.
fn arith(a: Value, b: Value, op: char) -> Result<Value, String> {
    match (a, b) {
        (Value::Number(a), Value::Number(b)) => {
            let out = match op {
                '+' => a + b,
                '-' => a - b,
                '*' => a * b,
                '/' if b == 0.0 => return Err("division by zero".to_string()),
                '/' => a / b,
                'm' => a.min(b),
                _ => a.max(b),
            };
            Ok(Value::Number(out))
        }
        (Value::List(a), Value::List(b)) => {
            if a.len() != b.len() {
                return Err(format!(
                    "a vector of {} and a vector of {} do not combine",
                    a.len(),
                    b.len()
                ));
            }
            a.into_iter()
                .zip(b)
                .map(|(a, b)| arith(a, b, op))
                .collect::<Result<_, _>>()
                .map(Value::List)
        }
        (Value::List(a), b @ Value::Number(_)) => a
            .into_iter()
            .map(|a| arith(a, b.clone(), op))
            .collect::<Result<_, _>>()
            .map(Value::List),
        (a @ Value::Number(_), Value::List(b)) => b
            .into_iter()
            .map(|b| arith(a.clone(), b, op))
            .collect::<Result<_, _>>()
            .map(Value::List),
        (a, b) => {
            let odd = if matches!(a, Value::Number(_) | Value::List(_)) {
                b
            } else {
                a
            };
            let what = match &odd {
                Value::Ref(text) => format!("`{text}`"),
                other => other.kind().to_string(),
            };
            Err(format!(
                "{what} cannot take part in arithmetic; numbers and vectors can"
            ))
        }
    }
}

/// A number in `[0, 1)` from the seed, the site and which draw at that site.
fn draw(seed: u64, site: &str, index: u64) -> f64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in site.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    let mixed = splitmix(seed ^ splitmix(hash ^ splitmix(index)));
    (mixed >> 11) as f64 / (1u64 << 53) as f64
}

fn splitmix(value: u64) -> u64 {
    let mut z = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> BTreeMap<String, Value> {
        [
            ("width".to_string(), Value::Number(6.0)),
            ("seed".to_string(), Value::Number(4.0)),
            (
                "size".to_string(),
                Value::List(vec![
                    Value::Number(1.0),
                    Value::Number(2.0),
                    Value::Number(3.0),
                ]),
            ),
            ("material".to_string(), Value::Text("stone".to_string())),
        ]
        .into_iter()
        .collect()
    }

    fn eval_at(text: &str, seed: u64, site: &str) -> Result<Value, String> {
        let inputs = inputs();
        eval(
            text,
            &Scope {
                inputs: &inputs,
                seed,
                site,
            },
        )
    }

    fn num(text: &str) -> f64 {
        match eval_at(text, 0, "0.x") {
            Ok(Value::Number(n)) => n,
            other => panic!("{text}: {other:?}"),
        }
    }

    fn list(values: &[f64]) -> Value {
        Value::List(values.iter().copied().map(Value::Number).collect())
    }

    #[test]
    fn arithmetic_follows_precedence_and_parentheses() {
        assert_eq!(num("$width * 0.5"), 3.0);
        assert_eq!(num("$seed + 1"), 5.0);
        assert_eq!(num("1 + 2 * 3"), 7.0);
        assert_eq!(num("(1 + 2) * 3"), 9.0);
        assert_eq!(num("-$width / 4"), -1.5);
        assert_eq!(num("= 10 - 4 - 3"), 3.0);
        assert_eq!(num("1.5e2"), 150.0);
    }

    #[test]
    fn vectors_combine_element_by_element_and_with_numbers() {
        let v = |text| eval_at(text, 0, "0.x").expect(text);
        assert_eq!(v("[1, $width, 2 * 2]"), list(&[1.0, 6.0, 4.0]));
        assert_eq!(v("$size * 2"), list(&[2.0, 4.0, 6.0]));
        assert_eq!(v("$size + [1, 1, 1]"), list(&[2.0, 3.0, 4.0]));
        assert_eq!(
            v("[[0, 0], [$width, 0]]"),
            Value::List(vec![list(&[0.0, 0.0]), list(&[6.0, 0.0])])
        );
        assert!(v("[]") == Value::List(Vec::new()));
        assert!(eval_at("$size + [1, 2]", 0, "0.x").is_err());
    }

    #[test]
    fn the_functions_compute_what_they_name() {
        assert_eq!(num("min(3, $width, 4)"), 3.0);
        assert_eq!(num("max(3, $width)"), 6.0);
        assert_eq!(num("clamp($width, 1, 4)"), 4.0);
        assert_eq!(num("clamp(-2, 1, 4)"), 1.0);
        assert_eq!(num("lerp(2, 4, 0.25)"), 2.5);
        assert_eq!(
            eval_at("lerp([0, 0], [2, 4], 0.5)", 0, "0.x").expect("lerp"),
            list(&[1.0, 2.0])
        );
        let r = num("rand(2, 3)");
        assert!((2.0..3.0).contains(&r), "{r}");
    }

    #[test]
    fn rand_is_fixed_by_seed_and_site_and_changes_with_either() {
        let at = |seed, site| eval_at("rand(0, 1)", seed, site).expect("rand");
        assert_eq!(at(7, "2.size"), at(7, "2.size"));
        assert_ne!(at(7, "2.size"), at(8, "2.size"));
        assert_ne!(at(7, "2.size"), at(7, "3.size"));
        let pair = eval_at("[rand(0, 1), rand(0, 1)]", 7, "2.size").expect("pair");
        let Value::List(items) = pair else {
            panic!("a list")
        };
        assert_ne!(items[0], items[1], "two draws at one site differ");
    }

    #[test]
    fn a_result_reference_stands_alone() {
        assert_eq!(
            eval_at("$0.entities.0", 0, "1.parent"),
            Ok(Value::Ref("$0.entities.0".to_string()))
        );
        assert_eq!(
            eval_at("[$0.entities.0, $2.entities.0]", 0, "3.entities"),
            Ok(Value::List(vec![
                Value::Ref("$0.entities.0".to_string()),
                Value::Ref("$2.entities.0".to_string()),
            ]))
        );
        let refused = eval_at("$0.bounds.0 + 1", 0, "1.x").expect_err("arithmetic");
        assert!(refused.contains("$0.bounds.0"), "{refused}");
    }

    #[test]
    fn text_is_used_whole_and_refused_in_arithmetic() {
        assert_eq!(
            eval_at("$material", 0, "0.material"),
            Ok(Value::Text("stone".to_string()))
        );
        assert!(eval_at("$material * 2", 0, "0.x").is_err());
    }

    #[test]
    fn mistakes_are_named() {
        for (text, says) in [
            ("$depth", "$depth"),
            ("width * 2", "width"),
            ("1 +", "ends early"),
            ("(1 + 2", "`)`"),
            ("1 / 0", "division by zero"),
            ("sqrt(4)", "sqrt"),
            ("rand(1)", "rand"),
            ("1 2", "unexpected"),
            ("1 % 2", "%"),
        ] {
            let err = eval_at(text, 0, "0.x").expect_err(text);
            assert!(err.contains(says), "{text}: {err}");
        }
    }

    #[test]
    fn whole_expressions_read_as_expressions_and_names_stay_text() {
        for text in [
            "$width",
            "= 3",
            "[1, 2, 3]",
            "2.5",
            "-1",
            "4 * 2",
            "-1 + 3",
            ".5",
            "(1 + 2) / 3",
            "min($width, 4)",
            "$0.entities.0",
            "[$0.entities.0, $1.entities.0]",
        ] {
            assert!(is_expression(text), "{text}");
        }
        for text in [
            "cylinder",
            "Inf",
            "nan",
            "-inf",
            "#roof",
            "$$5",
            "Roof Left",
            "a $b",
            "1, 2, 3",
            "Wall_Front",
            "",
        ] {
            assert!(!is_expression(text), "{text}");
        }
        let inputs = inputs();
        let scope = Scope {
            inputs: &inputs,
            seed: 0,
            site: "0.x",
        };
        assert_eq!(eval("4 * 2", &scope), Ok(Value::Number(8.0)));
        assert_eq!(eval("-1 + 3", &scope), Ok(Value::Number(2.0)));
    }
}
