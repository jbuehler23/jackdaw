//! Binary form of a BSN document: the same document the text parser builds,
//! written as a compact self-describing byte stream.
//!
//! Text stays the form a repository keeps. The binary form is for a shipped
//! game's export and for large files nobody diffs, so it holds exactly what
//! the text holds: the leading comment lines that carry the asset header and
//! the version stamp, the document roots, their patches and their values.
//! [`is_binary`] tells the two apart from the first four bytes, which lets
//! every reader sniff rather than trust an extension.

use bevy::ecs::entity::Entity;

use crate::document::{
    BsnField, BsnPatch, BsnStructData, BsnStructFields, BsnTupleStructData, BsnValue,
    MAX_AST_DEPTH, SceneBsnAst,
};

/// The four bytes every binary BSN document starts with.
///
/// The first byte is a UTF-8 continuation byte, which no text document can
/// begin with, so sniffing never mistakes one form for the other.
pub const MAGIC: [u8; 4] = [0x89, b'B', b'S', b'B'];

/// The format version this build writes.
pub const VERSION: u16 = 1;

const PATCH_NAME: u8 = 0;
const PATCH_BASE: u8 = 1;
const PATCH_TYPE: u8 = 2;
const PATCH_STRUCT: u8 = 3;
const PATCH_TUPLE_STRUCT: u8 = 4;
const PATCH_TEMPLATE: u8 = 5;
const PATCH_CHILDREN: u8 = 6;

const VALUE_FLOAT: u8 = 0;
const VALUE_INT: u8 = 1;
const VALUE_BOOL: u8 = 2;
const VALUE_STRING: u8 = 3;
const VALUE_TYPE: u8 = 4;
const VALUE_STRUCT: u8 = 5;
const VALUE_TUPLE_STRUCT: u8 = 6;
const VALUE_LIST: u8 = 7;
const VALUE_MAP: u8 = 8;

/// A document read back out of its binary form.
#[derive(Default)]
pub struct DecodedDocument {
    /// The document itself.
    pub ast: SceneBsnAst,
    /// The comment lines the text form opens with, verbatim.
    pub preamble: Option<String>,
}

/// Why a run of bytes did not read as a binary BSN document.
#[derive(Debug, thiserror::Error)]
pub enum BinaryError {
    /// The first bytes are not [`MAGIC`].
    #[error("not a binary BSN document: the first bytes are not {:?}", MAGIC)]
    BadMagic,
    /// The document was written by a later format than this build reads.
    #[error("binary BSN version {0} is newer than the supported {VERSION}")]
    UnsupportedVersion(u16),
    /// The bytes ran out mid-value.
    #[error("binary BSN document ends early")]
    Truncated,
    /// A tag byte named no patch or value this format has.
    #[error("binary BSN document holds an unknown {kind} tag {tag}")]
    UnknownTag {
        /// Whether the tag was read as a patch or as a value.
        kind: &'static str,
        /// The byte that named nothing.
        tag: u8,
    },
    /// A string field held bytes that are not UTF-8.
    #[error("binary BSN document holds text that is not UTF-8")]
    NotUtf8,
    /// The document's `Children` nesting ran past the walk's cap.
    #[error("binary BSN document nests deeper than {MAX_AST_DEPTH}")]
    TooDeep,
}

/// Whether these bytes open with [`MAGIC`].
pub fn is_binary(bytes: &[u8]) -> bool {
    bytes.starts_with(&MAGIC)
}

/// Write a document and the comment lines it opens with as binary.
pub fn encode(ast: &SceneBsnAst, preamble: Option<&str>) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC);
    out.extend_from_slice(&VERSION.to_le_bytes());
    write_option_str(preamble, &mut out);
    write_len(ast.roots.len(), &mut out);
    for &root in &ast.roots {
        write_node(ast, root, 0, &mut out);
    }
    out
}

/// Read a document and the comment lines it opens with back from binary.
pub fn decode(bytes: &[u8]) -> Result<DecodedDocument, BinaryError> {
    let mut reader = Reader::new(bytes);
    if reader.take(MAGIC.len())? != MAGIC {
        return Err(BinaryError::BadMagic);
    }
    let version = reader.u16()?;
    if version > VERSION {
        return Err(BinaryError::UnsupportedVersion(version));
    }
    let preamble = reader.option_str()?;

    let mut ast = SceneBsnAst::default();
    let roots = reader.len()?;
    let mut root_nodes = Vec::new();
    for _ in 0..roots {
        root_nodes.push(read_node(&mut reader, &mut ast, 0)?);
    }
    for root in root_nodes {
        ast.add_to_roots(root);
    }
    Ok(DecodedDocument { ast, preamble })
}

fn write_node(ast: &SceneBsnAst, node: Entity, depth: usize, out: &mut Vec<u8>) {
    if depth >= MAX_AST_DEPTH {
        log::warn!("document node {node} is deeper than {MAX_AST_DEPTH}; it was not written");
        write_len(0, out);
        return;
    }
    let patches = ast
        .get_patches(node)
        .map(|p| p.0.clone())
        .unwrap_or_default();
    let written: Vec<&BsnPatch> = patches
        .iter()
        .filter_map(|&patch| ast.get_patch(patch))
        .collect();
    write_len(written.len(), out);
    for patch in written {
        write_patch(ast, patch, depth, out);
    }
}

fn write_patch(ast: &SceneBsnAst, patch: &BsnPatch, depth: usize, out: &mut Vec<u8>) {
    match patch {
        BsnPatch::Name(name) => {
            out.push(PATCH_NAME);
            write_str(name, out);
        }
        BsnPatch::Base(path) => {
            out.push(PATCH_BASE);
            write_str(path, out);
        }
        BsnPatch::Type(type_path) => {
            out.push(PATCH_TYPE);
            write_str(type_path, out);
        }
        BsnPatch::Struct(data) => {
            out.push(PATCH_STRUCT);
            write_struct(data, out);
        }
        BsnPatch::TupleStruct(data) => {
            out.push(PATCH_TUPLE_STRUCT);
            write_tuple_struct(data, out);
        }
        BsnPatch::Template(type_path, fields) => {
            out.push(PATCH_TEMPLATE);
            write_str(type_path, out);
            match fields {
                Some(fields) => {
                    out.push(1);
                    write_fields(fields, out);
                }
                None => out.push(0),
            }
        }
        BsnPatch::Children(children) => {
            out.push(PATCH_CHILDREN);
            write_len(children.len(), out);
            for &child in children {
                write_node(ast, child, depth + 1, out);
            }
        }
    }
}

fn write_struct(data: &BsnStructData, out: &mut Vec<u8>) {
    write_str(&data.type_path, out);
    write_fields(&data.fields, out);
}

fn write_tuple_struct(data: &BsnTupleStructData, out: &mut Vec<u8>) {
    write_str(&data.type_path, out);
    write_len(data.values.len(), out);
    for value in &data.values {
        write_value(value, out);
    }
}

fn write_fields(fields: &BsnStructFields, out: &mut Vec<u8>) {
    write_len(fields.0.len(), out);
    for field in &fields.0 {
        write_str(&field.name, out);
        write_value(&field.value, out);
    }
}

fn write_value(value: &BsnValue, out: &mut Vec<u8>) {
    match value {
        BsnValue::Float(number) => {
            out.push(VALUE_FLOAT);
            out.extend_from_slice(&number.to_le_bytes());
        }
        BsnValue::Int(number) => {
            out.push(VALUE_INT);
            out.extend_from_slice(&number.to_le_bytes());
        }
        BsnValue::Bool(flag) => {
            out.push(VALUE_BOOL);
            out.push(u8::from(*flag));
        }
        BsnValue::String(text) => {
            out.push(VALUE_STRING);
            write_str(text, out);
        }
        BsnValue::Type(type_path) => {
            out.push(VALUE_TYPE);
            write_str(type_path, out);
        }
        BsnValue::Struct(data) => {
            out.push(VALUE_STRUCT);
            write_struct(data, out);
        }
        BsnValue::TupleStruct(data) => {
            out.push(VALUE_TUPLE_STRUCT);
            write_tuple_struct(data, out);
        }
        BsnValue::List(items) => {
            out.push(VALUE_LIST);
            write_len(items.len(), out);
            for item in items {
                write_value(item, out);
            }
        }
        BsnValue::Map(entries) => {
            out.push(VALUE_MAP);
            write_len(entries.len(), out);
            for (key, item) in entries {
                write_value(key, out);
                write_value(item, out);
            }
        }
    }
}

fn write_len(len: usize, out: &mut Vec<u8>) {
    out.extend_from_slice(&u64::try_from(len).unwrap_or(u64::MAX).to_le_bytes());
}

fn write_str(text: &str, out: &mut Vec<u8>) {
    write_len(text.len(), out);
    out.extend_from_slice(text.as_bytes());
}

fn write_option_str(text: Option<&str>, out: &mut Vec<u8>) {
    match text {
        Some(text) => {
            out.push(1);
            write_str(text, out);
        }
        None => out.push(0),
    }
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }

    fn take(&mut self, count: usize) -> Result<&'a [u8], BinaryError> {
        let end = self.at.checked_add(count).ok_or(BinaryError::Truncated)?;
        let slice = self.bytes.get(self.at..end).ok_or(BinaryError::Truncated)?;
        self.at = end;
        Ok(slice)
    }

    fn byte(&mut self) -> Result<u8, BinaryError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, BinaryError> {
        let bytes: [u8; 2] = self
            .take(2)?
            .try_into()
            .map_err(|_| BinaryError::Truncated)?;
        Ok(u16::from_le_bytes(bytes))
    }

    fn u64(&mut self) -> Result<u64, BinaryError> {
        let bytes: [u8; 8] = self
            .take(8)?
            .try_into()
            .map_err(|_| BinaryError::Truncated)?;
        Ok(u64::from_le_bytes(bytes))
    }

    fn f64(&mut self) -> Result<f64, BinaryError> {
        let bytes: [u8; 8] = self
            .take(8)?
            .try_into()
            .map_err(|_| BinaryError::Truncated)?;
        Ok(f64::from_le_bytes(bytes))
    }

    fn i128(&mut self) -> Result<i128, BinaryError> {
        let bytes: [u8; 16] = self
            .take(16)?
            .try_into()
            .map_err(|_| BinaryError::Truncated)?;
        Ok(i128::from_le_bytes(bytes))
    }

    fn bool(&mut self) -> Result<bool, BinaryError> {
        Ok(self.byte()? != 0)
    }

    /// A count, refused up front when it is longer than the bytes that remain.
    fn len(&mut self) -> Result<usize, BinaryError> {
        let len = usize::try_from(self.u64()?).map_err(|_| BinaryError::Truncated)?;
        if len > self.bytes.len() - self.at {
            return Err(BinaryError::Truncated);
        }
        Ok(len)
    }

    fn string(&mut self) -> Result<String, BinaryError> {
        let len = self.len()?;
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| BinaryError::NotUtf8)
    }

    fn option_str(&mut self) -> Result<Option<String>, BinaryError> {
        match self.byte()? {
            0 => Ok(None),
            _ => Ok(Some(self.string()?)),
        }
    }
}

fn read_node(
    reader: &mut Reader<'_>,
    ast: &mut SceneBsnAst,
    depth: usize,
) -> Result<Entity, BinaryError> {
    if depth >= MAX_AST_DEPTH {
        return Err(BinaryError::TooDeep);
    }
    let count = reader.len()?;
    let mut patches = Vec::new();
    for _ in 0..count {
        patches.push(read_patch(reader, ast, depth)?);
    }
    Ok(ast.create_entity_node(patches))
}

fn read_patch(
    reader: &mut Reader<'_>,
    ast: &mut SceneBsnAst,
    depth: usize,
) -> Result<BsnPatch, BinaryError> {
    let tag = reader.byte()?;
    match tag {
        PATCH_NAME => Ok(BsnPatch::Name(reader.string()?)),
        PATCH_BASE => Ok(BsnPatch::Base(reader.string()?)),
        PATCH_TYPE => Ok(BsnPatch::Type(reader.string()?)),
        PATCH_STRUCT => Ok(BsnPatch::Struct(read_struct(reader, 0)?)),
        PATCH_TUPLE_STRUCT => Ok(BsnPatch::TupleStruct(read_tuple_struct(reader, 0)?)),
        PATCH_TEMPLATE => {
            let type_path = reader.string()?;
            let fields = match reader.byte()? {
                0 => None,
                _ => Some(read_fields(reader, 0)?),
            };
            Ok(BsnPatch::Template(type_path, fields))
        }
        PATCH_CHILDREN => {
            let count = reader.len()?;
            let mut children = Vec::new();
            for _ in 0..count {
                children.push(read_node(reader, ast, depth + 1)?);
            }
            Ok(BsnPatch::Children(children))
        }
        tag => Err(BinaryError::UnknownTag { kind: "patch", tag }),
    }
}

fn read_struct(reader: &mut Reader<'_>, depth: usize) -> Result<BsnStructData, BinaryError> {
    let type_path = reader.string()?;
    let fields = read_fields(reader, depth)?;
    Ok(BsnStructData { type_path, fields })
}

fn read_tuple_struct(
    reader: &mut Reader<'_>,
    depth: usize,
) -> Result<BsnTupleStructData, BinaryError> {
    let type_path = reader.string()?;
    let count = reader.len()?;
    let mut values = Vec::new();
    for _ in 0..count {
        values.push(read_value(reader, depth + 1)?);
    }
    Ok(BsnTupleStructData { type_path, values })
}

fn read_fields(reader: &mut Reader<'_>, depth: usize) -> Result<BsnStructFields, BinaryError> {
    let count = reader.len()?;
    let mut fields = Vec::new();
    for _ in 0..count {
        let name = reader.string()?;
        let value = read_value(reader, depth + 1)?;
        fields.push(BsnField { name, value });
    }
    Ok(BsnStructFields(fields))
}

fn read_value(reader: &mut Reader<'_>, depth: usize) -> Result<BsnValue, BinaryError> {
    if depth >= MAX_AST_DEPTH {
        return Err(BinaryError::TooDeep);
    }
    let tag = reader.byte()?;
    match tag {
        VALUE_FLOAT => Ok(BsnValue::Float(reader.f64()?)),
        VALUE_INT => Ok(BsnValue::Int(reader.i128()?)),
        VALUE_BOOL => Ok(BsnValue::Bool(reader.bool()?)),
        VALUE_STRING => Ok(BsnValue::String(reader.string()?)),
        VALUE_TYPE => Ok(BsnValue::Type(reader.string()?)),
        VALUE_STRUCT => Ok(BsnValue::Struct(read_struct(reader, depth)?)),
        VALUE_TUPLE_STRUCT => Ok(BsnValue::TupleStruct(read_tuple_struct(reader, depth)?)),
        VALUE_LIST => {
            let count = reader.len()?;
            let mut items = Vec::new();
            for _ in 0..count {
                items.push(read_value(reader, depth + 1)?);
            }
            Ok(BsnValue::List(items))
        }
        VALUE_MAP => {
            let count = reader.len()?;
            let mut entries = Vec::new();
            for _ in 0..count {
                let key = read_value(reader, depth + 1)?;
                let value = read_value(reader, depth + 1)?;
                entries.push((key, value));
            }
            Ok(BsnValue::Map(entries))
        }
        tag => Err(BinaryError::UnknownTag { kind: "value", tag }),
    }
}
