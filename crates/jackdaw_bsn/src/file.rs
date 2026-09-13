//! Reading and writing a BSN document in either of its two forms.
//!
//! A document on disk is either `.bsn` text or its `.bsb` binary twin. Which
//! one a file holds is decided by its first bytes, never by its extension, so
//! a reader handles both without knowing which it was handed. Writing goes the
//! other way: the path's extension picks the form, which is how a save keeps a
//! file in the form it was opened in.

use std::path::{Path, PathBuf};

use crate::binary::{self, BinaryError};
use crate::header::read_asset_header;
use crate::{BsnLoadError, SceneBsnAst, emit_scene, parse_bsn_text};

/// The extension a text document carries.
pub const TEXT_EXTENSION: &str = "bsn";

/// The extension a binary document carries.
pub const BINARY_EXTENSION: &str = "bsb";

/// Which of the two forms a document is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DocumentForm {
    /// `.bsn` source text.
    Text,
    /// The `.bsb` binary twin.
    Binary,
}

/// A document read off disk, with the form it was in.
pub struct Document {
    /// The parsed document.
    pub ast: SceneBsnAst,
    /// The comment lines the document opens with, verbatim.
    pub preamble: Option<String>,
    /// The form the file on disk was in.
    pub form: DocumentForm,
}

impl Document {
    /// The type path the asset header names, for a document that carries one.
    pub fn header(&self) -> Option<String> {
        self.preamble.as_deref().and_then(read_asset_header)
    }
}

/// Why a document did not read or write.
#[derive(Debug, thiserror::Error)]
pub enum DocumentError {
    #[error("failed to read {}: {}", .0.display(), .1)]
    Read(PathBuf, std::io::Error),
    #[error("failed to write {}: {}", .0.display(), .1)]
    Write(PathBuf, std::io::Error),
    #[error("failed to parse {}: {}", .0.display(), .1)]
    Parse(PathBuf, BsnLoadError),
    #[error("failed to read {}: {}", .0.display(), .1)]
    Binary(PathBuf, BinaryError),
    #[error("{} holds text that is not UTF-8", .0.display())]
    NotUtf8(PathBuf),
    #[error("refusing to convert {}: {} is already on disk", .1.display(), .0.display())]
    TwinExists(PathBuf, PathBuf),
    #[error("refusing to export {}: {} sits inside it", .1.display(), .0.display())]
    DestinationInsideSource(PathBuf, PathBuf),
}

/// Whether an extension names a BSN document in either form.
pub fn is_document_extension(extension: &str) -> bool {
    extension.eq_ignore_ascii_case(TEXT_EXTENSION)
        || extension.eq_ignore_ascii_case(BINARY_EXTENSION)
}

/// Whether a path names a BSN document in either form.
pub fn is_document_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(is_document_extension)
}

/// Whether a path names a document in the binary form.
pub fn is_binary_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(BINARY_EXTENSION))
}

/// The binary twin of a document path.
pub fn binary_twin(path: &Path) -> PathBuf {
    path.with_extension(BINARY_EXTENSION)
}

/// The text twin of a document path.
pub fn text_twin(path: &Path) -> PathBuf {
    path.with_extension(TEXT_EXTENSION)
}

/// The form of a document that exists at this path or at its twin, preferring
/// the path as written and then the text form.
pub fn existing_form(path: &Path) -> Option<PathBuf> {
    if path.is_file() {
        return Some(path.to_path_buf());
    }
    if !is_document_path(path) {
        return None;
    }
    let text = text_twin(path);
    if text.is_file() {
        return Some(text);
    }
    let binary = binary_twin(path);
    binary.is_file().then_some(binary)
}

/// Read a document, choosing its form by what the first bytes say.
pub fn read_document(path: &Path) -> Result<Document, DocumentError> {
    let bytes = std::fs::read(path).map_err(|err| DocumentError::Read(path.to_path_buf(), err))?;
    document_from_bytes(&bytes, path)
}

/// [`read_document`] over bytes already in hand, `path` naming them in errors.
pub fn document_from_bytes(bytes: &[u8], path: &Path) -> Result<Document, DocumentError> {
    if binary::is_binary(bytes) {
        let decoded =
            binary::decode(bytes).map_err(|err| DocumentError::Binary(path.to_path_buf(), err))?;
        return Ok(Document {
            ast: decoded.ast,
            preamble: decoded.preamble,
            form: DocumentForm::Binary,
        });
    }
    let text =
        std::str::from_utf8(bytes).map_err(|_| DocumentError::NotUtf8(path.to_path_buf()))?;
    let ast = parse_bsn_text(text).map_err(|err| DocumentError::Parse(path.to_path_buf(), err))?;
    Ok(Document {
        ast,
        preamble: leading_comments(text),
        form: DocumentForm::Text,
    })
}

/// Read a document as `.bsn` text, whichever form it is stored in.
///
/// A text file reads back verbatim; a binary one is emitted under the comment
/// lines it was written with.
pub fn read_document_text(path: &Path) -> Result<String, DocumentError> {
    let bytes = std::fs::read(path).map_err(|err| DocumentError::Read(path.to_path_buf(), err))?;
    document_text_from_bytes(&bytes, path)
}

/// [`read_document_text`] over bytes already in hand.
pub fn document_text_from_bytes(bytes: &[u8], path: &Path) -> Result<String, DocumentError> {
    if !binary::is_binary(bytes) {
        return std::str::from_utf8(bytes)
            .map(str::to_string)
            .map_err(|_| DocumentError::NotUtf8(path.to_path_buf()));
    }
    let decoded =
        binary::decode(bytes).map_err(|err| DocumentError::Binary(path.to_path_buf(), err))?;
    Ok(document_as_text(&decoded.ast, decoded.preamble.as_deref()))
}

/// A document as the `.bsn` text it emits, under the comment lines it opens
/// with.
pub fn document_as_text(ast: &SceneBsnAst, preamble: Option<&str>) -> String {
    let body = emit_scene(ast);
    match preamble {
        Some(preamble) => format!("{preamble}{body}"),
        None => body,
    }
}

/// The comment lines a document opens with, which carry its asset header and
/// its version stamp.
pub fn leading_comments(text: &str) -> Option<String> {
    let mut end = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.starts_with("//") {
            break;
        }
        end += line.len();
    }
    (end > 0).then(|| text[..end].to_string())
}

/// Write `text` to `path` in the form the path's extension names.
pub fn write_document_text(path: &Path, text: &str) -> Result<(), DocumentError> {
    let bytes = document_bytes(path, text)?;
    std::fs::write(path, bytes).map_err(|err| DocumentError::Write(path.to_path_buf(), err))
}

/// The bytes `text` is stored as at `path`, in the form its extension names.
pub fn document_bytes(path: &Path, text: &str) -> Result<Vec<u8>, DocumentError> {
    if !is_binary_path(path) {
        return Ok(text.as_bytes().to_vec());
    }
    text_as_binary(text, path)
}

/// `.bsn` text encoded as its binary twin, leading comments and all.
pub fn text_as_binary(text: &str, path: &Path) -> Result<Vec<u8>, DocumentError> {
    let ast = parse_bsn_text(text).map_err(|err| DocumentError::Parse(path.to_path_buf(), err))?;
    Ok(binary::encode(&ast, leading_comments(text).as_deref()))
}

/// Rewrite one document as its binary twin, removing the text file once the
/// binary one is on disk. Returns the path the document now sits at.
pub fn convert_to_binary(path: &Path) -> Result<PathBuf, DocumentError> {
    convert(path, DocumentForm::Binary)
}

/// Rewrite one document as `.bsn` text, removing the binary file once the text
/// one is on disk. Returns the path the document now sits at.
pub fn convert_to_text(path: &Path) -> Result<PathBuf, DocumentError> {
    convert(path, DocumentForm::Text)
}

fn convert(path: &Path, to: DocumentForm) -> Result<PathBuf, DocumentError> {
    let document = read_document(path)?;
    let (target, bytes) = match to {
        DocumentForm::Binary => (
            binary_twin(path),
            binary::encode(&document.ast, document.preamble.as_deref()),
        ),
        DocumentForm::Text => (
            text_twin(path),
            document_as_text(&document.ast, document.preamble.as_deref()).into_bytes(),
        ),
    };
    if document.form == to && target == path {
        return Ok(target);
    }
    if target != path && target.exists() {
        return Err(DocumentError::TwinExists(target, path.to_path_buf()));
    }
    std::fs::write(&target, bytes).map_err(|err| DocumentError::Write(target.clone(), err))?;
    if target != path {
        std::fs::remove_file(path).map_err(|err| DocumentError::Write(path.to_path_buf(), err))?;
    }
    Ok(target)
}

/// Write every `.bsn` document under `source` into `destination` as its binary
/// twin, copying every other file through unchanged. Returns how many
/// documents were converted.
pub fn export_binary(source: &Path, destination: &Path) -> Result<usize, DocumentError> {
    let held = resolved(source);
    if resolved(destination).starts_with(&held) {
        return Err(DocumentError::DestinationInsideSource(
            destination.to_path_buf(),
            source.to_path_buf(),
        ));
    }
    let mut converted = 0;
    export_tree(source, source, destination, 0, &mut converted)?;
    Ok(converted)
}

/// A path with every part of it that exists on disk resolved, so two spellings
/// of one folder compare equal.
fn resolved(path: &Path) -> PathBuf {
    if let Ok(held) = path.canonicalize() {
        return held;
    }
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => resolved(parent).join(name),
        _ => path.to_path_buf(),
    }
}

fn export_tree(
    root: &Path,
    dir: &Path,
    destination: &Path,
    depth: usize,
    converted: &mut usize,
) -> Result<(), DocumentError> {
    if depth > crate::header::MAX_ASSET_DEPTH {
        return Ok(());
    }
    let entries =
        std::fs::read_dir(dir).map_err(|err| DocumentError::Read(dir.to_path_buf(), err))?;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let target = destination.join(relative);
        if file_type.is_dir() {
            export_tree(root, &path, destination, depth + 1, converted)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|err| DocumentError::Write(parent.to_path_buf(), err))?;
        }
        let is_text_document = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case(TEXT_EXTENSION));
        if !is_text_document {
            std::fs::copy(&path, &target)
                .map_err(|err| DocumentError::Write(target.clone(), err))?;
            continue;
        }
        let document = read_document(&path)?;
        let target = binary_twin(&target);
        std::fs::write(
            &target,
            binary::encode(&document.ast, document.preamble.as_deref()),
        )
        .map_err(|err| DocumentError::Write(target.clone(), err))?;
        *converted += 1;
    }
    Ok(())
}
