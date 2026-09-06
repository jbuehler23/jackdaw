//! Editor preferences that belong to a project, kept in
//! `<root>/.jackdaw/settings.json`.
//!
//! More than one subsystem keeps preferences there, so a write is a
//! read-modify-write of the one file: serialising a whole struct over it
//! would drop whatever another subsystem had written beside it.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};

/// The settings file of the project rooted at `root`.
pub fn settings_path(root: &Path) -> PathBuf {
    root.join(".jackdaw/settings.json")
}

/// Where a settings struct sits in the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Section<'a> {
    /// The struct's fields are the file's own top-level fields.
    TopLevel,
    /// The struct is the object under this key.
    Key(&'a str),
}

/// Read one section of the project's settings. A missing file, a missing
/// section and an unreadable one all give `T::default()`, so a project
/// that has never had these settings written opens on the defaults.
pub fn load_section<T: DeserializeOwned + Default>(root: &Path, section: Section<'_>) -> T {
    let Some(document) = read_document(root) else {
        return T::default();
    };
    let value = match section {
        Section::TopLevel => Value::Object(document),
        Section::Key(key) => document.get(key).cloned().unwrap_or(Value::Null),
    };
    serde_json::from_value(value).unwrap_or_default()
}

/// Write one section of the project's settings, leaving every other
/// section as it was on disk.
pub fn store_section<T: Serialize>(root: &Path, section: Section<'_>, settings: &T) {
    let value = match serde_json::to_value(settings) {
        Ok(value) => value,
        Err(error) => {
            warn!("could not encode the project settings: {error}");
            return;
        }
    };
    let mut document = read_document(root).unwrap_or_default();
    match section {
        Section::TopLevel => {
            let Some(fields) = value.as_object() else {
                warn!("top-level project settings must encode as an object");
                return;
            };
            for (key, value) in fields {
                document.insert(key.clone(), value.clone());
            }
        }
        Section::Key(key) => {
            document.insert(key.to_string(), value);
        }
    }
    write_document(root, &document);
}

fn read_document(root: &Path) -> Option<Map<String, Value>> {
    let bytes = std::fs::read(settings_path(root)).ok()?;
    match serde_json::from_slice::<Value>(&bytes) {
        Ok(Value::Object(document)) => Some(document),
        Ok(_) => {
            warn!("the project settings are not a JSON object; reading them as empty");
            None
        }
        Err(error) => {
            warn!("could not read the project settings: {error}");
            None
        }
    }
}

fn write_document(root: &Path, document: &Map<String, Value>) {
    let path = settings_path(root);
    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        warn!("could not persist the project settings: {error}");
        return;
    }
    match serde_json::to_vec_pretty(document) {
        Ok(bytes) => {
            if let Err(error) = std::fs::write(&path, bytes) {
                warn!("could not persist the project settings: {error}");
            }
        }
        Err(error) => warn!("could not encode the project settings: {error}"),
    }
}
