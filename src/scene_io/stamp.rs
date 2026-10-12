//! Scene version stamp plus type-path migration on load.
//!
//! Saved scenes carry a leading comment recording the jackdaw and Bevy
//! versions that wrote them:
//!
//! ```text
//! // jackdaw 0.19.0 | bevy 0.19
//! ```
//!
//! The BSN lexer treats it as a comment, so it never reaches the parsed
//! document; the save regenerates it and the load scans the raw text for
//! it. Its purpose is provenance: when a future jackdaw built against a
//! newer Bevy opens an older scene, the recorded Bevy version selects the
//! type-path renames to apply, so a component whose reflect path moved in
//! a later Bevy still resolves. Bevy renames land on minor bumps, and the
//! stamp records the Bevy minor, so that is the granularity migration
//! keys on.

use std::borrow::Cow;

use jackdaw_project_build::{BEVY_VERSION, VERSION};

const MARKER: &str = "// jackdaw ";
const SEPARATOR: &str = " | bevy ";

/// The versions recorded in a scene's stamp.
pub struct SceneStamp {
    /// The jackdaw version that wrote the scene, e.g. `0.19.0`.
    pub jackdaw: String,
    /// The Bevy minor the scene targeted, e.g. `0.19`.
    pub bevy: String,
}

/// Prepend the current version stamp to emitted scene text. Applied only
/// at the disk-write boundary, never to the in-memory snapshots undo and
/// tab-swap take from the same emitter.
pub fn with_stamp(body: &str) -> String {
    format!("{MARKER}{VERSION}{SEPARATOR}{BEVY_VERSION}\n{body}")
}

/// Read the stamp from the head of a scene file, if present. Absent for
/// hand-authored scenes and anything written before stamping existed;
/// those are treated as the current version (no migration).
pub fn read_stamp(text: &str) -> Option<SceneStamp> {
    let line = text.lines().next()?.trim();
    let rest = line.strip_prefix(MARKER)?;
    let (jackdaw, bevy) = rest.split_once(SEPARATOR)?;
    Some(SceneStamp {
        jackdaw: jackdaw.trim().to_string(),
        bevy: bevy.trim().to_string(),
    })
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
    }
    out
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
    fn stamp_round_trips() {
        let stamped = with_stamp("#Cube\njackdaw_scene_types::types::Brush {}");
        assert!(stamped.starts_with(MARKER));
        let stamp = read_stamp(&stamped).expect("stamp present");
        assert_eq!(stamp.jackdaw, VERSION);
        assert_eq!(stamp.bevy, BEVY_VERSION);
    }

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
    fn a_020_scene_is_left_as_written() {
        let text = "top_left: bevy_ui::geometry::Val::Px(6.0)\n\
                    bevy_core_pipeline::tonemapping::Tonemapping::None";
        assert!(matches!(migrate_type_paths(text, "0.20"), Cow::Borrowed(_)));
    }

    #[test]
    fn minor_ordering() {
        assert!(bevy_minor_lt("0.19", "0.20"));
        assert!(!bevy_minor_lt("0.20", "0.19"));
        assert!(!bevy_minor_lt("0.19", "0.19"));
    }
}
