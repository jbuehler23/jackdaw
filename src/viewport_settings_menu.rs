//! The viewport toolbar's settings menu: view mode, what the viewport shows,
//! quality, and whether it redraws every frame and shows its stats.

use std::sync::Arc;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_feathers::icons::Icon;
use jackdaw_feathers::menu_bar::{
    OP_ACTION_PREFIX, SECTION_ACTION_PREFIX, SEPARATOR_ACTION, checked_row, menu_icon_button,
    radio_row, submenu_row,
};
use jackdaw_feathers::tokens;

use crate::lod_bar::LodColorView;
use crate::view_modes::{ViewMode, ViewModeSettings};
use crate::view_ops::{
    ViewModeOp, ViewToggleAlignmentGuidesOp, ViewToggleBoundingBoxesOp, ViewToggleColliderGizmosOp,
    ViewToggleGridOp, ViewToggleIconsOp, ViewToggleXrayOp,
};
use crate::viewport_overlays::OverlaySettings;
use crate::viewport_settings::{
    AntiAliasing, DetailDistance, QualityPreset, ShadowQuality, ViewportFrameGraphToggleOp,
    ViewportQualityPresetOp, ViewportQualitySetOp, ViewportRealtimeToggleOp, ViewportSettings,
    ViewportShowToggleOp, ViewportStatsToggleOp,
};

/// The name the settings menu's item carries, and its tooltip.
pub const VIEWPORT_SETTINGS_MENU: &str = "Viewport settings";

/// The toolbar button that opens the viewport settings menu.
pub fn viewport_settings_menu() -> impl Bundle {
    (
        menu_icon_button(
            VIEWPORT_SETTINGS_MENU,
            Icon::SlidersHorizontal,
            Arc::new(viewport_settings_rows),
        ),
        jackdaw_feathers::tooltip::Tooltip::title(VIEWPORT_SETTINGS_MENU),
    )
}

/// On the toolbar's note that the viewport redraws only when something
/// changes.
#[derive(Component)]
pub struct RealtimeOffIndicator;

/// A pause mark and "Realtime off", shown on the toolbar while the Realtime
/// setting is off.
pub fn realtime_off_indicator(icon_font: Handle<Font>, text_font: Handle<Font>) -> impl Bundle {
    (
        RealtimeOffIndicator,
        Node {
            display: Display::None,
            align_items: AlignItems::Center,
            column_gap: px(tokens::SPACING_XS),
            padding: UiRect::horizontal(px(tokens::SPACING_SM)),
            flex_shrink: 0.0,
            ..default()
        },
        jackdaw_feathers::tooltip::Tooltip::title(
            "Realtime is off: the viewport redraws only when something changes",
        ),
        children![
            (
                Text::new(String::from(Icon::Pause.unicode())),
                TextFont {
                    font: icon_font.into(),
                    font_size: tokens::TEXT_SIZE_SM,
                    ..default()
                },
                TextColor(tokens::TEXT_SECONDARY),
            ),
            (
                Text::new("Realtime off"),
                TextLayout {
                    linebreak: bevy::text::LineBreak::NoWrap,
                    ..default()
                },
                TextFont {
                    font: text_font.into(),
                    font_size: tokens::TEXT_SIZE_SM,
                    ..default()
                },
                TextColor(tokens::TEXT_SECONDARY),
            ),
        ],
    )
}

/// Show the Realtime-off note on every toolbar while the setting is off.
pub fn show_realtime_off_indicator(
    settings: Res<ViewportSettings>,
    mut indicators: Query<&mut Node, With<RealtimeOffIndicator>>,
) {
    let wanted = if settings.realtime {
        Display::None
    } else {
        Display::Flex
    };
    for mut node in &mut indicators {
        if node.display != wanted {
            node.display = wanted;
        }
    }
}

/// The menu's rows for the world as it stands.
pub fn viewport_settings_rows(world: &World) -> Vec<(String, String)> {
    let settings = world
        .get_resource::<ViewportSettings>()
        .cloned()
        .unwrap_or_default();
    let mut rows = [
        submenu_row("View Mode", view_mode_rows(world)),
        submenu_row("Show", show_rows(world, &settings)),
        submenu_row("Quality", quality_rows(&settings)),
        vec![
            separator(),
            toggle_row::<ViewportRealtimeToggleOp>(settings.realtime, "Realtime"),
            toggle_row::<ViewportStatsToggleOp>(settings.stats, "Stats"),
        ],
    ]
    .concat();
    if settings.stats {
        rows.push(toggle_row::<ViewportFrameGraphToggleOp>(
            settings.frame_graph,
            "Frame Time Graph",
        ));
    }
    rows
}

fn view_mode_rows(world: &World) -> Vec<(String, String)> {
    let view = world
        .get_resource::<ViewModeSettings>()
        .cloned()
        .unwrap_or_default();
    let lod_colors = world
        .get_resource::<LodColorView>()
        .is_some_and(|view| view.0);
    let current = ViewMode::of(&view, lod_colors);
    let mut rows: Vec<_> = ViewMode::ALL
        .into_iter()
        .map(|mode| {
            radio_row(
                mode == current,
                call(ViewModeOp::ID, &[("mode", mode.id())]),
                mode.label(),
            )
        })
        .collect();
    rows.push(separator());
    rows.push(checked_row(
        view.x_ray,
        call(ViewToggleXrayOp::ID, &[]),
        "X-Ray",
    ));
    rows
}

fn show_rows(world: &World, settings: &ViewportSettings) -> Vec<(String, String)> {
    let overlays = world
        .get_resource::<OverlaySettings>()
        .cloned()
        .unwrap_or_default();
    let colliders = world
        .get_resource::<jackdaw_avian_integration::PhysicsOverlayConfig>()
        .is_some_and(|config| config.show_colliders);
    let quality = settings.quality;
    let flag = |on: bool, name: &str, label: &str| {
        checked_row(
            on,
            call(
                ViewportShowToggleOp::ID,
                &[("flag", name), ("on", if on { "false" } else { "true" })],
            ),
            label,
        )
    };
    let overlay = |on: bool, id: &str, label: &str| checked_row(on, call(id, &[]), label);
    vec![
        section("Lighting"),
        flag(quality.sun_shadows, "sun_shadows", "Sun Shadows"),
        flag(
            quality.point_shadows,
            "point_shadows",
            "Point and Spot Shadows",
        ),
        flag(settings.sky_reflection, "sky_reflection", "Sky Reflection"),
        section("Effects"),
        flag(quality.fog, "fog", "Fog"),
        flag(
            quality.post_processing,
            "post_processing",
            "Post Processing",
        ),
        flag(quality.bloom, "bloom", "Bloom"),
        flag(
            quality.ambient_occlusion,
            "ambient_occlusion",
            "Ambient Occlusion",
        ),
        section("World"),
        flag(
            settings.terrain_detail,
            "terrain_detail",
            "Grass and Terrain Detail",
        ),
        section("Editor"),
        overlay(overlays.show_grid, ViewToggleGridOp::ID, "Grid"),
        overlay(
            overlays.show_bounding_boxes,
            ViewToggleBoundingBoxesOp::ID,
            "Bounding Boxes",
        ),
        overlay(
            overlays.show_alignment_guides,
            ViewToggleAlignmentGuidesOp::ID,
            "Alignment Guides",
        ),
        overlay(colliders, ViewToggleColliderGizmosOp::ID, "Colliders"),
        overlay(
            overlays.show_icons,
            ViewToggleIconsOp::ID,
            "Light and Camera Icons",
        ),
    ]
}

fn quality_rows(settings: &ViewportSettings) -> Vec<(String, String)> {
    let quality = settings.quality;
    let preset = settings.preset();
    let mut rows = vec![section("Preset")];
    rows.extend(QualityPreset::ALL.into_iter().map(|choice| {
        radio_row(
            preset == Some(choice),
            call(
                ViewportQualityPresetOp::ID,
                &[("preset", &choice.label().to_ascii_lowercase())],
            ),
            choice.label(),
        )
    }));
    if preset.is_none() {
        rows.push(radio_row(true, "", "Custom"));
    }

    let set = |field: &str, value: &str| call(ViewportQualitySetOp::ID, &[(field, value)]);
    rows.push(section("Render Scale"));
    rows.extend([50, 75, 100].map(|percent| {
        radio_row(
            quality.render_scale == percent,
            set("render_scale", &percent.to_string()),
            format!("{percent}%"),
        )
    }));
    rows.push(section("Anti-Aliasing"));
    rows.extend(AntiAliasing::ALL.map(|choice| {
        radio_row(
            quality.anti_aliasing == choice,
            set("anti_aliasing", &choice.label().to_ascii_lowercase()),
            choice.label(),
        )
    }));
    rows.push(section("Shadow Quality"));
    rows.extend(ShadowQuality::ALL.map(|choice| {
        radio_row(
            quality.shadow_quality == choice,
            set("shadow_quality", &choice.label().to_ascii_lowercase()),
            choice.label(),
        )
    }));
    rows.push(section("Detail and LOD Distance"));
    rows.extend(DetailDistance::ALL.map(|choice| {
        radio_row(
            quality.detail_distance == choice,
            set("detail_distance", &choice.label().to_ascii_lowercase()),
            choice.label(),
        )
    }));
    rows
}

fn toggle_row<O: Operator>(on: bool, label: &str) -> (String, String) {
    checked_row(
        on,
        call(O::ID, &[("on", if on { "false" } else { "true" })]),
        label,
    )
}

/// An operator call a menu row dispatches, its parameters as text.
fn call(id: &str, params: &[(&str, &str)]) -> String {
    let query = params
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    if query.is_empty() {
        format!("{OP_ACTION_PREFIX}{id}")
    } else {
        format!("{OP_ACTION_PREFIX}{id}?{query}")
    }
}

fn section(label: &str) -> (String, String) {
    (format!("{SECTION_ACTION_PREFIX}{label}"), String::new())
}

fn separator() -> (String, String) {
    (SEPARATOR_ACTION.to_string(), String::new())
}
