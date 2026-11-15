use std::ops::Deref;

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
    /// Floating point: 0.1, .1, 10.1, and the `-inf` / `-NaN` spellings.
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

/// A token read from the start of some text: the token, the byte range its
/// span covers, and how many bytes it consumes. A string's span is its body
/// without the quotes.
struct Lexed {
    token: Token,
    span: std::ops::Range<usize>,
    len: usize,
}

impl Lexed {
    fn whole(token: Token, len: usize) -> Self {
        Self {
            token,
            span: 0..len,
            len,
        }
    }
}

fn is_word_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// True when no identifier character follows byte `at`, so a word only
/// matches whole.
fn word_ends_at(bytes: &[u8], at: usize) -> bool {
    !bytes.get(at).copied().is_some_and(is_word_byte)
}

fn count_while(bytes: &[u8], from: usize, mut pred: impl FnMut(u8) -> bool) -> usize {
    bytes[from.min(bytes.len())..]
        .iter()
        .take_while(|&&b| pred(b))
        .count()
}

/// Reads one token from the start of `text`, which holds no leading
/// whitespace. `None` when no token starts there.
fn lex_token(text: &str) -> Option<Lexed> {
    let bytes = text.as_bytes();
    let punctuation = |token| Some(Lexed::whole(token, 1));
    match *bytes.first()? {
        b'-' if bytes.get(1) == Some(&b'-') => Some(Lexed::whole(Token::DoubleMinus, 2)),
        b'-' => lex_special_float(text)
            .or_else(|| lex_number(text))
            .or_else(|| punctuation(Token::Minus)),
        b'a'..=b'z' | b'A'..=b'Z' | b'_' => Some(lex_ident(text)),
        b'"' => lex_string(text),
        b'0'..=b'9' | b'.' | b'+' => lex_number(text),
        b'{' => punctuation(Token::LBrace),
        b'}' => punctuation(Token::RBrace),
        b'(' => punctuation(Token::LParen),
        b')' => punctuation(Token::RParen),
        b'[' => punctuation(Token::LBracket),
        b']' => punctuation(Token::RBracket),
        b',' => punctuation(Token::Comma),
        b':' if bytes.get(1) == Some(&b':') => Some(Lexed::whole(Token::DoubleColon, 2)),
        b':' => punctuation(Token::Colon),
        b'#' => punctuation(Token::Hash),
        b'@' => punctuation(Token::At),
        b'<' => punctuation(Token::Less),
        b'>' => punctuation(Token::Greater),
        _ => None,
    }
}

/// `-inf`, `-NaN`, and the `-Infinity` / `-nan` spellings the reflect
/// serializer also writes. Whole words only, so `-information` is not one.
///
/// Unsigned, these words lex as identifiers, since a field, a variant or a
/// path segment may be named `inf` or `Infinity`; the parser reads one as a
/// float only where a value stands alone (see [`special_float`]).
fn lex_special_float(text: &str) -> Option<Lexed> {
    let word = ["Infinity", "inf", "NaN", "nan"]
        .into_iter()
        .find(|word| text[1..].starts_with(word))?;
    let len = 1 + word.len();
    if !word_ends_at(text.as_bytes(), len) {
        return None;
    }
    let value = special_float(word).map_or(f64::NAN, |magnitude| -magnitude);
    Some(Lexed::whole(Token::Float(value), len))
}

/// The float an unsigned `inf`, `Infinity`, `NaN` or `nan` stands for.
pub(crate) fn special_float(word: &str) -> Option<f64> {
    match word {
        "inf" | "Infinity" => Some(f64::INFINITY),
        "NaN" | "nan" => Some(f64::NAN),
        _ => None,
    }
}

fn lex_ident(text: &str) -> Lexed {
    let len = 1 + count_while(text.as_bytes(), 1, is_word_byte);
    let token = match &text[..len] {
        "true" => Token::Bool(true),
        "false" => Token::Bool(false),
        word => Token::Ident(word.to_owned()),
    };
    Lexed::whole(token, len)
}

fn lex_number(text: &str) -> Option<Lexed> {
    lex_float(text).or_else(|| lex_int(text))
}

/// An optional sign, then digits with a `.` among them and an optional
/// exponent: `1.`, `.5`, `-1.5e3`.
fn lex_float(text: &str) -> Option<Lexed> {
    let bytes = text.as_bytes();
    let sign = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let point = sign + count_while(bytes, sign, |b| b.is_ascii_digit());
    if bytes.get(point) != Some(&b'.') {
        return None;
    }
    let mut len = point + 1 + count_while(bytes, point + 1, |b| b.is_ascii_digit());
    if matches!(bytes.get(len), Some(b'e' | b'E')) {
        let exponent_sign = usize::from(matches!(bytes.get(len + 1), Some(b'+' | b'-')));
        let digits_at = len + 1 + exponent_sign;
        let digits = count_while(bytes, digits_at, |b| b.is_ascii_digit());
        if digits > 0 {
            len = digits_at + digits;
        }
    }
    let raw = &text[..len];
    if raw == "." {
        return None;
    }
    let value: f64 = raw.parse().ok()?;
    Some(Lexed::whole(Token::Float(value), len))
}

/// An optional sign and `0x`, then the longest run of hex digits, which must
/// all be decimal digits without the `0x`.
fn lex_int(text: &str) -> Option<Lexed> {
    let bytes = text.as_bytes();
    let sign = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let is_negative = bytes.first() == Some(&b'-');
    let is_hex = bytes.get(sign) == Some(&b'0') && matches!(bytes.get(sign + 1), Some(b'x' | b'X'));
    let digits_at = sign + if is_hex { 2 } else { 0 };
    let len = digits_at + count_while(bytes, digits_at, |b| b.is_ascii_hexdigit());
    let digits = &text[digits_at..len];
    if digits.is_empty() || (!is_hex && !digits.bytes().all(|b| b.is_ascii_digit())) {
        return None;
    }
    let integer = i128::from_str_radix(digits, if is_hex { 16 } else { 10 }).ok()?;
    let value = if is_negative { -integer } else { integer };
    Some(Lexed::whole(Token::Int(value), len))
}

/// `"..."` with the escapes `\n \r \t \0 \\ \" \xNN`; any other escape keeps
/// its backslash. `None` when the closing quote is missing.
fn lex_string(text: &str) -> Option<Lexed> {
    let bytes = text.as_bytes();
    let mut out = String::new();
    let mut at = 1;
    loop {
        let chunk = count_while(bytes, at, |b| b != b'\\' && b != b'"');
        out.push_str(&text[at..at + chunk]);
        at += chunk;
        if at == bytes.len() || bytes[at] == b'"' {
            break;
        }
        at += 1;
        let Some(escaped) = text[at..].chars().next() else {
            out.push('\\');
            break;
        };
        at += escaped.len_utf8();
        match escaped {
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            '0' => out.push('\0'),
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            'x' => {
                if let Some(hex) = text[at..].get(..2)
                    && let Ok(byte) = u8::from_str_radix(hex, 16)
                {
                    out.push(byte as char);
                    at += 2;
                } else {
                    out.push_str("\\x");
                }
            }
            other => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    if bytes.get(at) != Some(&b'"') {
        return None;
    }
    Some(Lexed {
        token: Token::StringLit(out),
        span: 1..at,
        len: at + 1,
    })
}

/// The length of the `/* ... */` comment `text` starts with, nesting like
/// Rust's block comments. `None` when it never closes.
fn block_comment_len(text: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut at = 0;
    while at + 1 < bytes.len() {
        match (bytes[at], bytes[at + 1]) {
            (b'/', b'*') => {
                depth += 1;
                at += 2;
            }
            (b'*', b'/') => {
                depth -= 1;
                at += 2;
                if depth == 0 {
                    return Some(at);
                }
            }
            _ => at += 1,
        }
    }
    None
}

/// The length of the whitespace and comments `text` starts with.
fn whitespace_len(text: &str) -> usize {
    let bytes = text.as_bytes();
    let mut at = 0;
    while let Some(&byte) = bytes.get(at) {
        match (byte, bytes.get(at + 1)) {
            (b' ' | b'\t' | b'\r' | b'\n', _) => at += 1,
            (b'/', Some(b'/')) => at += 2 + count_while(bytes, at + 2, |b| b != b'\n'),
            (b'/', Some(b'*')) => match block_comment_len(&text[at..]) {
                Some(len) => at += len,
                None => break,
            },
            _ => break,
        }
    }
    at
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

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        self.advance(whitespace_len(self.fragment));
        let first = self.fragment.chars().next()?;
        match lex_token(self.fragment) {
            Some(Lexed { token, span, len }) => {
                let span = self.sub(span);
                self.advance(len);
                Some(Ok(SpannedToken { token, span }))
            }
            None => {
                // Stop here: the caller sees one error, not one per character.
                self.fragment = "";
                Some(Err(LexError::UnexpectedChar(first)))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::syntax::lex::{LexError, Token};
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
    fn lexes_signed_special_floats() {
        let floats: Vec<f64> = tokens("-inf -Infinity -NaN -nan")
            .into_iter()
            .map(|t| match t {
                Token::Float(f) => f,
                other => panic!("expected a float, got {other:?}"),
            })
            .collect();
        assert!(floats[..2].iter().all(|f| f.is_infinite() && *f < 0.0));
        assert!(floats[2..].iter().all(|f| f.is_nan()));
    }

    #[test]
    fn unsigned_special_float_words_lex_as_identifiers() {
        for word in ["inf", "Infinity", "NaN", "nan"] {
            assert_eq!(tokens(word), vec![Token::Ident(word.into())], "{word}");
        }
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

    fn lex(input: &str) -> Result<Vec<Token>, char> {
        Span::from(input).lex_all().map_err(|err| match err {
            LexError::UnexpectedChar(c) => c,
        })
    }

    /// Each token's span as its text, offset and line.
    fn spans(input: &str) -> Vec<(&str, usize, u32)> {
        Span::from(input)
            .map(|token| {
                let span = token.expect("the input lexes").span;
                (span.fragment, span.offset, span.line)
            })
            .collect()
    }

    #[test]
    fn numbers_lex_by_their_spelling() {
        assert_eq!(lex("1."), Ok(vec![Token::Float(1.0)]));
        assert_eq!(lex(".5"), Ok(vec![Token::Float(0.5)]));
        assert_eq!(lex("1.e2"), Ok(vec![Token::Float(100.0)]));
        assert_eq!(lex("-1.5E-1"), Ok(vec![Token::Float(-0.15)]));
        assert_eq!(lex("+1.5"), Ok(vec![Token::Float(1.5)]));
        assert_eq!(lex("+7"), Ok(vec![Token::Int(7)]));
        assert_eq!(lex("0x1F -0X1f"), Ok(vec![Token::Int(31), Token::Int(-31)]));
        assert_eq!(lex("007"), Ok(vec![Token::Int(7)]));
        assert_eq!(
            lex("1.5e"),
            Ok(vec![Token::Float(1.5), Token::Ident("e".into())])
        );
        assert_eq!(
            lex("5xyz"),
            Ok(vec![Token::Int(5), Token::Ident("xyz".into())])
        );
        assert_eq!(
            lex("--5 - 5"),
            Ok(vec![
                Token::DoubleMinus,
                Token::Int(5),
                Token::Minus,
                Token::Int(5)
            ])
        );
        assert_eq!(lex(&i128::MAX.to_string()), Ok(vec![Token::Int(i128::MAX)]));
    }

    #[test]
    fn malformed_numbers_are_lex_errors() {
        assert_eq!(lex("1e5"), Err('1'));
        assert_eq!(lex("12ab"), Err('1'));
        assert_eq!(lex("0xg"), Err('0'));
        assert_eq!(lex("170141183460469231731687303715884105728"), Err('1'));
        assert_eq!(lex("."), Err('.'));
        assert_eq!(lex("+"), Err('+'));
        assert_eq!(lex("-."), Err('.'));
        assert_eq!(lex("1.5e+"), Err('+'));
    }

    #[test]
    fn special_float_words_need_a_minus_and_a_word_end() {
        assert_eq!(
            lex("-infinity -Infinityx"),
            Ok(vec![
                Token::Minus,
                Token::Ident("infinity".into()),
                Token::Minus,
                Token::Ident("Infinityx".into()),
            ])
        );
        let tokens = lex("-inf]").unwrap();
        assert!(matches!(tokens[0], Token::Float(f) if f == f64::NEG_INFINITY));
        assert_eq!(tokens[1], Token::RBracket);
    }

    #[test]
    fn string_escapes_read_as_their_characters() {
        assert_eq!(
            lex(r#""\n\r\t\0\\\"" "\x41" "\xZZ" "\x4" "\q" "\é""#),
            Ok(vec![
                Token::StringLit("\n\r\t\0\\\"".into()),
                Token::StringLit("A".into()),
                Token::StringLit("\\xZZ".into()),
                Token::StringLit("\\x4".into()),
                Token::StringLit("\\q".into()),
                Token::StringLit("\\é".into()),
            ])
        );
        assert_eq!(
            lex("\"a\nb\" \"\u{e9}\u{4e2d}\""),
            Ok(vec![
                Token::StringLit("a\nb".into()),
                Token::StringLit("\u{e9}\u{4e2d}".into()),
            ])
        );
    }

    #[test]
    fn unterminated_strings_are_lex_errors() {
        assert_eq!(lex(r#"A "abc"#), Err('"'));
        assert_eq!(lex(r#""abc\"#), Err('"'));
        assert_eq!(lex(r#""abc\""#), Err('"'));
    }

    #[test]
    fn block_comments_nest_and_must_close() {
        assert_eq!(lex("/**/ A /***/"), Ok(vec![Token::Ident("A".into())]));
        assert_eq!(
            lex("/* //* */ */ A /* * / */"),
            Ok(vec![Token::Ident("A".into())])
        );
        assert_eq!(lex("A /* open"), Err('/'));
        assert_eq!(lex("/* a/*/ b */"), Err('/'));
        assert_eq!(lex("/*/"), Err('/'));
        assert_eq!(lex("/* a */ */"), Err('*'));
        assert_eq!(lex("a / b"), Err('/'));
        assert_eq!(
            lex("A // tail without a newline"),
            Ok(vec![Token::Ident("A".into())])
        );
    }

    #[test]
    fn only_ascii_spaces_tabs_and_newlines_are_whitespace() {
        assert_eq!(
            lex("A\r\n\tB"),
            Ok(vec![Token::Ident("A".into()), Token::Ident("B".into())])
        );
        assert_eq!(lex("A\u{c}B"), Err('\u{c}'));
        assert_eq!(lex("A\u{a0}B"), Err('\u{a0}'));
    }

    #[test]
    fn identifiers_are_ascii_words() {
        assert_eq!(
            lex("_ _9 a9_b true_ true]"),
            Ok(vec![
                Token::Ident("_".into()),
                Token::Ident("_9".into()),
                Token::Ident("a9_b".into()),
                Token::Ident("true_".into()),
                Token::Bool(true),
                Token::RBracket,
            ])
        );
        assert_eq!(lex("ab\u{e9}"), Err('\u{e9}'));
        assert_eq!(lex(":::"), Ok(vec![Token::DoubleColon, Token::Colon]));
    }

    #[test]
    fn spans_cover_each_token_as_written() {
        assert_eq!(
            spans("-0x1F -1.5e3 -inf\n\"a\nb\" /* x\n */ :: Foo"),
            vec![
                ("-0x1F", 0, 1),
                ("-1.5e3", 6, 1),
                ("-inf", 13, 1),
                ("a\nb", 19, 2),
                ("::", 33, 4),
                ("Foo", 36, 4),
            ]
        );
    }

    #[test]
    fn a_lex_error_ends_the_tokens() {
        let mut span = Span::from("A $ B");
        assert!(matches!(span.next(), Some(Ok(_))));
        assert!(matches!(
            span.next(),
            Some(Err(LexError::UnexpectedChar('$')))
        ));
        assert!(span.next().is_none());
    }
}
