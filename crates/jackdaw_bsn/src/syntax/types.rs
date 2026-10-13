use crate::syntax::{
    lex::Token,
    parse_stream::{Parse, ParseError, ParseStream, Peek},
};

/// The root of a BSN document.
#[derive(Debug, Clone, PartialEq)]
pub struct BsnRoot(pub Bsn);

/// The root of a BSN document holding several entities, separated by `--`
/// like a scene list. Jackdaw: named after the `bsn_list!` root.
#[derive(Debug, Clone, PartialEq)]
pub struct BsnListRoot(pub BsnSceneListItems);

/// A BSN definition for a single entity.
#[derive(Debug, Clone, PartialEq)]
pub struct Bsn {
    /// The entries in this BSN definition.
    pub entries: Vec<BsnEntry>,
}

/// An entry within a [`Bsn`] that defines something inside of it.
#[derive(Debug, Clone, PartialEq)]
pub enum BsnEntry {
    /// A named entity reference. Jackdaw also reads `#"a name"`, for names
    /// that are not identifiers.
    Name(Ident),
    /// An included cached scene: `:"path.bsn"`.
    CachedScene(BsnScene),
    /// An included uncached scene: `@"path.bsn"` or `@Type { ... }`.
    /// Jackdaw: named after the `bsn!` entry.
    UncachedScene(BsnScene),
    /// A patch of a `FromTemplate` type
    FromTemplatePatch(BsnType),
    /// Related scene definitions.
    RelatedSceneList {
        /// The type path of the relationship target.
        relationship_target_path: Path,
        /// The list of related scene definitions.
        scene_list: BsnSceneList,
    },
}

/// Another BSN definition to include in the current scene.
#[derive(Debug, Clone, PartialEq)]
pub enum BsnScene {
    /// A "scene asset" path
    Asset(StringLit),
    /// A scene component. Jackdaw: named after the `bsn!` scene.
    SceneComponent(BsnType),
}

/// A list of BSN documents (each item in the list corresponding to a different entity), bounded by
/// brackets.
#[derive(Debug, Clone, PartialEq)]
pub struct BsnSceneList(pub BsnSceneListItems);

/// A flat list of BSN documents (each item in the list corresponding to a different entity).
#[derive(Debug, Clone, PartialEq)]
pub struct BsnSceneListItems(pub Vec<Bsn>);

/// A definition of a type in BSN. This maps to a Rust datatype.
#[derive(Debug, Clone, PartialEq)]
pub struct BsnType {
    /// The type path of the type.
    pub path: Path,
    /// The enum variant, if one exists.
    pub variant: Option<Ident>,
    /// The fields of the type.
    pub fields: BsnFields,
}

/// The fields of a [`BsnType`].
#[derive(Debug, Clone, PartialEq)]
pub enum BsnFields {
    /// Named fields: { field: "value" }
    NamedFields(Vec<BsnNamedField>),
    /// Unnamed fields: ("value1", "value2")
    UnnamedFields(Vec<BsnValue>),
    /// No fields
    Unit,
}

/// A named field. Ex: field: "value"
#[derive(Debug, Clone, PartialEq)]
pub struct BsnNamedField {
    /// The name of the field
    pub name: String,
    /// The value of the field
    pub value: BsnValue,
}

/// The BSN value type.
#[derive(Debug, Clone, PartialEq)]
pub enum BsnValue {
    /// A floating point number
    Float(f64),
    /// An integer number
    Int(i128),
    /// A boolean: true or false
    Bool(bool),
    /// A string literal
    String(String),
    /// A BSN type
    Type(BsnType),
    /// A list: `[value, value]`. Jackdaw: for `Vec`, arrays and tuples.
    List(Vec<BsnValue>),
    /// A map: `map[(key, value), (key, value)]`. Jackdaw: for maps.
    Map(Vec<(BsnValue, BsnValue)>),
}

/// A path to a type. This follows Rust conventions.
#[derive(Debug, Clone, PartialEq)]
pub struct Path {
    /// Whether or not this path has a leading colon.
    pub leading_colon: bool,
    /// The segments of the path.
    pub segments: Vec<PathSegment>,
}

/// A segment of a path, corresponding to a single identifier.
#[derive(Debug, Clone, PartialEq)]
pub struct PathSegment {
    /// The identifier of the segment.
    pub ident: Ident,
    /// The generic arguments, `<A, B>`. Jackdaw: for generic type paths such
    /// as `Handle<Image>`.
    pub arguments: Vec<Path>,
}

impl BsnType {
    /// Creates a new [`BsnType`] from the given `path` and `fields`.
    pub fn new(path: impl Into<Path>, fields: impl Into<BsnFields>) -> Self {
        BsnType {
            path: path.into(),
            variant: None,
            fields: fields.into(),
        }
    }

    /// Creates a new [`BsnType`] from the given `path`
    pub fn path(path: impl Into<Path>) -> Self {
        BsnType {
            path: path.into(),
            variant: None,
            fields: BsnFields::Unit,
        }
    }
}

impl<I: Into<String>> From<I> for PathSegment {
    fn from(value: I) -> Self {
        PathSegment {
            ident: Ident(value.into()),
            arguments: Vec::new(),
        }
    }
}

impl Path {
    /// Converts the `Path` to the equivalent type path (which is a canonical, Rust-like path to the type.)
    /// Generic arguments are written as `TypePath` spells them, `A<B, C>`.
    pub fn to_type_path(&self) -> String {
        let mut value = String::new();
        self.write_type_path(&mut value);
        value
    }

    fn write_type_path(&self, out: &mut String) {
        // NOTE: skipping leading_colon here as type paths are normalized
        for (index, segment) in self.segments.iter().enumerate() {
            if index > 0 {
                out.push_str("::");
            }
            out.push_str(&segment.ident.0);
            if !segment.arguments.is_empty() {
                out.push('<');
                for (i, argument) in segment.arguments.iter().enumerate() {
                    if i > 0 {
                        out.push_str(", ");
                    }
                    argument.write_type_path(out);
                }
                out.push('>');
            }
        }
    }
}

impl From<Vec<PathSegment>> for Path {
    fn from(segments: Vec<PathSegment>) -> Self {
        Self {
            leading_colon: false,
            segments,
        }
    }
}

impl<const LEN: usize, I: Into<String>> From<[I; LEN]> for Path {
    fn from(segments: [I; LEN]) -> Self {
        Self {
            leading_colon: false,
            segments: segments.into_iter().map(PathSegment::from).collect(),
        }
    }
}

impl BsnNamedField {
    /// Creates a new instance with the given `name` and `value`.
    pub fn new(name: impl Into<String>, value: BsnValue) -> Self {
        Self {
            name: name.into(),
            value,
        }
    }
}

macro_rules! impl_parse_token {
    ($token:ident) => {
        impl Parse for $token {
            fn parse(input: &mut ParseStream<'_>) -> Result<Self, ParseError> {
                match input.next_token()? {
                    Some(token) => {
                        if matches!(token.token, Token::$token) {
                            Ok($token)
                        } else {
                            Err(ParseError::UnexpectedToken(token.into()))
                        }
                    }
                    None => Err(ParseError::EndOfInput),
                }
            }
        }

        impl Peek for $token {
            fn peek(input: &mut ParseStream<'_>) -> bool {
                if let Some(token) = input.peek_token() {
                    matches!(token.token, Token::$token)
                } else {
                    false
                }
            }
        }
    };
}

macro_rules! impl_parse_arg_token {
    ($token:ident) => {
        impl Parse for $token {
            fn parse(input: &mut ParseStream<'_>) -> Result<Self, ParseError> {
                match input.next_token()? {
                    Some(token) => {
                        if let Token::$token(value) = token.token {
                            Ok($token(value))
                        } else {
                            Err(ParseError::UnexpectedToken(token.into()))
                        }
                    }
                    None => Err(ParseError::EndOfInput),
                }
            }
        }

        impl Peek for $token {
            fn peek(input: &mut ParseStream<'_>) -> bool {
                if let Some(token) = input.peek_token() {
                    matches!(token.token, Token::$token(_))
                } else {
                    false
                }
            }
        }
    };
}

/// Boolean value: true, false.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bool(pub bool);
impl_parse_arg_token!(Bool);

/// Identifier: `Foo`, `bar`, `Baz12`, `FooBar`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ident(pub String);
impl_parse_arg_token!(Ident);

impl From<&str> for Ident {
    fn from(value: &str) -> Self {
        Ident(value.into())
    }
}

/// String literal: "hello world"
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StringLit(pub String);
impl_parse_arg_token!(StringLit);

/// Integer: 1, 10, -10
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Int(pub i128);
impl_parse_arg_token!(Int);

/// Floating point: 0.1, .1, 10.1
#[derive(Debug, Clone, PartialEq)]
pub struct Float(pub f64);
impl_parse_arg_token!(Float);

/// Left bracket: [
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LBracket;
impl_parse_token!(LBracket);

/// Right bracket: ]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RBracket;
impl_parse_token!(RBracket);

/// Left Paren: (
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LParen;
impl_parse_token!(LParen);

/// Right Paren: )
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RParen;
impl_parse_token!(RParen);

/// Left Brace: {
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LBrace;
impl_parse_token!(LBrace);

/// Right Brace: }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RBrace;
impl_parse_token!(RBrace);

/// Comma: ,
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comma;
impl_parse_token!(Comma);

/// Double Colon: ::
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoubleColon;
impl_parse_token!(DoubleColon);

/// Double Minus: --
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoubleMinus;
impl_parse_token!(DoubleMinus);

/// Colon: :
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Colon;
impl_parse_token!(Colon);

/// Hash: #
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hash;
impl_parse_token!(Hash);

/// At: @
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct At;
impl_parse_token!(At);

/// Less: <
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Less;
impl_parse_token!(Less);

/// Greater: >
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Greater;
impl_parse_token!(Greater);
