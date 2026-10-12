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

use std::borrow::Cow;

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

/// Migrate `text` from the Bevy minor its stamp records, or leave it as it is
/// when it carries no stamp.
pub fn migrate_stamped(text: &str) -> Cow<'_, str> {
    match read_stamp(text) {
        Some(stamp) => migrate_type_paths(text, &stamp.bevy),
        None => Cow::Borrowed(text),
    }
}

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
    if bevy_minor_lt(from_bevy, "0.20") {
        out = circular_corner_radii(out);
        // Bevy 0.20 keeps an editable text's scroll in `EditableText` itself.
        out = without_component(out, "bevy_ui::widget::text_input_layout::TextScroll(");
    }
    out
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

fn parse_minor(v: &str) -> (u32, u32) {
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
                    bevy_core_pipeline::tonemapping::Tonemapping::None";
        assert!(matches!(migrate_type_paths(text, "0.20"), Cow::Borrowed(_)));
    }

    #[test]
    fn a_stamped_019_scene_migrates_and_an_unstamped_one_does_not() {
        let body = "bevy_core_pipeline::tonemapping::Tonemapping::None";
        let stamped = format!("{STAMP_MARKER}0.19.0{STAMP_SEPARATOR}0.19\n{body}");
        assert!(migrate_stamped(&stamped).contains("bevy_render::view::Tonemapping::None"));
        assert!(matches!(migrate_stamped(body), Cow::Borrowed(_)));
    }

    #[test]
    fn minor_ordering() {
        assert!(bevy_minor_lt("0.19", "0.20"));
        assert!(!bevy_minor_lt("0.20", "0.19"));
        assert!(!bevy_minor_lt("0.19", "0.19"));
    }
}
