//! Every parameter an operator reads is a parameter it declares.
//!
//! `OperatorEntity::parameters` is the whole schema an outside caller
//! has: `jackdaw/operators` publishes it, `jackdaw/call_operator` types
//! JSON values against it, and `boot_ops` resolves `Entity` parameters
//! from it. A parameter read but not declared is therefore invisible --
//! a caller cannot discover it, and a value passed for it arrives typed
//! by its spelling rather than by what the operator declared.
//!
//! There is no runtime hook to catch that: the accessors take a `&str`
//! and a miss is indistinguishable from an absent optional. So this
//! reads the source, which is where the declaration and the read sit a
//! few lines apart.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Accessor calls that name a parameter: `params.as_str("path")` and the
/// `read_*_param(&params, "axis")` helpers a few modules keep.
fn parameters_read(body: &str) -> BTreeSet<String> {
    const ACCESSORS: &[&str] = &[
        "as_str",
        "as_int",
        "as_float",
        "as_bool",
        "as_entity",
        "as_vec2",
        "as_vec3",
        "as_color",
        "get",
    ];
    let mut found = BTreeSet::new();
    for (at, _) in body.match_indices('"') {
        let Some(rest) = body.get(at + 1..) else {
            continue;
        };
        let Some(end) = rest.find('"') else { continue };
        let name = &rest[..end];
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        // What sits in front of the opening quote decides whether this
        // literal is a parameter name or any other string in the body.
        let before = body[..at].trim_end();
        // `params.as_str("x")`: the accessor sits right before the paren.
        let call = before
            .strip_suffix('(')
            .map(str::trim_end)
            .unwrap_or_default();
        let is_accessor = ACCESSORS
            .iter()
            .any(|accessor| call.ends_with(&format!(".{accessor}")));
        // `read_int_param(&params, "x")`: the helpers a few modules keep
        // take the name as a second argument. Only the arguments of the
        // call this literal actually sits in are read: matching anywhere in
        // the prefix would count every later string in a body that called
        // such a helper once, and fail an operator for a parameter it does
        // not have.
        let is_helper = before.ends_with(',')
            && before
                .rfind("_param(")
                .map(|at| &before[at + "_param(".len()..])
                .is_some_and(|args| !args.contains(')') && args.contains("params"));
        if is_accessor || is_helper {
            found.insert(name.to_string());
        }
    }
    found
}

/// The `(` ... `)` group starting at `open`, which has to be the index
/// of the opening paren.
fn group_end(src: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (offset, byte) in src.as_bytes()[open..].iter().enumerate() {
        match byte {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(open + offset);
                }
            }
            _ => {}
        }
    }
    None
}

/// The value of a `key = "value"` entry in an attribute.
fn attribute_string(attr: &str, key: &str) -> Option<String> {
    let at = attr.find(&format!("{key} ="))?;
    let rest = &attr[at..];
    let open = rest.find('"')? + 1;
    let end = rest[open..].find('"')? + open;
    Some(rest[open..end].to_string())
}

/// The parameter names a `params(..)` block declares.
fn declared_parameters(attr: &str) -> BTreeSet<String> {
    let Some(at) = attr.find("params(") else {
        return BTreeSet::new();
    };
    let open = at + "params".len();
    let Some(end) = group_end(attr, open) else {
        return BTreeSet::new();
    };
    let block = &attr[open + 1..end];

    let mut names = BTreeSet::new();
    let mut depth = 0usize;
    let mut word = String::new();
    for ch in block.chars() {
        match ch {
            '(' => {
                // A parameter whose name is a Rust keyword is declared raw
                // (`r#match`); the macro publishes it without the escape.
                let name = word.trim();
                if depth == 0 && !name.is_empty() {
                    names.insert(name.strip_prefix("r#").unwrap_or(name).to_string());
                }
                depth += 1;
                word.clear();
            }
            ')' => {
                depth = depth.saturating_sub(1);
                word.clear();
            }
            ',' if depth == 0 => word.clear(),
            _ if depth == 0 => word.push(ch),
            _ => {}
        }
    }
    names
}

fn rust_sources(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "target") {
                continue;
            }
            rust_sources(&path, found);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

/// Every operator in the tree, as `(id, file, declared, read)`.
fn operators() -> Vec<(String, PathBuf, BTreeSet<String>, BTreeSet<String>)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_sources(&root.join("src"), &mut files);
    rust_sources(&root.join("crates"), &mut files);

    let mut found = Vec::new();
    for path in files {
        let Ok(src) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (at, _) in src.match_indices("#[operator(") {
            let open = at + "#[operator".len();
            let Some(end) = group_end(&src, open) else {
                continue;
            };
            let attr = &src[at..=end];
            let Some(id) = attribute_string(attr, "id") else {
                continue;
            };
            // The system body: everything up to the next item, which is
            // the first `}` in the first column.
            let rest = &src[end + 1..];
            let body = match rest.find("\n}\n") {
                Some(stop) => &rest[..stop],
                None => rest,
            };
            found.push((
                id,
                path.clone(),
                declared_parameters(attr),
                parameters_read(body),
            ));
        }
    }
    found
}

/// The reader finds what it is looking for. Without this the audit below
/// would pass by finding nothing at all.
#[test]
fn the_source_reader_finds_operators_and_their_parameters() {
    let all = operators();
    assert!(
        all.len() > 100,
        "only {} operators found; the reader is broken",
        all.len()
    );

    let (_, _, declared, read) = all
        .iter()
        .find(|(id, ..)| id == "view.set_axis")
        .expect("view.set_axis is declared in src/view_ops.rs");
    assert!(
        declared.contains("axis") && declared.contains("sign"),
        "declared: {declared:?}"
    );
    assert!(
        read.contains("axis") && read.contains("sign"),
        "read: {read:?}"
    );
}

/// No operator reads a parameter it does not declare.
#[test]
fn every_parameter_an_operator_reads_is_declared() {
    let undeclared: Vec<String> = operators()
        .into_iter()
        .filter_map(|(id, path, declared, read)| {
            let missing: Vec<String> = read.difference(&declared).cloned().collect();
            (!missing.is_empty()).then(|| {
                format!(
                    "{id} ({}) reads {missing:?}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                )
            })
        })
        .collect();

    assert!(
        undeclared.is_empty(),
        "these operators read parameters they do not declare, so no caller can discover them \
         and a value passed for one is typed by its spelling rather than by the operator's \
         schema. Add a `params(name(Type, doc = \"..\"))` entry for each: {undeclared:#?}"
    );
}
