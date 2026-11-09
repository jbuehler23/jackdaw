//! Type-path migration for scene text written under an older Bevy.
//!
//! Saved scenes carry a leading comment recording the jackdaw and Bevy
//! versions that wrote them:
//!
//! ```text
//! // jackdaw 0.19.0 | bevy 0.19
//! ```
//!
//! The lexer treats it as a comment. When a newer Bevy moved a reflected
//! type, the recorded Bevy minor selects the renames to apply before the text
//! is parsed, so the component still resolves. Renames land on Bevy minor
//! bumps, which is the granularity the stamp records. Unstamped text, such as
//! a hand-authored scene, is read as current.
//!
//! The one structural change between the Bevy 0.19 and 0.20 forms of a
//! jackdaw scene is the separator between sibling entities: `,` before, `--`
//! now. A `,` between entities is never valid 0.20 syntax, so the loader
//! parses first and, only when that fails, rewrites the separators with
//! [`entity_separators`] and parses again. That reads 0.19 scenes stamped or
//! not, and scenes an early 0.20 build stamped while it still wrote commas,
//! without a second pass over current text. Saving writes `--` and the
//! current stamp.
//!
//! Types that move between jackdaw's own crates are listed in
//! [`MOVED_TYPES`] and renamed in every document, stamped or not.

use std::borrow::Cow;

use crate::syntax::{lex::Token, span::Span};

/// The start of a scene's version stamp line.
pub const STAMP_MARKER: &str = "// jackdaw ";
/// Between the jackdaw and the Bevy version in a stamp line.
pub const STAMP_SEPARATOR: &str = " | bevy ";

/// The versions recorded in a scene's stamp.
pub struct SceneStamp {
    /// The jackdaw version that wrote the scene, e.g. `0.19.0`.
    pub jackdaw: String,
    /// The Bevy minor the scene targeted, e.g. `0.19`.
    pub bevy: String,
}

/// Read the stamp from the head of a scene file, if present. Absent for
/// hand-authored scenes and anything written before stamping existed.
pub fn read_stamp(text: &str) -> Option<SceneStamp> {
    let line = text.lines().next()?.trim();
    let rest = line.strip_prefix(STAMP_MARKER)?;
    let (jackdaw, bevy) = rest.split_once(STAMP_SEPARATOR)?;
    Some(SceneStamp {
        jackdaw: jackdaw.trim().to_string(),
        bevy: bevy.trim().to_string(),
    })
}

/// Migrate `text` from the Bevy minor its stamp records, then rename the
/// types that moved between jackdaw crates.
pub fn migrate_stamped(text: &str) -> Cow<'_, str> {
    migrate_with(text, MOVED_TYPES)
}

fn migrate_with<'a>(text: &'a str, moved: &[(&str, &str)]) -> Cow<'a, str> {
    let out = match read_stamp(text) {
        Some(stamp) => migrate_type_paths(text, &stamp.bevy),
        None => Cow::Borrowed(text),
    };
    match owned(rename_paths(&out, moved)) {
        Some(renamed) => Cow::Owned(renamed),
        None => out,
    }
}

/// Reflected types that moved from one jackdaw crate to another, oldest
/// first, as `(old, new)` type paths. An entry ending in `::` moves every type
/// under that module. A moved type's old path is never registered again, so
/// these apply to every document whatever its stamp, including documents
/// saved by a development build of the same version.
///
/// Moving a saved type into another crate means adding its entry here and a
/// test in the new crate that loads a scene saved before the move.
pub const MOVED_TYPES: &[(&str, &str)] = &[
    (
        "jackdaw::entity_ops::EmptyEntity",
        "jackdaw_editor_core::entity_ops::EmptyEntity",
    ),
    (
        "jackdaw::entity_ops::SceneCamera",
        "jackdaw_editor_core::entity_ops::SceneCamera",
    ),
    (
        "jackdaw::entity_ops::SceneLight",
        "jackdaw_editor_core::entity_ops::SceneLight",
    ),
    (
        "jackdaw::entity_ops::SceneFogVolume",
        "jackdaw_editor_core::entity_ops::SceneFogVolume",
    ),
    (
        "jackdaw::entity_ops::SceneReflectionProbe",
        "jackdaw_editor_core::entity_ops::SceneReflectionProbe",
    ),
    (
        "jackdaw::entity_ops::SceneAnimationPlayer",
        "jackdaw_editor_core::entity_ops::SceneAnimationPlayer",
    ),
    (
        "jackdaw::entity_ops::SceneAudioSource",
        "jackdaw_editor_core::entity_ops::SceneAudioSource",
    ),
    (
        "jackdaw::reference_image::ReferenceImage",
        "jackdaw_editor_core::reference_image::ReferenceImage",
    ),
    (
        "jackdaw::recipes::Recipe",
        "jackdaw_editor_core::recipes::Recipe",
    ),
    (
        "jackdaw::recipes::RecipeInput",
        "jackdaw_editor_core::recipes::RecipeInput",
    ),
    (
        "jackdaw::recipes::RecipeCall",
        "jackdaw_editor_core::recipes::RecipeCall",
    ),
];

/// Rename every [`MOVED_TYPES`] path in `text`.
pub fn moved_types(text: &str) -> Cow<'_, str> {
    rename_paths(text, MOVED_TYPES)
}

/// The current path of a single type path that moved crate, or `None` when
/// it did not.
pub fn moved_type_path(path: &str) -> Option<String> {
    owned(moved_types(path))
}

fn owned(text: Cow<'_, str>) -> Option<String> {
    match text {
        Cow::Owned(text) => Some(text),
        Cow::Borrowed(_) => None,
    }
}

/// Apply `renames` in order. A path only matches whole: not as the tail of a
/// longer path (`my_jackdaw::`) and, unless it ends in `::`, not as the head
/// of a longer name (`Tile` in `TileSet`).
fn rename_paths<'a>(text: &'a str, renames: &[(&str, &str)]) -> Cow<'a, str> {
    let mut out = Cow::Borrowed(text);
    for (old, new) in renames {
        if let Some(renamed) = owned(rename_path(&out, old, new)) {
            out = Cow::Owned(renamed);
        }
    }
    out
}

fn rename_path<'a>(text: &'a str, old: &str, new: &str) -> Cow<'a, str> {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let prefix = old.ends_with("::");
    let mut out = String::new();
    let mut rest = 0;
    for (at, _) in text.match_indices(old) {
        let end = at + old.len();
        let starts_path = text[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !is_ident(c) && c != ':');
        let ends_name = prefix || text[end..].chars().next().is_none_or(|c| !is_ident(c));
        if starts_path && ends_name {
            out.push_str(&text[rest..at]);
            out.push_str(new);
            rest = end;
        }
    }
    if rest == 0 {
        return Cow::Borrowed(text);
    }
    out.push_str(&text[rest..]);
    Cow::Owned(out)
}

/// Rewrite the `,` that Bevy 0.19 put between sibling entities to the `--`
/// of Bevy 0.20, leaving every other comma alone.
///
/// A comma separates entities exactly when it sits outside any `{}`, `()`
/// or `<>`: entities are only listed at the top level or in a relationship's
/// `[...]`, and fields, tuple values, lists, maps and generic arguments all
/// sit inside one of those delimiters or inside a value. Text that does not
/// lex is returned as it is, for the parser to report.
pub fn entity_separators(text: &str) -> Cow<'_, str> {
    // Each open delimiter, and whether its contents are entities.
    let mut open: Vec<bool> = Vec::new();
    let mut commas = Vec::new();
    for token in Span::from(text) {
        let Ok(token) = token else {
            return Cow::Borrowed(text);
        };
        let entities = open.last().is_none_or(|&entities| entities);
        match token.token {
            Token::LBracket => open.push(entities),
            Token::LBrace | Token::LParen | Token::Less => open.push(false),
            Token::RBracket | Token::RBrace | Token::RParen | Token::Greater => {
                open.pop();
            }
            Token::Comma if entities => commas.push(token.span.offset),
            _ => {}
        }
    }
    if commas.is_empty() {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() + commas.len());
    let mut rest = 0;
    for comma in commas {
        out.push_str(&text[rest..comma]);
        out.push_str("--");
        rest = comma + 1;
    }
    out.push_str(&text[rest..]);
    Cow::Owned(out)
}

/// The Bevy minor that last changed the form of a saved value, beyond renames.
const LATEST_FORM_CHANGE: &str = "0.20";

/// Apply type-path migrations to raw scene text for a scene written under
/// `from_bevy`. Each Bevy minor's renames apply only to scenes older than
/// that minor, so opening an old scene rewrites every rename between its
/// version and the current one. When a later Bevy renames a reflected type,
/// add its old -> new path to `RENAMES` under that minor.
pub fn migrate_type_paths<'a>(text: &'a str, from_bevy: &str) -> Cow<'a, str> {
    let mut out = Cow::Borrowed(text);
    for (introduced_in, renames) in RENAMES {
        if !bevy_minor_lt(from_bevy, introduced_in) {
            continue;
        }
        for (old, new) in *renames {
            if out.contains(old) {
                out = Cow::Owned(out.replace(old, new));
            }
        }
    }
    if bevy_minor_lt(from_bevy, LATEST_FORM_CHANGE) {
        out = circular_corner_radii(out);
        // Bevy 0.20 keeps an editable text's scroll in `EditableText` itself.
        out = without_component(out, "bevy_ui::widget::text_input_layout::TextScroll(");
    }
    out
}

/// Whether [`migrate_type_paths`] has anything to do for a scene written under
/// `from_bevy`.
pub(crate) fn migrates_from(from_bevy: &str) -> bool {
    bevy_minor_lt(from_bevy, LATEST_FORM_CHANGE)
        || RENAMES
            .iter()
            .any(|(introduced_in, _)| bevy_minor_lt(from_bevy, introduced_in))
}

/// Drop every line that holds only the tuple component `opening` starts, as a
/// scene saved its no-longer-existing components.
fn without_component<'a>(text: Cow<'a, str>, opening: &str) -> Cow<'a, str> {
    if !text.contains(opening) {
        return text;
    }
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim();
        let alone = trimmed.starts_with(opening)
            && trimmed.ends_with(')')
            && trimmed.len() == opening.len() - 1 + val_len(&trimmed[opening.len() - 1..]);
        if !alone {
            out.push_str(line);
        }
    }
    Cow::Owned(out)
}

/// Type-path renames grouped by the Bevy minor that introduced them. Each
/// entry rewrites a fully-qualified reflect path, or a module prefix every
/// type under it moved with, so an older scene's components resolve against
/// the newer registry.
const RENAMES: &[(&str, &[(&str, &str)])] = &[(
    "0.20",
    &[
        (
            "bevy_core_pipeline::tonemapping::Tonemapping",
            "bevy_render::view::Tonemapping",
        ),
        (
            "bevy_core_pipeline::tonemapping::DebandDither",
            "bevy_render::view::DebandDither",
        ),
        ("bevy_feathers::cursor::", "bevy_picking::cursor::"),
        ("bevy_math::primitives::", "bevy_shape::"),
        ("bevy_math::bounding::", "bevy_shape::bounding::"),
        ("bevy_math::ray::", "bevy_shape::ray::"),
        ("bevy_math::curve::", "bevy_curve::"),
        ("bevy_math::cubic_splines::", "bevy_curve::cubic_splines::"),
        (
            "bevy_sprite::sprite_mesh::SpriteAlphaMode",
            "bevy_sprite::sprite::SpriteAlphaMode",
        ),
        (
            "bevy_sprite_render::sprite_mesh::sprite_material::SpriteMaterial",
            "bevy_sprite_render::sprite_mesh::sprite_extended_material::SpriteMaterial",
        ),
    ],
)];

/// Bevy 0.20 made each `BorderRadius` corner a two-axis `CornerRadius`. A
/// corner saved as one `Val` was circular, which is a `CornerRadius` whose
/// vertical radius is `Auto`.
fn circular_corner_radii(text: Cow<'_, str>) -> Cow<'_, str> {
    const CORNERS: [&str; 4] = [
        "top_left: ",
        "top_right: ",
        "bottom_right: ",
        "bottom_left: ",
    ];
    const VAL: &str = "bevy_ui::geometry::Val::";
    if !CORNERS
        .iter()
        .any(|corner| text.contains(&format!("{corner}{VAL}")))
    {
        return text;
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_ref();
    while let Some((at, corner)) = CORNERS
        .iter()
        .filter_map(|corner| rest.find(&format!("{corner}{VAL}")).map(|at| (at, *corner)))
        .min_by_key(|(at, _)| *at)
    {
        let value_start = at + corner.len();
        out.push_str(&rest[..value_start]);
        let value = &rest[value_start..];
        let len = val_len(value);
        out.push_str("bevy_ui::geometry::CornerRadius { x: ");
        out.push_str(&value[..len]);
        out.push_str(", y: bevy_ui::geometry::Val::Auto }");
        rest = &value[len..];
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// The length of the `Val` expression `value` starts with: a unit variant,
/// or a variant and its parenthesised payload.
fn val_len(value: &str) -> usize {
    let name_end = value
        .char_indices()
        .find(|(_, c)| !(c.is_alphanumeric() || *c == '_' || *c == ':'))
        .map_or(value.len(), |(i, _)| i);
    if !value[name_end..].starts_with('(') {
        return name_end;
    }
    let mut depth = 0usize;
    for (i, c) in value[name_end..].char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return name_end + i + 1;
                }
            }
            _ => {}
        }
    }
    value.len()
}

/// Is `major.minor` string `a` an older Bevy minor than `b`?
fn bevy_minor_lt(a: &str, b: &str) -> bool {
    parse_minor(a) < parse_minor(b)
}

/// `major.minor`, with Bevy's main branch after every release.
fn parse_minor(v: &str) -> (u32, u32) {
    if v == "main" {
        return (u32::MAX, u32::MAX);
    }
    let mut it = v.split('.');
    let major = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    let minor = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
    (major, minor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hand_authored_scene_has_no_stamp() {
        assert!(read_stamp("#Cube\njackdaw_scene_types::types::Brush {}").is_none());
    }

    #[test]
    fn baseline_migration_is_a_noop_and_does_not_allocate() {
        let text = "bevy_pbr::StandardMaterial {}";
        let out = migrate_type_paths(text, "0.19");
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out, text);
    }

    #[test]
    fn a_scene_from_bevy_main_is_newer_than_every_release() {
        assert!(bevy_minor_lt("0.20", "main"));
        assert!(!bevy_minor_lt("main", "0.20"));
        let text = "bevy_core_pipeline::tonemapping::Tonemapping::TonyMcMapface";
        assert!(matches!(migrate_type_paths(text, "main"), Cow::Borrowed(_)));
    }

    #[test]
    fn a_019_scene_takes_the_020_type_paths() {
        let text = "bevy_core_pipeline::tonemapping::Tonemapping::TonyMcMapface\n\
                    bevy_feathers::cursor::EntityCursor::System(\
                    bevy_window::cursor::system_cursor::SystemCursorIcon::Pointer)\n\
                    bevy_math::primitives::dim3::Cuboid { half_size: glam::Vec3 { x: 1.0 } }";
        let out = migrate_type_paths(text, "0.19");
        assert!(out.contains("bevy_render::view::Tonemapping::TonyMcMapface"));
        assert!(out.contains("bevy_picking::cursor::EntityCursor::System("));
        assert!(out.contains("bevy_shape::dim3::Cuboid {"));
        assert!(!out.contains("bevy_core_pipeline::tonemapping"));
    }

    #[test]
    fn a_019_corner_radius_becomes_a_circular_corner() {
        let text = "border_radius: bevy_ui::ui_node::BorderRadius {\n\
                    top_left: bevy_ui::geometry::Val::Px(6.0),\n\
                    top_right: bevy_ui::geometry::Val::Auto,\n\
                    bottom_right: bevy_ui::geometry::Val::Percent(50.0),\n\
                    bottom_left: bevy_ui::geometry::Val::Px(0.0),\n\
                    }";
        let out = migrate_type_paths(text, "0.19");
        assert!(out.contains(
            "top_left: bevy_ui::geometry::CornerRadius { x: bevy_ui::geometry::Val::Px(6.0), \
             y: bevy_ui::geometry::Val::Auto },"
        ));
        assert!(out.contains(
            "top_right: bevy_ui::geometry::CornerRadius { x: bevy_ui::geometry::Val::Auto, \
             y: bevy_ui::geometry::Val::Auto },"
        ));
        assert!(out.contains(
            "bottom_right: bevy_ui::geometry::CornerRadius { x: \
             bevy_ui::geometry::Val::Percent(50.0), y: bevy_ui::geometry::Val::Auto },"
        ));
    }

    #[test]
    fn a_019_text_scroll_is_dropped() {
        let text = "bevy_input_focus::tab_navigation::TabIndex(0)\n        \
                    bevy_ui::widget::text_input_layout::TextScroll(glam::Vec2 { x: 0.0, y: 0.0 })\n        \
                    bevy_ui::ui_node::ZIndex(0)\n";
        let out = migrate_type_paths(text, "0.19");
        assert!(!out.contains("TextScroll"), "got:\n{out}");
        assert!(out.contains("TabIndex(0)\n") && out.contains("ZIndex(0)"));
    }

    #[test]
    fn a_020_scene_is_left_as_written() {
        let text = "top_left: bevy_ui::geometry::Val::Px(6.0)\n\
                    bevy_core_pipeline::tonemapping::Tonemapping::None\n\
                    bevy_ui::experimental::ghost_hierarchy::GhostNode";
        assert!(matches!(migrate_type_paths(text, "0.20"), Cow::Borrowed(_)));
        assert!(!migrates_from("0.20"));
    }

    #[test]
    fn a_stamped_019_scene_migrates_and_an_unstamped_one_does_not() {
        let body = "bevy_core_pipeline::tonemapping::Tonemapping::None";
        let stamped = format!("{STAMP_MARKER}0.19.0{STAMP_SEPARATOR}0.19\n{body}");
        assert!(migrate_stamped(&stamped).contains("bevy_render::view::Tonemapping::None"));
        assert!(matches!(migrate_stamped(body), Cow::Borrowed(_)));
    }

    #[test]
    fn a_019_entity_comma_becomes_a_double_minus_and_value_commas_stay() {
        let text = "#Root\n\
                    bevy_ecs::hierarchy::Children [\n\
                    #A\n\
                    Wrap<a::B, a::C>(1, [2, 3])\n\
                    Holder { m: map[(\"k\", 1)], n: 2 }\n\
                    ,\n\
                    #B\n\
                    bevy_ecs::hierarchy::Children [ #C, #D ],\n\
                    ]";
        let out = entity_separators(text);
        assert_eq!(
            out,
            "#Root\n\
             bevy_ecs::hierarchy::Children [\n\
             #A\n\
             Wrap<a::B, a::C>(1, [2, 3])\n\
             Holder { m: map[(\"k\", 1)], n: 2 }\n\
             --\n\
             #B\n\
             bevy_ecs::hierarchy::Children [ #C-- #D ]--\n\
             ]"
        );
    }

    #[test]
    fn a_020_scene_keeps_its_text() {
        let text = "#A Holder { n: [1, 2] }\n--\n#B";
        assert!(matches!(entity_separators(text), Cow::Borrowed(_)));
    }

    const MOVES: &[(&str, &str)] = &[
        ("jackdaw::tilemap::Tile", "jackdaw_editor_tile::Tile"),
        ("jackdaw::terrain::", "jackdaw_editor_terrain::"),
    ];

    #[test]
    fn a_moved_type_takes_its_new_path_in_stamped_and_unstamped_scenes() {
        let body = "#Root\njackdaw::tilemap::Tile { size: 2 }\njackdaw::terrain::Brush::Raise\n";
        let want =
            "#Root\njackdaw_editor_tile::Tile { size: 2 }\njackdaw_editor_terrain::Brush::Raise\n";
        assert_eq!(migrate_with(body, MOVES), want);
        let stamped = format!("// jackdaw 0.21.0-dev | bevy main\n{body}");
        assert_eq!(
            migrate_with(&stamped, MOVES),
            format!("// jackdaw 0.21.0-dev | bevy main\n{want}")
        );
    }

    #[test]
    fn a_moved_type_is_renamed_in_strings_and_generics() {
        let text =
            r#"Track { component: "jackdaw::tilemap::Tile", list: Vec<jackdaw::tilemap::Tile> }"#;
        assert_eq!(
            rename_paths(text, MOVES),
            r#"Track { component: "jackdaw_editor_tile::Tile", list: Vec<jackdaw_editor_tile::Tile> }"#
        );
    }

    #[test]
    fn only_whole_paths_are_renamed() {
        let text =
            "jackdaw::tilemap::TileSet my_jackdaw::tilemap::Tile my_game::jackdaw::terrain::X";
        assert!(matches!(rename_paths(text, MOVES), Cow::Borrowed(_)));
    }

    #[test]
    fn a_document_on_the_new_paths_is_left_alone() {
        let current: String = MOVED_TYPES
            .iter()
            .map(|(_, new)| format!("{new}X\n"))
            .collect();
        assert!(matches!(moved_types(&current), Cow::Borrowed(_)));
        assert!(matches!(
            rename_paths("jackdaw_editor_tile::Tile", MOVES),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn minor_ordering() {
        assert!(bevy_minor_lt("0.19", "0.20"));
        assert!(!bevy_minor_lt("0.20", "0.19"));
        assert!(!bevy_minor_lt("0.19", "0.19"));
    }
}
