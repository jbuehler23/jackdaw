//! Reflected-type bridge into the feathers tooltip pipeline.
//!
//! Attach [`ReflectedTypeTooltip`] to any UI entity that
//! displays a reflected Bevy type. The observer derives a
//! [`Tooltip`]: short name as title, resolved type chrome
//! description as body, full type path as footer.
//!
//! Same bridge shape as the `ButtonOperatorCall` to `Tooltip` path
//! in `src/operator_tooltip.rs`.

use std::borrow::Cow;

use crate::project_types::ProjectTypes;
use crate::type_metadata::TypeMetadata;
use bevy::prelude::*;
use jackdaw_feathers::tooltip::Tooltip;

/// Source component for type-reflection-driven tooltips. Carries
/// the fully-qualified `type_path` of a Bevy reflected type that
/// has been registered in [`AppTypeRegistry`]; the auto-attach
/// observer below resolves the registry entry and inserts a
/// [`Tooltip`] derived from it.
#[derive(Component, Clone, Debug)]
pub struct ReflectedTypeTooltip {
    pub type_path: Cow<'static, str>,
}

impl ReflectedTypeTooltip {
    pub fn new(type_path: impl Into<Cow<'static, str>>) -> Self {
        Self {
            type_path: type_path.into(),
        }
    }
}

pub(super) fn plugin(app: &mut App) {
    app.add_observer(auto_attach_reflected_type_tooltip);
}

/// Derive a [`Tooltip`] from the type registry entry pointed at by
/// a freshly-added [`ReflectedTypeTooltip`] and insert it on the
/// same entity. Falls back to the project schema when the type is not
/// in the editor registry.
fn auto_attach_reflected_type_tooltip(
    trigger: On<Add<ReflectedTypeTooltip>>,
    sources: Query<&ReflectedTypeTooltip>,
    type_registry: Res<AppTypeRegistry>,
    project_types: Res<ProjectTypes>,
    type_metadata: Res<TypeMetadata>,
    mut commands: Commands,
) {
    let entity = trigger.event_target();
    let Ok(source) = sources.get(entity) else {
        return;
    };
    let type_path = source.type_path.as_ref();
    let registry = type_registry.read();
    let title = if let Some(registration) = registry.get_with_type_path(type_path) {
        registration
            .type_info()
            .type_path_table()
            .short_path()
            .to_string()
    } else if let Some(schema) = project_types.component(type_path) {
        schema.short_name.clone()
    } else {
        return;
    };
    let description = type_metadata
        .resolve(type_path, &registry, &project_types)
        .description;
    let footer = source.type_path.to_string();
    commands.entity(entity).insert(
        Tooltip::title(title)
            .with_description(plain_doc(&description))
            .with_footer(footer),
    );
}

/// Doc text as a tooltip reads it: the paragraphs before the first heading or
/// code block, each on one line, without link targets or code marks.
fn plain_doc(docs: &str) -> String {
    let mut paragraphs: Vec<String> = Vec::new();
    let mut current: Vec<&str> = Vec::new();
    for line in docs.lines().map(str::trim) {
        if line.starts_with('#') || line.starts_with("```") {
            break;
        }
        if line.is_empty() {
            if !current.is_empty() {
                paragraphs.push(current.join(" "));
                current.clear();
            }
        } else {
            current.push(line);
        }
    }
    if !current.is_empty() {
        paragraphs.push(current.join(" "));
    }
    paragraphs
        .iter()
        .map(|paragraph| strip_links(paragraph).replace('`', ""))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// Links as their text: `[text](target)`, and an intra-doc link `` [`Foo`] ``
/// with or without a `[target]` after it. Other brackets, and anything inside a
/// code span, stay as written.
fn strip_links(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(['[', '`']) {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        if rest.starts_with('`') {
            let end = rest[1..].find('`').map_or(rest.len(), |close| close + 2);
            out.push_str(&rest[..end]);
            rest = &rest[end..];
        } else if let Some((shown, after)) = link(rest) {
            out.push_str(shown);
            rest = after;
        } else {
            out.push('[');
            rest = &rest[1..];
        }
    }
    out.push_str(rest);
    out
}

/// The text of the link `text` opens with, and what follows the link.
fn link(text: &str) -> Option<(&str, &str)> {
    let close = text.find(']')?;
    let shown = &text[1..close];
    let after = &text[close + 1..];
    if let Some(target) = after.strip_prefix('(') {
        let mut depth = 1;
        for (at, letter) in target.char_indices() {
            match letter {
                '(' => depth += 1,
                ')' if depth == 1 => return Some((shown, &target[at + 1..])),
                ')' => depth -= 1,
                _ => {}
            }
        }
        return None;
    }
    let code = shown.len() > 2
        && shown.starts_with('`')
        && shown.ends_with('`')
        && !shown[1..shown.len() - 1].contains('`');
    if !code {
        return None;
    }
    let after = after
        .strip_prefix('[')
        .and_then(|target| target.find(']').map(|end| &target[end + 1..]))
        .unwrap_or(after);
    Some((shown, after))
}

#[cfg(test)]
mod tests {
    use super::plain_doc;

    #[test]
    fn doc_text_loses_its_markdown_and_hard_wraps() {
        let docs = " A map of tiles.\n Layers are painted\n in order; see [`Tilemap::data_path`] and [the docs](https://example.com).\n\n Second `part`.\n\n # Examples\n ```\n code\n ```";
        assert_eq!(
            plain_doc(docs),
            "A map of tiles. Layers are painted in order; see Tilemap::data_path and the docs.\n\nSecond part."
        );
        assert_eq!(plain_doc("Plain."), "Plain.");
        assert_eq!(
            plain_doc("[`Sprite`][crate::Sprite] draws."),
            "Sprite draws."
        );
    }

    #[test]
    fn only_link_syntax_loses_its_brackets() {
        assert_eq!(
            plain_doc("See [the guide](https://example.com/a_(b)) first."),
            "See the guide first."
        );
        assert_eq!(
            plain_doc("Reads `values[0]` first."),
            "Reads values[0] first."
        );
        assert_eq!(
            plain_doc("A ratio in [0, 1] or [0.5, 2)."),
            "A ratio in [0, 1] or [0.5, 2)."
        );
        assert_eq!(plain_doc("Like [`Foo`] does."), "Like Foo does.");
    }
}
