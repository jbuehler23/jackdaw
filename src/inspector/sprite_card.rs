//! The `Sprite` card: the sprite's own fields, its anchor as a preset with a
//! custom offset beside it, and a row that sizes it from the pixels per unit
//! its art was drawn at.

use bevy::{ecs::system::SystemState, prelude::*, sprite::Anchor, ui_widgets::ValueChange};
use jackdaw_api::prelude::*;
use jackdaw_feathers::{
    button::{ButtonClickEvent, ButtonProps, button},
    combobox::{
        ComboBoxChangeEvent, ComboBoxOptionData, ComboBoxSelectedIndex, combobox_with_selected,
    },
    field_row::{FieldRowProps, spawn_field_row},
    icons::{EditorFont, IconFont},
    number_input::{NumberInputPrecision, ScrubNumberInput, ScrubNumberInputValue},
};

use crate::commands::FieldWritten;
use crate::remote::entity_browser::RemoteEntityProxy;
use crate::sprite_props::{ANCHOR_PRESETS, SpriteSetAnchorOp, SpriteSetPixelsPerUnitOp};

/// Combo index of the Custom option, after the presets.
pub const CUSTOM_ANCHOR: usize = ANCHOR_PRESETS.len();

/// One row of the card.
#[derive(Clone, Copy, PartialEq)]
enum CardRow<'a> {
    /// A `Sprite` field, by name.
    Field(&'a str),
    /// The anchor preset combo and its custom offset.
    Anchor,
    /// Size from pixels per unit.
    SizeFromPpu,
}

/// The card's rows top to bottom; `Sprite` fields it does not name follow.
const ROW_ORDER: [CardRow<'static>; 10] = [
    CardRow::Field("image"),
    CardRow::Anchor,
    CardRow::Field("flip_x"),
    CardRow::Field("flip_y"),
    CardRow::Field("custom_size"),
    CardRow::SizeFromPpu,
    CardRow::Field("color"),
    CardRow::Field("rect"),
    CardRow::Field("texture_atlas"),
    CardRow::Field("image_mode"),
];

/// Sprites whose anchor the card's own offset row is writing, so the write it
/// announces keeps the combo on Custom.
#[derive(Resource, Default)]
struct OwnOffsetEdits(bevy::platform::collections::HashSet<Entity>);

/// The anchor preset combo of the card showing `source`.
#[derive(Component)]
pub struct SpriteAnchorCombo {
    pub source: Entity,
}

/// The row holding the custom anchor offset, shown while the combo reads
/// Custom.
#[derive(Component)]
pub struct SpriteCustomAnchor {
    pub source: Entity,
}

/// The pixels per unit the size row applies, before it is applied.
#[derive(Component)]
pub struct SpritePpuInput;

/// The button that sizes the card's sprite from the number beside it.
#[derive(Component)]
pub struct SpritePpuApply {
    pub input: Entity,
    pub source: Entity,
}

/// Pixels per unit the size row starts at: one image pixel per world unit.
const DEFAULT_PPU: f64 = 1.0;

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<OwnOffsetEdits>()
        .add_observer(on_anchor_pick)
        .add_observer(mark_own_offset_edit)
        .add_observer(follow_anchor_write)
        .add_observer(on_ppu_number)
        .add_observer(on_ppu_apply);
}

fn anchor_type_path() -> &'static str {
    <Anchor as bevy::reflect::TypePath>::type_path()
}

/// The combo index showing `anchor`: its preset, or Custom.
pub fn anchor_index(anchor: Anchor) -> usize {
    ANCHOR_PRESETS
        .iter()
        .position(|(_, preset)| *preset == anchor)
        .unwrap_or(CUSTOM_ANCHOR)
}

/// `bottom_left` as the combo shows it, `Bottom Left`.
fn preset_label(name: &str) -> String {
    name.split('_')
        .map(|word| {
            let mut letters = word.chars();
            letters.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + letters.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn custom_display(index: usize) -> Display {
    if index == CUSTOM_ANCHOR {
        Display::Flex
    } else {
        Display::None
    }
}

/// Fill the `Sprite` card body for `source` from the live components.
/// World-exclusive and deferred by the caller.
pub(crate) fn fill_sprite_card_body(world: &mut World, source: Entity, body: Entity) {
    if world.get_entity(body).is_err() {
        return;
    }
    let Some(sprite) = world.get::<Sprite>(source).cloned() else {
        return;
    };
    let anchor = world.get::<Anchor>(source).copied();
    let icon_font = world
        .get_resource::<IconFont>()
        .map(|font| font.0.clone())
        .unwrap_or_default();
    let editor_font = world
        .get_resource::<EditorFont>()
        .map(|font| font.0.clone())
        .unwrap_or_default();
    let registry = world
        .get_resource::<AppTypeRegistry>()
        .cloned()
        .unwrap_or_default();
    let sprite_path = <Sprite as bevy::reflect::TypePath>::type_path();

    let mut state: SystemState<(Commands, Query<&Name>)> = SystemState::new(world);
    if let Ok((mut commands, names)) = state.get_mut(world) {
        let row = |commands: &mut Commands,
                   parent: Entity,
                   label: &str,
                   value: &dyn PartialReflect,
                   path: &str,
                   type_path: &str| {
            super::reflect_fields::spawn_field_row(
                commands,
                parent,
                label,
                value,
                0,
                path.to_string(),
                source,
                type_path,
                &names,
                &registry,
                &editor_font,
                &icon_font,
            );
        };
        let unlisted = (0..sprite.field_len())
            .filter_map(|index| sprite.name_at(index))
            .filter(|name| !ROW_ORDER.contains(&CardRow::Field(name)))
            .map(CardRow::Field);
        let listed = ROW_ORDER.iter().copied();
        for card_row in listed.chain(unlisted) {
            if card_row == CardRow::Anchor {
                if let Some(anchor) = anchor {
                    let custom = spawn_anchor_rows(&mut commands, body, source, anchor);
                    row(
                        &mut commands,
                        custom,
                        "offset",
                        &anchor.0,
                        ".0",
                        anchor_type_path(),
                    );
                }
            } else if card_row == CardRow::SizeFromPpu {
                spawn_ppu_row(&mut commands, body, source);
            } else if let CardRow::Field(name) = card_row
                && let Some(value) = sprite.field(name)
            {
                row(&mut commands, body, name, value, name, sprite_path);
            }
        }
    }
    state.apply(world);
    world.flush();
}

/// The preset combo, and the container the custom offset row goes in.
fn spawn_anchor_rows(
    commands: &mut Commands,
    body: Entity,
    source: Entity,
    anchor: Anchor,
) -> Entity {
    let index = anchor_index(anchor);
    let row = spawn_field_row(commands, body, FieldRowProps::new("anchor"));
    let options: Vec<ComboBoxOptionData> = ANCHOR_PRESETS
        .iter()
        .map(|(name, _)| ComboBoxOptionData::new(preset_label(name)))
        .chain([ComboBoxOptionData::new("Custom")])
        .collect();
    commands
        .spawn((combobox_with_selected(options, index), ChildOf(row.control)))
        .insert((SpriteAnchorCombo { source }, ComboBoxSelectedIndex(index)));
    commands
        .spawn((
            SpriteCustomAnchor { source },
            Node {
                display: custom_display(index),
                flex_direction: FlexDirection::Column,
                width: percent(100.0),
                ..default()
            },
            ChildOf(body),
        ))
        .id()
}

/// A preset sets the anchor through `sprite.set_anchor`; Custom only opens the
/// offset row.
fn on_anchor_pick(
    pick: On<ComboBoxChangeEvent>,
    combos: Query<&SpriteAnchorCombo>,
    proxies: Query<(), With<RemoteEntityProxy>>,
    mut custom: Query<(&SpriteCustomAnchor, &mut Node)>,
    mut commands: Commands,
) {
    let Ok(combo) = combos.get(pick.entity) else {
        return;
    };
    let source = combo.source;
    if proxies.contains(source) {
        return;
    }
    commands
        .entity(pick.entity)
        .insert(ComboBoxSelectedIndex(pick.selected));
    for (row, mut node) in &mut custom {
        if row.source == source {
            node.display = custom_display(pick.selected);
        }
    }
    let Some((name, _)) = ANCHOR_PRESETS.get(pick.selected) else {
        return;
    };
    commands
        .operator(SpriteSetAnchorOp::ID)
        .param("entity", source)
        .param("preset", *name)
        .call();
}

/// Remember that the offset row is about to write `source`'s anchor.
fn mark_own_offset_edit(
    change: On<ValueChange<f64>>,
    bindings: Query<&super::FieldBinding>,
    mut own: ResMut<OwnOffsetEdits>,
) {
    if let Ok(binding) = bindings.get(change.source)
        && binding.type_path == anchor_type_path()
    {
        own.0.insert(binding.source_entity);
    }
}

/// Show the anchor any write left on the sprite: a preset, an undo or redo, a
/// script or another card. Only the offset row's own edit keeps the combo on
/// Custom when it lands on a preset value, so the row being typed in stays
/// open.
fn follow_anchor_write(
    written: On<FieldWritten>,
    anchors: Query<&Anchor>,
    mut own: ResMut<OwnOffsetEdits>,
    mut combos: Query<(&SpriteAnchorCombo, &mut ComboBoxSelectedIndex)>,
    mut custom: Query<(&SpriteCustomAnchor, &mut Node)>,
) {
    if written.type_path != anchor_type_path() {
        return;
    }
    let from_offset_row = own.0.remove(&written.entity) && !written.undo;
    let Ok(anchor) = anchors.get(written.entity) else {
        return;
    };
    let index = anchor_index(*anchor);
    for (combo, mut shown) in &mut combos {
        if combo.source != written.entity
            || (shown.0 == CUSTOM_ANCHOR && from_offset_row)
            || shown.0 == index
        {
            continue;
        }
        shown.0 = index;
        for (row, mut node) in &mut custom {
            if row.source == written.entity {
                node.display = custom_display(index);
            }
        }
    }
}

fn spawn_ppu_row(commands: &mut Commands, body: Entity, source: Entity) {
    let row = spawn_field_row(commands, body, FieldRowProps::new("size from ppu"));
    let input = commands
        .spawn_scene(bsn! { @ScrubNumberInput })
        .insert((
            ScrubNumberInputValue::F64(DEFAULT_PPU),
            NumberInputPrecision(3),
            SpritePpuInput,
            ChildOf(row.control),
        ))
        .id();
    commands.spawn((
        button(ButtonProps::new("Apply")),
        SpritePpuApply { input, source },
        ChildOf(row.control),
    ));
}

fn on_ppu_number(
    change: On<ValueChange<f64>>,
    inputs: Query<(), With<SpritePpuInput>>,
    mut commands: Commands,
) {
    if inputs.contains(change.source) {
        commands
            .entity(change.source)
            .insert(ScrubNumberInputValue::F64(change.value));
    }
}

/// Size the sprite the card shows from the number beside the button.
fn on_ppu_apply(
    click: On<ButtonClickEvent>,
    applies: Query<&SpritePpuApply>,
    values: Query<&ScrubNumberInputValue>,
    proxies: Query<(), With<RemoteEntityProxy>>,
    mut commands: Commands,
) {
    let Ok(apply) = applies.get(click.entity) else {
        return;
    };
    if proxies.contains(apply.source) {
        return;
    }
    let ppu = match values.get(apply.input) {
        Ok(ScrubNumberInputValue::F64(ppu)) => *ppu,
        _ => DEFAULT_PPU,
    };
    commands
        .operator(SpriteSetPixelsPerUnitOp::ID)
        .param("entity", apply.source)
        .param("ppu", ppu)
        .call();
}
