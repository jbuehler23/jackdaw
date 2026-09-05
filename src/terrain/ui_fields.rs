//! Small building blocks shared by the terrain options bar, the Terrain panel,
//! and the Scatter section: numeric fields, a tile grid, and their labels and
//! hints.
//!
//! Two field shapes, one per container:
//!
//! - [`spawn_slider_row`]: the terrain spelling of the editor-wide
//!   [`jackdaw_feathers::slider_row`], so a terrain field's label sits in the
//!   same column, and its control is the same widget, as every other continuous
//!   value in the editor. Used by the Terrain panel and the Scatter section,
//!   where fields stack down a dock-panel sidebar.
//! - [`spawn_scrub_chip`]: a compact inline label and value chip sized to its
//!   content, sitting beside other chips on the options bar's single wrapping
//!   row. Built on jackdaw's `ScrubNumberInput`; see that type's docs for what
//!   a native widget does not cover.
//!
//! Neither shows its description as a permanent line: both attach it as a
//! [`Tooltip`], read on hover, so the field stays compact.

use std::ops::Range;

use bevy::feathers::controls::FeathersCheckbox;
use bevy::feathers::theme::ThemedText;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::text::FontSource;
use bevy::ui::Checked;
use jackdaw_api::prelude::*;
use jackdaw_feathers::icons::FeathersDefaultFont;
use jackdaw_feathers::number_input::{
    NumberInputPrecision, ScrubNumberInput, ScrubNumberInputValue, SoftLimit,
};
pub(super) use jackdaw_feathers::slider_row::FieldKind;
use jackdaw_feathers::slider_row::SliderRowProps;
use jackdaw_feathers::tokens;
use jackdaw_feathers::tooltip::Tooltip;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(Update, apply_terrain_default_font);
}

/// Marker for the root of a terrain UI surface: the options bar, the Terrain
/// panel, the tool palette.
///
/// Any plain-label `Text` spawned underneath one, meaning one that never
/// requested an explicit font and so still carries the sentinel
/// `FontSource::default()`, is pinned to `bevy::feathers`'s default body font
/// by [`apply_terrain_default_font`] rather than jackdaw's ambient-default
/// override; see `FeathersDefaultFont`'s docs for how those differ. Icon glyphs
/// (`font_paths::LUCIDE`) and any `Text` that sets its own `font:` are
/// untouched, since they never carry the sentinel.
#[derive(Component, Clone, Default)]
pub(super) struct TerrainDefaultFontRoot;

/// How many `ChildOf` hops to walk looking for a [`TerrainDefaultFontRoot`]
/// before giving up. Deep enough for every terrain surface's nesting (row to
/// label, tile to swatch to label) without an unbounded walk for text that is
/// not terrain UI.
const ROOT_SEARCH_DEPTH: u8 = 8;

/// See [`TerrainDefaultFontRoot`]. Runs once per newly-spawned `TextFont`
/// rather than continuously, so a caller that changes its own font after
/// spawning keeps it.
fn apply_terrain_default_font(
    mut spawned: Query<(Entity, &mut TextFont), Added<TextFont>>,
    parents: Query<&ChildOf>,
    roots: Query<(), With<TerrainDefaultFontRoot>>,
    default_font: Res<FeathersDefaultFont>,
) {
    for (entity, mut text_font) in &mut spawned {
        if text_font.font != FontSource::default() {
            continue;
        }
        let mut current = entity;
        let mut under_root = false;
        for _ in 0..ROOT_SEARCH_DEPTH {
            if roots.contains(current) {
                under_root = true;
                break;
            }
            let Ok(&ChildOf(parent)) = parents.get(current) else {
                break;
            };
            current = parent;
        }
        if under_root {
            text_font.font = FontSource::Handle(default_font.0.clone());
        }
    }
}

/// Width of the value and scrub region in a [`spawn_scrub_chip`] chip.
/// Narrower than a panel row's, since several chips share one bar line.
const SCRUB_CHIP_INPUT_WIDTH: f32 = 64.0;

/// The terrain spelling of [`jackdaw_feathers::slider_row`]: every terrain
/// slider row carries a description, so the tooltip is a plain argument rather
/// than optional, and the returned entity is the slider the caller re-inserts
/// `SliderValue` on. See `sync_gen_fields` in `panel.rs` and
/// `sync_scatter_fields` in `scatter.rs`.
pub(super) fn spawn_slider_row<C: Component>(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    tooltip: &str,
    value: f32,
    range: Range<f32>,
    kind: FieldKind,
    field: C,
) -> Entity {
    jackdaw_feathers::slider_row::spawn_slider_row(
        commands,
        parent,
        SliderRowProps::new(label, value, range)
            .with_tooltip(tooltip)
            .with_kind(kind),
        field,
    )
    .slider
}

/// A compact inline label and value chip sized to its content: a
/// `ScrubNumberInput` tagged with `field`, sitting beside sibling chips on one
/// options-bar row. Same binding, tooltip and [`FieldKind`] contract as
/// [`spawn_slider_row`], different container: no `width: 100%`, so the bar's
/// `FlexDirection::Row` lays chips out side by side rather than one full-width
/// bar per field.
///
/// `kind` reaches the widget as its `NumberInputPrecision`, the same number
/// `FieldKind` gives a slider row's `SliderPrecision`, so a value shows the
/// same digits in either shape.
pub(super) fn spawn_scrub_chip<C: Component>(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    tooltip: &str,
    value: f32,
    soft_range: Range<f32>,
    kind: FieldKind,
    field: C,
) {
    let chip = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(tokens::SPACING_XS),
                flex_shrink: 0.0,
                ..Default::default()
            },
            ChildOf(parent),
            Tooltip::title(label).with_description(tooltip),
            Hovered::default(),
        ))
        .id();

    commands.spawn((
        Text::new(label),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(chip),
    ));

    commands
        .spawn_scene(bsn! {
            @ScrubNumberInput
            Node {
                width: px(SCRUB_CHIP_INPUT_WIDTH),
                height: px(22.0),
                flex_shrink: 0.0,
            }
        })
        .insert((
            ScrubNumberInputValue::F32(value),
            SoftLimit::f32(soft_range),
            NumberInputPrecision(kind.precision()),
            field,
            ChildOf(chip),
        ));
}

/// A labeled native `bevy_feathers` checkbox, tagged with `field` so a caller's
/// `ValueChange<bool>` observer can tell it apart from every other checkbox in
/// the editor.
///
/// `FeathersCheckbox` does not self-manage `Checked`: this seeds the initial
/// state from `checked`, and the caller's commit handler has to insert or
/// remove it on every commit.
pub(super) fn spawn_checkbox<C: Component>(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    checked: bool,
    field: C,
) {
    let label = label.to_string();
    let mut cb = commands.spawn_scene(bsn! {
        @FeathersCheckbox { @caption: bsn! { Text(label) ThemedText } }
    });
    cb.insert((field, ChildOf(parent)));
    if checked {
        cb.insert(Checked);
    }
}

// --- Tile grid ---

/// Edge of one tile (channel, palette value, or scatter asset).
const TILE_PX: f32 = 52.0;
/// Height of the accent bar marking the selected tile.
const ACCENT_PX: f32 = 3.0;

pub(super) fn spawn_hint(commands: &mut Commands, parent: Entity, text: &str) {
    commands.spawn((
        Text::new(text),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(parent),
    ));
}

/// Same as [`spawn_hint`], coloured for a typed error or a quarantine notice: a
/// refused material change, a bad texture-set reference, a load failure.
pub(super) fn spawn_error_hint(commands: &mut Commands, parent: Entity, text: &str) {
    commands.spawn((
        Text::new(text),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
            ..Default::default()
        },
        TextColor(tokens::TEXT_ERROR),
        ChildOf(parent),
    ));
}

pub(super) fn spawn_tile_grid(commands: &mut Commands, parent: Entity) -> Entity {
    commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                column_gap: px(tokens::SPACING_XS),
                row_gap: px(tokens::SPACING_XS),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id()
}

/// The tile container every tile grid entry shares, before its swatch is added.
/// Split out so a colour swatch ([`spawn_tile`]) and an image thumbnail
/// ([`spawn_texture_tile`]) share the accent bar, label and click dispatch.
fn spawn_tile_frame(commands: &mut Commands, parent: Entity) -> Entity {
    commands
        .spawn((
            Node {
                width: px(TILE_PX),
                flex_direction: FlexDirection::Column,
                row_gap: px(2.0),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id()
}

/// Characters a tile label shows before eliding. What [`TILE_PX`] holds at
/// `TEXT_SIZE_XS`.
const TILE_LABEL_CHARS: usize = 9;

/// Shorten `label` to `max` characters, marking that it was cut.
fn elide(label: &str, max: usize) -> String {
    if label.chars().count() <= max {
        return label.to_string();
    }
    let kept: String = label.chars().take(max.saturating_sub(1)).collect();
    format!("{kept}...")
}

/// The accent bar, label and click dispatch shared by every tile-grid entry,
/// whatever its swatch shows.
fn finish_tile(
    commands: &mut Commands,
    tile: Entity,
    label: &str,
    selected: bool,
    op_id: Option<&'static str>,
    index: Option<usize>,
) {
    commands.spawn((
        Node {
            width: Val::Percent(100.0),
            height: px(ACCENT_PX),
            ..Default::default()
        },
        BackgroundColor(if selected {
            tokens::ACCENT_BLUE
        } else {
            Color::NONE
        }),
        ChildOf(tile),
    ));

    // Elided to what the tile is wide enough to hold: a longer label has no
    // whitespace to wrap at, so it would run under its neighbour's. The full
    // text stays reachable on hover.
    commands.spawn((
        Text::new(elide(label, TILE_LABEL_CHARS)),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
            ..Default::default()
        },
        TextColor(if selected {
            tokens::TEXT_BODY_COLOR.into()
        } else {
            tokens::TEXT_SECONDARY
        }),
        Node {
            max_width: px(TILE_PX),
            overflow: Overflow::clip(),
            ..Default::default()
        },
        Hovered::default(),
        Tooltip::title(label.to_string()),
        ChildOf(tile),
    ));

    // A tile with no operator behind it has no click observer at all, rather
    // than a disabled flag.
    let Some(op_id) = op_id else {
        return;
    };
    commands
        .entity(tile)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            let mut call = commands.operator(op_id).settings(tile_dispatch_settings());
            if let Some(index) = index {
                call = call.param("index", index as i64);
            }
            call.call();
        });
}

/// One selectable tile: colour swatch, accent bar, name under it.
/// Clicking dispatches `op_id` with the tile's index.
pub(super) fn spawn_tile(
    commands: &mut Commands,
    parent: Entity,
    swatch: Color,
    label: &str,
    selected: bool,
    op_id: &'static str,
    index: Option<usize>,
) {
    let tile = spawn_tile_frame(commands, parent);
    commands.spawn((
        Node {
            width: Val::Percent(100.0),
            height: px(TILE_PX * 0.62),
            ..Default::default()
        },
        BackgroundColor(swatch),
        ChildOf(tile),
    ));
    finish_tile(commands, tile, label, selected, Some(op_id), index);
}

/// One tile with an image thumbnail instead of a colour swatch, as the Textures
/// tab's per-slot grid uses.
///
/// `thumbnail` is `None` while the albedo image is loading, or when the slot
/// has no material to show one for, which draws as a solid placeholder rather
/// than leaving the tile blank.
///
/// `op_id` is `None` for a tile that is not selectable: a vacated texture id,
/// which holds a place in the grid rather than something the brush can be
/// loaded with.
///
/// `badge` marks what a tile *is* rather than what it shows, and rides on the
/// thumbnail so the label underneath still holds the material's name.
pub(super) fn spawn_texture_tile(
    commands: &mut Commands,
    parent: Entity,
    thumbnail: Option<Handle<Image>>,
    label: &str,
    selected: bool,
    badge: Option<&str>,
    op_id: Option<&'static str>,
    index: Option<usize>,
) {
    let tile = spawn_tile_frame(commands, parent);
    let mut swatch = commands.spawn((
        Node {
            width: Val::Percent(100.0),
            height: px(TILE_PX * 0.62),
            ..Default::default()
        },
        BackgroundColor(tokens::INPUT_BG),
        ChildOf(tile),
    ));
    if let Some(image) = thumbnail {
        swatch.insert(ImageNode::new(image));
    }
    let swatch = swatch.id();
    if let Some(badge) = badge {
        spawn_tile_badge(commands, swatch, badge);
    }
    finish_tile(commands, tile, label, selected, op_id, index);
}

/// A small pill in the corner of a tile's thumbnail. Absolute so it overlays
/// the image rather than taking a line of the 52px tile's height.
fn spawn_tile_badge(commands: &mut Commands, swatch: Entity, text: &str) {
    let pill = commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(2.0),
                top: px(2.0),
                padding: UiRect::axes(px(3.0), px(1.0)),
                border_radius: BorderRadius::all(px(3.0)),
                ..Default::default()
            },
            BackgroundColor(tokens::ELEVATED_BG),
            ChildOf(swatch),
        ))
        .id();
    commands.spawn((
        Text::new(text.to_string()),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
            ..Default::default()
        },
        TextColor(tokens::TEXT_BODY_COLOR.into()),
        ChildOf(pill),
    ));
}

/// Edge of the inline material swatch the options bar shows beside a texture's
/// name. Sized to the bar's row rather than to a grid tile, since the bar is
/// one line high.
const BAR_SWATCH_PX: f32 = 18.0;

/// A single material's albedo as a small square, for a readout that names a
/// texture. `None`, meaning still loading or a slot with no material, draws the
/// same square filled, so a row of swatches keeps its shape while thumbnails
/// arrive.
pub(super) fn spawn_bar_swatch(
    commands: &mut Commands,
    parent: Entity,
    thumbnail: Option<Handle<Image>>,
) {
    let mut swatch = commands.spawn((
        Node {
            width: px(BAR_SWATCH_PX),
            height: px(BAR_SWATCH_PX),
            flex_shrink: 0.0,
            border_radius: BorderRadius::all(px(3.0)),
            ..Default::default()
        },
        BackgroundColor(tokens::INPUT_BG),
        ChildOf(parent),
    ));
    if let Some(image) = thumbnail {
        swatch.insert(ImageNode::new(image));
    }
}

/// Dispatch settings shared by every tile-grid affordance (select, add,
/// remove). Matches the dispatcher in `core_extension.rs`, so history entries
/// and tab dirtiness follow a tile click as they do a toolbar button click.
/// Operators that opt out (`allows_undo = false`) are unaffected.
fn tile_dispatch_settings() -> CallOperatorSettings {
    CallOperatorSettings {
        creates_history_entry: true,
        execution_context: ExecutionContext::Invoke,
    }
}

/// The `+` tile that adds a layer, sitting at the end of the grid.
pub(super) fn spawn_add_tile(commands: &mut Commands, parent: Entity, op_id: &'static str) {
    let tile = commands
        .spawn((
            Node {
                width: px(TILE_PX),
                height: px(TILE_PX * 0.62),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border: UiRect::all(px(1.0)),
                ..Default::default()
            },
            BorderColor::all(tokens::TEXT_SECONDARY.with_alpha(0.4)),
            ChildOf(parent),
        ))
        .id();
    commands.spawn((
        Text::new("+"),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(tile),
    ));
    commands
        .entity(tile)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            commands
                .operator(op_id)
                .settings(tile_dispatch_settings())
                .call();
        });
}

/// A small remove affordance beside the tile rather than on it, so the swatch
/// stays an unobstructed colour sample.
///
/// `op_id` is dispatched with the tile's index, so the same affordance serves
/// the channel grid and the scatter palette.
pub(super) fn spawn_tile_remove(
    commands: &mut Commands,
    parent: Entity,
    op_id: &'static str,
    index: usize,
) {
    let button = commands
        .spawn((
            Node {
                width: px(14.0),
                height: px(TILE_PX * 0.62),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();
    commands.spawn((
        Text::new("x"),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(button),
    ));
    commands
        .entity(button)
        .observe(move |_: On<Pointer<Click>>, mut commands: Commands| {
            commands
                .operator(op_id)
                .param("index", index as i64)
                .settings(tile_dispatch_settings())
                .call();
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tile is 52px wide with no room to wrap, so a long name is cut rather
    /// than allowed to run under the next tile's label.
    #[test]
    fn tile_labels_are_elided_only_when_they_do_not_fit() {
        assert_eq!(elide("grass", TILE_LABEL_CHARS), "grass");
        assert_eq!(elide("audit_gro", TILE_LABEL_CHARS), "audit_gro");
        assert_eq!(elide("audit_ground", TILE_LABEL_CHARS), "audit_gr...");
    }

    /// A chip and a slider row are the same field in two containers, so the
    /// digits a value shows come from its [`FieldKind`] either way.
    #[test]
    fn a_chip_takes_its_digits_from_the_same_field_kind_a_slider_row_does() {
        assert_eq!(
            FieldKind::Continuous.precision(),
            jackdaw_feathers::slider_row::CONTINUOUS_PRECISION,
        );
        assert_eq!(FieldKind::Count.precision(), 0);
    }
}
