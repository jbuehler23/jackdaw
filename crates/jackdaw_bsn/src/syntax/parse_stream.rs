use std::collections::VecDeque;
use std::fmt::Display;

use thiserror::Error;

use crate::syntax::{
    lex::{LexError, SpannedToken, Token},
    span::{OwnedSpan, Span},
    types::{LBrace, LBracket, LParen, RBrace, RBracket, RParen},
};

/// An error that occurs while parsing BSN.
#[derive(Error)]
pub enum ParseError {
    /// A lex error.
    #[error(transparent)]
    LexError(LexError),
    /// Occurs when an unexpected token is encountered.
    #[error("{}", .0.span.as_span().message(&format!("Unexpected Token {:?}", .0.token)))]
    UnexpectedToken(OwnedSpannedToken),
    /// Occurs when the end of input was reached unexpectedly.
    #[error("Unexpected end of input.")]
    EndOfInput,
    /// An error that occurred for a given spanned token.
    #[error("{}", .token.span.as_span().message(.message))]
    SpannedMessage {
        /// The message. This should provide helpful user-facing context for the error.
        message: String,
        /// The spanned token that produced the error.
        token: OwnedSpannedToken,
    },
    /// A span-less error occurred. Prefer [`ParseError::SpannedMessage`] when possible.
    #[error("{0}")]
    Message(String),
}

/// A [`Token`] coupled with the [`Span`] that produced it.
#[derive(Debug, Clone)]
pub struct OwnedSpannedToken {
    /// A [`Token`].
    pub token: Token,
    /// The [`Span`] that produced the [`Token`].
    pub span: OwnedSpan,
}

impl<'a> From<SpannedToken<'a>> for OwnedSpannedToken {
    fn from(value: SpannedToken<'a>) -> Self {
        Self {
            token: value.token,
            span: value.span.into(),
        }
    }
}

impl std::fmt::Debug for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        Display::fmt(self, f)
    }
}

impl From<LexError> for ParseError {
    fn from(value: LexError) -> Self {
        Self::LexError(value)
    }
}

/// A peek-able stream of tokens to be parsed.
pub struct ParseStream<'a> {
    span: Span<'a>,
    peek_buffer: VecDeque<Result<SpannedToken<'a>, LexError>>,
}

impl<'a> From<Span<'a>> for ParseStream<'a> {
    fn from(value: Span<'a>) -> Self {
        Self {
            span: value,
            peek_buffer: VecDeque::new(),
        }
    }
}

impl<'a> ParseStream<'a> {
    /// Try to parse the given type `P`.
    pub fn parse<P: Parse>(&mut self) -> Result<P, ParseError> {
        P::parse(self)
    }

    /// Peeks the given type `P`. Returns true if it is present. Otherwise returns false.
    pub fn peek<P: Peek>(&mut self) -> bool {
        P::peek(self)
    }

    /// Returns true if the parse stream is empty (no more tokens). A pending
    /// lex error is not the end of the input.
    pub fn is_empty(&mut self) -> bool {
        self.fill(0);
        self.peek_buffer.is_empty()
    }

    /// Returns true if the parse stream is empty or [`Token::RBracket`], [`Token::RParen`], or [`Token::RBrace`]
    /// are present.
    pub fn is_empty_or_closing_delimiter(&mut self) -> bool {
        self.fill(0);
        match self.peek_buffer.front() {
            None => true,
            Some(Ok(token)) => {
                matches!(token.token, Token::RBracket | Token::RParen | Token::RBrace)
            }
            Some(Err(_)) => false,
        }
    }

    /// Lexes ahead until the buffer holds token `n` (0 is the next one), or
    /// the input runs out.
    fn fill(&mut self, n: usize) {
        while self.peek_buffer.len() <= n {
            match self.span.next() {
                Some(result) => self.peek_buffer.push_back(result),
                None => break,
            }
        }
    }

    /// Peeks the next token. Returns [`Some`] if the token is present. Otherwise returns [`None`].
    pub fn peek_token(&mut self) -> Option<&SpannedToken<'a>> {
        self.peek_nth_token(0)
    }

    /// Peeks the token `n` places ahead (0 is the next one), like
    /// [`ParseStream::peek_token`].
    pub fn peek_nth_token(&mut self, n: usize) -> Option<&SpannedToken<'a>> {
        self.fill(n);
        self.peek_buffer
            .get(n)
            .and_then(|token| token.as_ref().ok())
    }

    /// Retrieves the next [`SpannedToken`] in the parse stream, advancing the position in the stream forward.
    pub fn next_token(&mut self) -> Result<Option<SpannedToken<'a>>, LexError> {
        if let Some(value) = self.peek_buffer.pop_front() {
            return value.map(Some);
        }
        self.span.next().transpose()
    }

    /// Returns a [`ParseError`] with the given `message`. If there is a next token in the stream,
    /// its [`Span`] will be used.
    pub fn error(&mut self, message: impl Into<String>) -> ParseError {
        match self.peek_token() {
            Some(token) => ParseError::SpannedMessage {
                message: message.into(),
                token: token.clone().into(),
            },
            None => ParseError::Message(message.into()),
        }
    }

    /// Parses a type `P` contained within brackets.
    pub fn bracketed<P: Parse>(&mut self) -> Result<P, ParseError> {
        self.parse::<LBracket>()?;
        let p = self.parse::<P>()?;
        self.parse::<RBracket>()?;
        Ok(p)
    }

    /// Parses a type `P` contained within parentheses.
    pub fn parenthesized<P: Parse>(&mut self) -> Result<P, ParseError> {
        self.parse::<LParen>()?;
        let p = self.parse::<P>()?;
        self.parse::<RParen>()?;
        Ok(p)
    }

    /// Parses a type `P` contained within braces.
    pub fn braced<P: Parse>(&mut self) -> Result<P, ParseError> {
        self.parse::<LBrace>()?;
        let p = self.parse::<P>()?;
        self.parse::<RBrace>()?;
        Ok(p)
    }
}

/// A parse-able type.
pub trait Parse: Sized {
    /// Parses the given type using the next tokens in the `input` [`ParseStream`].
    fn parse(input: &mut ParseStream<'_>) -> Result<Self, ParseError>;
}

/// A peek-able type.
pub trait Peek {
    /// Peeks the given type using the next tokens in the `input` [`ParseStream`].
    fn peek(input: &mut ParseStream<'_>) -> bool;
}

#[cfg(test)]
mod tests {
    use crate::syntax::{
        parse_stream::{ParseError, ParseStream},
        span::Span,
        types::At,
    };

    #[test]
    fn parse_basics() {
        let mut input = ParseStream::from(Span::from("@"));
        assert!(input.peek::<At>());
        assert!(!input.is_empty());
        assert!(input.peek::<At>());
        let _ = input.parse::<At>().unwrap();
        assert!(matches!(input.parse::<At>(), Err(ParseError::EndOfInput)));
        assert!(input.is_empty());
    }

    #[test]
    fn a_lex_error_is_not_the_end_of_input() {
        let mut input = ParseStream::from(Span::from("$"));
        assert!(!input.is_empty());
        assert!(!input.is_empty_or_closing_delimiter());
        assert!(matches!(input.parse::<At>(), Err(ParseError::LexError(_))));
    }
}
