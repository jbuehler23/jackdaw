//! Editor display metadata as Bevy reflect custom attributes.
//! The picker reads them via
//! `type_info.custom_attributes().get::<T>()` and falls back to
//! the type's reflected doc comment (workspace bevy
//! `reflect_documentation` feature).
//!
//! ```ignore
//! use bevy::prelude::*;
//! use jackdaw_jsn::EditorCategory;
//!
//! /// Spawns the player entity.
//! #[derive(Component, Reflect, Default)]
//! #[reflect(Component, Default, @EditorCategory("Actor"))]
//! pub struct PlayerSpawn;
//! ```
//!
//! `jackdaw_runtime` and `jackdaw` re-export both newtypes
//! through their preludes.

use bevy::prelude::*;
use std::borrow::Cow;

/// Picker grouping for a component. Attach via
/// `#[reflect(@EditorCategory("Your Group"))]`.
#[derive(Reflect, Clone, Debug, PartialEq, Eq)]
pub struct EditorCategory(pub Cow<'static, str>);

impl EditorCategory {
    pub const fn new(name: &'static str) -> Self {
        EditorCategory(Cow::Borrowed(name))
    }
}

impl From<&'static str> for EditorCategory {
    fn from(value: &'static str) -> Self {
        EditorCategory(Cow::Borrowed(value))
    }
}

impl From<String> for EditorCategory {
    fn from(value: String) -> Self {
        EditorCategory(Cow::Owned(value))
    }
}

/// Picker tooltip override. Falls back to the reflected doc
/// comment when absent.
#[derive(Reflect, Clone, Debug, PartialEq, Eq)]
pub struct EditorDescription(pub Cow<'static, str>);

impl EditorDescription {
    pub const fn new(text: &'static str) -> Self {
        EditorDescription(Cow::Borrowed(text))
    }
}

impl From<&'static str> for EditorDescription {
    fn from(value: &'static str) -> Self {
        EditorDescription(Cow::Borrowed(value))
    }
}

impl From<String> for EditorDescription {
    fn from(value: String) -> Self {
        EditorDescription(Cow::Owned(value))
    }
}
