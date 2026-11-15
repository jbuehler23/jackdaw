use std::fmt::Display;

use crate::syntax::lex::{LexError, Token};

/// A reference to a range of characters within a source string.
///
/// See also: [`OwnedSpan`]
#[derive(Clone, Debug)]
pub struct Span<'a> {
    /// The fragment of the input that this span refers to.
    pub fragment: &'a str,
    /// The position of the fragment relative to the input of the parser. Starts at 0.
    pub offset: usize,
    /// The line number of the fragment relative to the input of the parser. Starts at 1.
    pub line: u32,
}

/// An owned reference to a range of characters within a source string.
///
/// See also: [`Span`]
#[derive(Clone, Debug)]
pub struct OwnedSpan {
    /// The fragment of the input that this span refers to.
    pub fragment: String,
    /// The position of the fragment relative to the input of the parser. Starts at 0.
    pub offset: usize,
    /// The line number of the fragment relative to the input of the parser. Starts at 1.
    pub line: u32,
}

impl OwnedSpan {
    /// Borrows the owned span as an equivalent [`Span`].
    pub fn as_span(&self) -> Span<'_> {
        Span {
            fragment: &self.fragment,
            offset: self.offset,
            line: self.line,
        }
    }
}

impl<'a> From<Span<'a>> for OwnedSpan {
    fn from(value: Span<'a>) -> Self {
        Self {
            fragment: value.fragment.to_owned(),
            line: value.line,
            offset: value.offset,
        }
    }
}

impl Display for Span<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.fragment.fmt(f)
    }
}

impl<'a> From<&'a str> for Span<'a> {
    fn from(value: &'a str) -> Self {
        Self {
            fragment: value,
            offset: 0,
            line: 1,
        }
    }
}

impl<'a> Span<'a> {
    /// Formats a message string that references this span.
    pub fn message(&self, message: &str) -> String {
        format!("{message}. Line {}: {}", self.line, self.fragment)
    }

    /// Moves the start of the span `len` bytes on, counting the lines it
    /// passes.
    pub(crate) fn advance(&mut self, len: usize) {
        let (consumed, rest) = self.fragment.split_at(len);
        self.line += consumed.bytes().filter(|&b| b == b'\n').count() as u32;
        self.offset += len;
        self.fragment = rest;
    }

    /// The part of the span at `range`, a byte range on its first line.
    pub(crate) fn sub(&self, range: std::ops::Range<usize>) -> Self {
        Self {
            offset: self.offset + range.start,
            fragment: &self.fragment[range],
            line: self.line,
        }
    }

    /// Lex all tokens in this [`Span`].
    pub fn lex_all(self) -> Result<Vec<Token>, LexError> {
        let mut tokens = Vec::new();
        for token in self {
            tokens.push(token?.token);
        }
        Ok(tokens)
    }
}

impl<'a> core::ops::Deref for Span<'a> {
    type Target = &'a str;
    fn deref(&self) -> &Self::Target {
        &self.fragment
    }
}
