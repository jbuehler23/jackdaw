use std::ops::Deref;

use nom::{
    IResult, Input, Parser,
    branch::alt,
    bytes::complete::{tag, tag_no_case, take_while, take_while1},
    character::complete::{alpha1, alphanumeric1, char, digit1, multispace1, none_of, one_of},
    combinator::{not, opt, recognize, value},
    multi::{many0, many0_count},
    sequence::{delimited, pair, terminated},
};
use thiserror::Error;

use crate::syntax::span::Span;

/// A BSN token.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    /// Boolean value: true, false.
    Bool(bool),
    /// Identifier: `Foo`, `bar`, `Baz12`, `FooBar`
    Ident(String),
    /// String literal: "hello world"
    StringLit(String),
    /// Integer: 1, 10, -10
    Int(i128),
    /// Floating point: 0.1, .1, 10.1, and the `inf` / `NaN` spellings.
    Float(f64),
    /// Left bracket: [
    LBracket,
    /// Right bracket: ]
    RBracket,
    /// Left Paren: (
    LParen,
    /// Right Paren: )
    RParen,
    /// Left Brace: {
    LBrace,
    /// Right Brace: }
    RBrace,
    /// Comma: ,
    Comma,
    /// Double Colon: ::
    DoubleColon,
    /// Colon: :
    Colon,
    /// Double Minus: --
    DoubleMinus,
    /// Minus: -
    Minus,
    /// Hash: #
    Hash,
    /// At: @
    At,
    /// Less: <, opening a type path's generic arguments.
    Less,
    /// Greater: >, closing them. Lexed one character at a time, so the `>>`
    /// that ends a nested generic is two closers rather than a shift.
    Greater,
}

impl Token {
    fn spanned(self, span: Span<'_>) -> SpannedToken<'_> {
        SpannedToken { span, token: self }
    }
}

/// A [`Token`] coupled with the [`Span`] that produced it.
#[derive(Debug, Clone)]
pub struct SpannedToken<'a> {
    /// A [`Token`].
    pub token: Token,
    /// The [`Span`] that produced the [`Token`].
    pub span: Span<'a>,
}

impl Deref for SpannedToken<'_> {
    type Target = Token;

    #[inline]
    fn deref(&self) -> &Self::Target {
        &self.token
    }
}

fn lex_token(input: Span<'_>) -> IResult<Span<'_>, SpannedToken<'_>> {
    // `--` and the signed numbers come before `-`; the words before `lex_ident`.
    alt((
        lex_double_minus,
        lex_special_float,
        lex_true,
        lex_false,
        lex_ident,
        lex_string,
        lex_float,
        lex_int,
        lex_punctuation,
    ))
    .parse(input)
}

fn lex_punctuation(input: Span<'_>) -> IResult<Span<'_>, SpannedToken<'_>> {
    alt((
        lex_l_brace,
        lex_r_brace,
        lex_l_paren,
        lex_r_paren,
        lex_l_bracket,
        lex_r_bracket,
        lex_comma,
        lex_double_colon,
        lex_colon,
        lex_minus,
        lex_hash,
        lex_at,
        lex_less,
        lex_greater,
    ))
    .parse(input)
}

/// Fails when an identifier character follows, so a word only matches whole.
fn word_end(input: Span<'_>) -> IResult<Span<'_>, ()> {
    not(one_of(
        "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789_",
    ))
    .parse(input)
}

fn lex_true(input: Span<'_>) -> IResult<Span<'_>, SpannedToken<'_>> {
    let (rest, value) = terminated(tag("true"), word_end).parse(input)?;
    Ok((rest, Token::Bool(true).spanned(value)))
}

fn lex_false(input: Span<'_>) -> IResult<Span<'_>, SpannedToken<'_>> {
    let (rest, value) = terminated(tag("false"), word_end).parse(input)?;
    Ok((rest, Token::Bool(false).spanned(value)))
}

/// `inf`, `-inf`, `NaN`, `-NaN`, and the `Infinity` / `nan` spellings the
/// reflect serializer also writes. Whole words only, so `information` stays
/// an identifier.
fn lex_special_float(input: Span<'_>) -> IResult<Span<'_>, SpannedToken<'_>> {
    let (rest, raw) = recognize((
        opt(char('-')),
        alt((tag("Infinity"), tag("inf"), tag("NaN"), tag("nan"))),
        word_end,
    ))
    .parse(input)?;
    let (negative, word) = match raw.fragment.strip_prefix('-') {
        Some(word) => (true, word),
        None => (false, raw.fragment),
    };
    let magnitude = if word.eq_ignore_ascii_case("nan") {
        f64::NAN
    } else {
        f64::INFINITY
    };
    let value = if negative { -magnitude } else { magnitude };
    Ok((rest, Token::Float(value).spanned(raw)))
}

fn lex_ident(input: Span<'_>) -> IResult<Span<'_>, SpannedToken<'_>> {
    let (rest, value) = recognize(pair(
        alt((alpha1, tag("_"))),
        many0_count(alt((alphanumeric1, tag("_")))),
    ))
    .parse(input)?;
    Ok((rest, Token::Ident(value.fragment.to_owned()).spanned(value)))
}

macro_rules! lex_symbol {
    ($name:ident, $symbol:literal, $token:expr) => {
        fn $name(input: Span<'_>) -> IResult<Span<'_>, SpannedToken<'_>> {
            let (rest, value) = tag($symbol)(input)?;
            Ok((rest, $token.spanned(value)))
        }
    };
}

lex_symbol!(lex_l_bracket, "[", Token::LBracket);
lex_symbol!(lex_r_bracket, "]", Token::RBracket);
lex_symbol!(lex_l_paren, "(", Token::LParen);
lex_symbol!(lex_r_paren, ")", Token::RParen);
lex_symbol!(lex_l_brace, "{", Token::LBrace);
lex_symbol!(lex_r_brace, "}", Token::RBrace);
lex_symbol!(lex_comma, ",", Token::Comma);
lex_symbol!(lex_colon, ":", Token::Colon);
lex_symbol!(lex_double_colon, "::", Token::DoubleColon);
lex_symbol!(lex_minus, "-", Token::Minus);
lex_symbol!(lex_double_minus, "--", Token::DoubleMinus);
lex_symbol!(lex_hash, "#", Token::Hash);
lex_symbol!(lex_at, "@", Token::At);
lex_symbol!(lex_less, "<", Token::Less);
lex_symbol!(lex_greater, ">", Token::Greater);

fn lex_int(input: Span<'_>) -> IResult<Span<'_>, SpannedToken<'_>> {
    let start = input.clone();
    let (rest, (sign, radix_prefix, digits)) = (
        opt(alt((tag("+"), tag("-")))),
        opt(tag_no_case("0x")),
        alt((
            take_while1(|c: char| c.is_ascii_hexdigit()),
            take_while1(|c: char| c.is_ascii_digit()),
        )),
    )
        .parse(input.clone())?;

    let is_hex = radix_prefix.is_some();
    let is_negative = sign.map(|s| s.fragment) == Some("-");
    let radix = if is_hex { 16 } else { 10 };

    // Make sure decimal numbers have no hex digits.
    if !is_hex
        && digits
            .fragment
            .chars()
            .any(|c| c.is_ascii_hexdigit() && !c.is_ascii_digit())
    {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Digit,
        )));
    }

    let int_span = Input::take(&start, (digits.offset - start.offset) + digits.len());
    match i128::from_str_radix(digits.fragment, radix) {
        Ok(integer) if is_negative => Ok((rest, Token::Int(-integer).spanned(int_span))),
        Ok(integer) => Ok((rest, Token::Int(integer).spanned(int_span))),
        Err(_) => Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Digit,
        ))),
    }
}

fn lex_float(input: Span<'_>) -> IResult<Span<'_>, SpannedToken<'_>> {
    let (rest, raw) = recognize((
        opt(one_of("+-")),
        opt(digit1),
        char('.'),
        opt(digit1),
        opt((one_of("eE"), opt(one_of("+-")), digit1)),
    ))
    .parse(input.clone())?;

    // `.` isn't a valid number.
    if raw.fragment == "." {
        return Err(nom::Err::Error(nom::error::Error::new(
            input,
            nom::error::ErrorKind::Float,
        )));
    }

    let value: f64 = raw.fragment.parse().map_err(|_| {
        nom::Err::Error(nom::error::Error::new(input, nom::error::ErrorKind::Float))
    })?;

    Ok((rest, Token::Float(value).spanned(raw)))
}

/// `/* ... */`, nesting like Rust's block comments.
fn lex_block_comment(input: Span<'_>) -> IResult<Span<'_>, ()> {
    value(
        (),
        delimited(
            tag("/*"),
            many0(alt((
                value((), lex_block_comment),
                value((), none_of("*/")),
                value((), terminated(char('*'), not(char('/')))),
                value((), terminated(char('/'), not(char('*')))),
            ))),
            tag("*/"),
        ),
    )
    .parse(input)
}

fn lex_line_comment(input: Span<'_>) -> IResult<Span<'_>, ()> {
    value((), pair(tag("//"), take_while(|c: char| c != '\n'))).parse(input)
}

/// Whitespace and comments, which carry no tokens.
fn lex_whitespace(input: Span<'_>) -> IResult<Span<'_>, ()> {
    value(
        (),
        many0(alt((
            value((), multispace1),
            lex_line_comment,
            lex_block_comment,
        ))),
    )
    .parse(input)
}

fn lex_string(input: Span<'_>) -> IResult<Span<'_>, SpannedToken<'_>> {
    let (rest, (value_span, value)) = delimited(char('"'), string_body, char('"')).parse(input)?;
    return Ok((rest, Token::StringLit(value).spanned(value_span)));

    fn string_body(mut input: Span<'_>) -> IResult<Span<'_>, (Span<'_>, String)> {
        let mut out = String::new();
        let start = input.clone();
        loop {
            let (rest, chunk) = take_while(|c: char| c != '\\' && c != '"')(input)?;
            out.push_str(chunk.fragment);
            input = rest;

            if input.starts_with('"') || input.is_empty() {
                break;
            }

            let (rest, _) = char('\\')(input)?;
            if rest.is_empty() {
                out.push('\\');
                input = rest;
                break;
            }
            let esc_char = rest.chars().next().unwrap();
            let rest = rest.take_from(esc_char.len_utf8());
            match esc_char {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                '0' => out.push('\0'),
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                'x' => {
                    if rest.len() >= 2
                        && let Some(hex) = rest.get(..2)
                        && let Ok(byte) = u8::from_str_radix(hex, 16)
                    {
                        out.push(byte as char);
                        input = rest.take_from(2);
                        continue;
                    }
                    out.push('\\');
                    out.push('x');
                    input = rest;
                    continue;
                }
                other => {
                    out.push('\\');
                    out.push(other);
                }
            }
            input = rest;
        }

        let span_size = input.offset - start.offset;
        Ok((input, (Input::take(&start, span_size), out)))
    }
}

/// An error that occurs when lexing BSN.
#[derive(Error, Debug, Clone)]
pub enum LexError {
    /// Occurs when an unexpected character is encountered during lexing.
    #[error("Unexpected character: {0}")]
    UnexpectedChar(char),
}

impl<'a> Iterator for Span<'a> {
    type Item = Result<SpannedToken<'a>, LexError>;

    fn next(&mut self) -> Option<Self::Item> {
        let (rest, ()) = lex_whitespace(self.clone()).ok()?;
        *self = rest;
        if self.fragment.is_empty() {
            return None;
        }

        let start = self.clone();
        match lex_token(start.clone()) {
            Ok((rest, token)) => {
                *self = rest;
                Some(Ok(token))
            }
            Err(_) => {
                let c = start.fragment.chars().next().unwrap();
                // Stop here: the caller sees one error, not one per character.
                self.fragment = "";
                Some(Err(LexError::UnexpectedChar(c)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::syntax::lex::Token;
    use crate::syntax::span::Span;

    fn tokens(input: &str) -> Vec<Token> {
        Span::from(input).lex_all().unwrap()
    }

    #[test]
    fn lexes_every_token_kind() {
        assert_eq!(
            tokens(
                r#"true false Foo foo_bar1 Foo2Bar { }
                ( ) [ ] :: : , # @ 1 10 0.1 1.0 10.1
                "hello" "a \" \t"
                - -- < >"#
            ),
            vec![
                Token::Bool(true),
                Token::Bool(false),
                Token::Ident("Foo".to_string()),
                Token::Ident("foo_bar1".to_string()),
                Token::Ident("Foo2Bar".to_string()),
                Token::LBrace,
                Token::RBrace,
                Token::LParen,
                Token::RParen,
                Token::LBracket,
                Token::RBracket,
                Token::DoubleColon,
                Token::Colon,
                Token::Comma,
                Token::Hash,
                Token::At,
                Token::Int(1),
                Token::Int(10),
                Token::Float(0.1),
                Token::Float(1.0),
                Token::Float(10.1),
                Token::StringLit("hello".to_string()),
                Token::StringLit("a \" \t".to_string()),
                Token::Minus,
                Token::DoubleMinus,
                Token::Less,
                Token::Greater,
            ]
        );
    }

    #[test]
    fn a_double_minus_separates_entities_and_a_negative_number_follows_it() {
        assert_eq!(
            tokens("A -- -5 -0.5"),
            vec![
                Token::Ident("A".into()),
                Token::DoubleMinus,
                Token::Int(-5),
                Token::Float(-0.5),
            ]
        );
    }

    #[test]
    fn lexes_special_floats() {
        let floats: Vec<f64> = tokens("inf -inf Infinity NaN nan -NaN")
            .into_iter()
            .map(|t| match t {
                Token::Float(f) => f,
                other => panic!("expected a float, got {other:?}"),
            })
            .collect();
        assert!(floats[0].is_infinite() && floats[0] > 0.0);
        assert!(floats[1].is_infinite() && floats[1] < 0.0);
        assert!(floats[2].is_infinite() && floats[2] > 0.0);
        assert!(floats[3..].iter().all(|f| f.is_nan()));
    }

    #[test]
    fn identifiers_starting_with_reserved_words_stay_identifiers() {
        for word in [
            "information",
            "infinite_health",
            "nanometers",
            "NaNny",
            "trueish",
            "false_start",
        ] {
            assert_eq!(tokens(word), vec![Token::Ident(word.into())], "{word}");
        }
    }

    #[test]
    fn a_nested_generic_closes_with_two_separate_tokens() {
        assert_eq!(
            tokens("Vec<Vec<f32>>"),
            vec![
                Token::Ident("Vec".into()),
                Token::Less,
                Token::Ident("Vec".into()),
                Token::Less,
                Token::Ident("f32".into()),
                Token::Greater,
                Token::Greater,
            ],
        );
    }

    #[test]
    fn comments_lex_as_whitespace() {
        assert_eq!(
            tokens("// line\nA /* block /* nested */ still */ B // tail"),
            vec![Token::Ident("A".into()), Token::Ident("B".into())]
        );
    }

    #[test]
    fn spans_record_offsets_and_lines() {
        let tokens: Vec<_> = Span::from("A\n  --\nB").map(Result::unwrap).collect();
        assert_eq!(tokens[1].span.offset, 4);
        assert_eq!(tokens[1].span.line, 2);
        assert_eq!(tokens[2].span.line, 3);
    }
}
