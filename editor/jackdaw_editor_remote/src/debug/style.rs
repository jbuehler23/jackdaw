//! Shared styling for the Native Remote Debugger's introspection panels
//! (Archetypes, Schedules, Queries): the component/set chip, the entity-count
//! bar, and the muted meta row every panel opens with. Keeping these in one
//! module is what makes the three panels read as one tool instead of three.

use bevy::prelude::*;

use jackdaw_feathers::tokens;

/// The final `::` segment of a reflect type path, with any generic argument
/// list stripped first so a generic wrapper shows its own name rather than
/// its last type parameter's name: `bevy_pbr::MeshMaterial3d<bevy_pbr::StandardMaterial>`
/// -> `MeshMaterial3d`; `bevy_transform::components::transform::Transform` ->
/// `Transform`; a bare name with no separators passes through unchanged.
pub fn short_type_name(type_path: &str) -> String {
    let base = match type_path.find('<') {
        Some(idx) => &type_path[..idx],
        None => type_path,
    };
    base.rsplit("::").next().unwrap_or(base).to_string()
}

/// A small rounded pill for a component or system-set type path: the one chip
/// shared by the Archetypes table, the Schedules system boxes, and the
/// Queries builder/results, so all three read the same way.
pub fn component_chip(type_path: &str) -> impl Bundle + use<> {
    (
        Node {
            padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)),
            border_radius: BorderRadius::all(tokens::CORNER_RADIUS),
            flex_shrink: 0.0,
            ..default()
        },
        BackgroundColor(tokens::ACCENT_BLUE.with_alpha(0.14)),
        children![(
            Text::new(short_type_name(type_path)),
            TextFont {
                font_size: tokens::TEXT_SIZE_XS,
                ..default()
            },
            TextColor(tokens::TEXT_ACCENT),
        )],
    )
}

/// A translucent accent fill spanning `fraction` (clamped to `0..1`) of its
/// parent's width and the parent's full height. Spawn it as the first child
/// of a fixed-width, relatively positioned cell, before the value text, so
/// the number reads on top of the bar.
pub fn count_bar(fraction: f32) -> impl Bundle {
    let fraction = fraction.clamp(0.0, 1.0);
    (
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(0.0),
            top: Val::Px(0.0),
            bottom: Val::Px(0.0),
            width: Val::Percent(fraction * 100.0),
            ..default()
        },
        BackgroundColor(tokens::ACCENT_BLUE.with_alpha(0.16)),
    )
}

/// A panel's top row: a muted `subtitle` on the left and a right-aligned
/// muted `right` meta string. Never a title; the dock tab already shows one.
pub fn panel_meta(subtitle: &str, right: &str) -> impl Bundle + use<> {
    (
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            width: Val::Percent(100.0),
            column_gap: Val::Px(tokens::SPACING_MD),
            padding: UiRect::axes(Val::Px(tokens::SPACING_MD), Val::Px(tokens::SPACING_SM)),
            ..default()
        },
        children![
            (
                Text::new(subtitle.to_string()),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..default()
                },
                TextColor(tokens::TEXT_SECONDARY),
            ),
            (
                Text::new(right.to_string()),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..default()
                },
                TextColor(tokens::TEXT_SECONDARY),
            ),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_type_name_strips_generics_then_takes_final_segment() {
        assert_eq!(
            short_type_name("bevy_pbr::MeshMaterial3d<bevy_pbr::StandardMaterial>"),
            "MeshMaterial3d"
        );
    }

    #[test]
    fn short_type_name_takes_final_segment_with_no_generics() {
        assert_eq!(
            short_type_name("bevy_transform::components::transform::Transform"),
            "Transform"
        );
    }

    #[test]
    fn short_type_name_passes_bare_name_through() {
        assert_eq!(short_type_name("Foo"), "Foo");
    }
}
