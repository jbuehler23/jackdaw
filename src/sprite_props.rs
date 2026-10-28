//! A sprite's anchor, its draw order, and sizing a sprite from the pixels
//! per unit its art was drawn at, each set by an operator as one history
//! entry.

use bevy::{prelude::*, sprite::Anchor};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::warn_caller;
use jackdaw_scene_types::{Y_SORT_TYPE_PATH, YSort};

use crate::commands::{CommandGroup, CommandHistory, FieldEditSkip};

/// The anchor presets by the names `sprite.set_anchor` takes, in the order
/// the inspector lists them.
pub const ANCHOR_PRESETS: [(&str, Anchor); 9] = [
    ("top_left", Anchor::TOP_LEFT),
    ("top_center", Anchor::TOP_CENTER),
    ("top_right", Anchor::TOP_RIGHT),
    ("center_left", Anchor::CENTER_LEFT),
    ("center", Anchor::CENTER),
    ("center_right", Anchor::CENTER_RIGHT),
    ("bottom_left", Anchor::BOTTOM_LEFT),
    ("bottom_center", Anchor::BOTTOM_CENTER),
    ("bottom_right", Anchor::BOTTOM_RIGHT),
];

fn sprite_type_path() -> &'static str {
    <Sprite as bevy::reflect::TypePath>::type_path()
}

fn anchor_type_path() -> &'static str {
    <Anchor as bevy::reflect::TypePath>::type_path()
}

/// Size sprites so `ppu` of their image pixels make one world unit: each
/// one's `custom_size` becomes its image's pixel size (its rect or atlas cell
/// when it has one) over `ppu`, the project's pixels per unit when left out,
/// all as one history entry. Nothing else is
/// stored; the size is the sprite's own from then on.
#[operator(
    id = "sprite.set_pixels_per_unit",
    label = "Size From Pixels Per Unit",
    description = "Size sprites from the image pixels that make one world unit.",
    allows_undo = false,
    is_available = has_scene_sprite,
    params(
        entity(
            Entity,
            doc = "Sprite to size. Left out, every selected sprite."
        ),
        ppu(
            f64,
            doc = "Image pixels per world unit, above 0. Left out, the project's pixels per unit."
        ),
    )
)]
pub(crate) fn sprite_set_pixels_per_unit(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let id = SpriteSetPixelsPerUnitOp::ID;
    let Some(targets) = target_sprites(world, &params, id) else {
        return OperatorResult::Cancelled;
    };
    let ppu = match params.get("ppu") {
        None => Some(world.resource::<crate::world_units::WorldUnits>().ppu()),
        Some(_) => params.as_float("ppu").map(|ppu| ppu as f32),
    };
    let Some(ppu) = ppu.filter(|ppu| *ppu > 0.0 && ppu.is_finite()) else {
        warn_caller(world, format!("{id}: ppu is a number above 0"));
        return OperatorResult::Cancelled;
    };
    let mut sizes = Vec::new();
    for entity in targets {
        match measured_size(world, entity) {
            Ok(size) => sizes.push((entity, size / ppu)),
            Err(refusal) => {
                warn_caller(world, format!("{id}: {entity} {refusal}"));
                return OperatorResult::Cancelled;
            }
        }
    }
    let writes = sizes
        .into_iter()
        .map(|(entity, size)| (entity, serde_json::json!([size.x, size.y])))
        .collect();
    write_fields(
        world,
        id,
        FieldWrite {
            type_path: sprite_type_path(),
            field_path: "custom_size",
            label: "Size From Pixels Per Unit",
        },
        writes,
    )
}

/// Set the point sprites' positions name, all as one history entry.
#[operator(
    id = "sprite.set_anchor",
    label = "Set Anchor",
    description = "Set the point of a sprite its position names: a preset, or an offset from its centre.",
    allows_undo = false,
    is_available = has_scene_sprite,
    params(
        entity(Entity, doc = "Sprite to anchor. Left out, every selected sprite."),
        preset(
            String,
            doc = "top_left, top_center, top_right, center_left, center, center_right, bottom_left, bottom_center or bottom_right."
        ),
        x(
            f64,
            doc = "Offset right of the centre in sprite widths, -0.5 at the left edge. Taken with y when no preset is named."
        ),
        y(
            f64,
            doc = "Offset up from the centre in sprite heights, -0.5 at the bottom edge."
        ),
    )
)]
pub(crate) fn sprite_set_anchor(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let id = SpriteSetAnchorOp::ID;
    let Some(targets) = target_sprites(world, &params, id) else {
        return OperatorResult::Cancelled;
    };
    let after = if let Some(name) = params.as_str("preset") {
        let Some((_, preset)) = ANCHOR_PRESETS.iter().find(|(known, _)| *known == name) else {
            let names: Vec<&str> = ANCHOR_PRESETS.iter().map(|(name, _)| *name).collect();
            warn_caller(
                world,
                format!("{id}: no preset {name}; one of {}", names.join(", ")),
            );
            return OperatorResult::Cancelled;
        };
        *preset
    } else {
        let (Some(x), Some(y)) = (params.as_float("x"), params.as_float("y")) else {
            warn_caller(
                world,
                format!("{id}: name a preset=, or give both x= and y="),
            );
            return OperatorResult::Cancelled;
        };
        if !(x.is_finite() && y.is_finite()) {
            warn_caller(world, format!("{id}: x and y are finite numbers"));
            return OperatorResult::Cancelled;
        }
        Anchor(Vec2::new(x as f32, y as f32))
    };
    let offset = serde_json::json!([after.0.x, after.0.y]);
    let writes = targets
        .into_iter()
        .map(|entity| (entity, offset.clone()))
        .collect();
    write_fields(
        world,
        id,
        FieldWrite {
            type_path: anchor_type_path(),
            field_path: ".0",
            label: "Set Anchor",
        },
        writes,
    )
}

/// Draw the selected sprites one z step further in front, as one history entry.
#[operator(
    id = "sprite.z_up",
    label = "Bring Forward",
    description = "Step the selected sprites' z up by one, drawing them in front.",
    allows_undo = false,
    is_available = can_step_z
)]
pub(crate) fn sprite_z_up(_: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    crate::sprite_gizmo::step_sprites_z(world, 1.0);
    OperatorResult::Finished
}

/// Draw the selected sprites one z step further back, as one history entry.
#[operator(
    id = "sprite.z_down",
    label = "Send Backward",
    description = "Step the selected sprites' z down by one, drawing them behind.",
    allows_undo = false,
    is_available = can_step_z
)]
pub(crate) fn sprite_z_down(_: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    crate::sprite_gizmo::step_sprites_z(world, -1.0);
    OperatorResult::Finished
}

/// Whether the page keys step z: the primary selection is a sprite the
/// canvas may move, as for a nudge, and no other modifier is held.
fn can_step_z(
    gate: crate::sprite_ops::SpriteAddGate,
    keyboard: Res<ButtonInput<KeyCode>>,
    world: &World,
) -> bool {
    !crate::keybinds::unwanted_modifier(&keyboard, crate::keybinds::ChordModifiers::default())
        && world
            .get_resource::<crate::selection::Selection>()
            .and_then(crate::selection::Selection::primary)
            .is_some_and(|primary| crate::sprite_gizmo::movable_sprite(world, primary))
        && gate.allows()
}

/// Sort sprites by world y within their z layer, or stop, as one history
/// entry. The sort point is the transform origin, so a sprite that stands on
/// the ground anchors at its feet: `sprite.set_anchor preset=bottom_center`.
#[operator(
    id = "sprite.y_sort",
    label = "Y-Sort",
    description = "Draw sprites lower on screen in front of those above them in the same z layer.",
    allows_undo = false,
    is_available = has_scene_sprite,
    params(
        entity(Entity, doc = "Sprite to sort. Left out, every selected sprite."),
        on(bool, doc = "Sort by y when true (the default), stop when false."),
    )
)]
pub(crate) fn sprite_y_sort(params: In<OperatorParameters>, world: &mut World) -> OperatorResult {
    let id = SpriteYSortOp::ID;
    let Some(targets) = target_sprites(world, &params, id) else {
        return OperatorResult::Cancelled;
    };
    let on = params.as_bool("on").unwrap_or(true);
    let mut edits = Vec::new();
    for entity in targets {
        if world.get::<YSort>(entity).is_some() == on {
            continue;
        }
        let path = Y_SORT_TYPE_PATH.to_owned();
        let edit = if on {
            crate::inspector::ops::add_component_edit(world, entity, path)
        } else {
            crate::inspector::ops::remove_component_edit(world, entity, path)
        };
        edits.extend(edit);
    }
    if !edits.is_empty() {
        let label = if on { "Y-Sort On" } else { "Y-Sort Off" };
        world
            .resource_mut::<CommandHistory>()
            .push_executed(Box::new(CommandGroup {
                commands: edits,
                label: label.to_owned(),
            }));
    }
    OperatorResult::Finished
}

/// The field an operator writes on each of its targets, and the history label
/// the writes land under.
struct FieldWrite {
    type_path: &'static str,
    field_path: &'static str,
    label: &'static str,
}

/// Write each target's value through the shared field edit path, as one
/// history entry, by [`field_edits`].
fn write_fields(
    world: &mut World,
    id: &str,
    field: FieldWrite,
    writes: Vec<(Entity, serde_json::Value)>,
) -> OperatorResult {
    let FieldWrite {
        type_path,
        field_path,
        label,
    } = field;
    let writes = writes
        .into_iter()
        .map(|(entity, json)| (entity, field_path, json))
        .collect();
    let Some(edits) = field_edits(world, id, type_path, writes) else {
        return OperatorResult::Cancelled;
    };
    crate::commands::push_field_edits(world, edits, label);
    OperatorResult::Finished
}

/// The unexecuted edits writing each `(entity, field, value)` of `type_path`
/// through the shared field edit path, or `None` after telling the caller.
/// Every target is checked before anything is written: one the running
/// preview drives, or a value that does not convert, refuses the whole call.
/// A target projected from the running game in Live view takes its write
/// there instead, as the inspector's own rows do.
pub(crate) fn field_edits(
    world: &mut World,
    id: &str,
    type_path: &str,
    writes: Vec<(Entity, &str, serde_json::Value)>,
) -> Option<Vec<crate::commands::SetBsnField>> {
    let mut routed = Vec::new();
    let mut authored = Vec::new();
    for (entity, field_path, json) in writes {
        if crate::preview_context::preview_writes_type_path(world, entity, type_path) {
            warn_caller(
                world,
                format!("{id}: the running preview drives this on {entity}"),
            );
            return None;
        }
        if goes_to_the_game(world, entity) {
            routed.push((entity, field_path, json));
            continue;
        }
        let Some((path, value)) =
            crate::commands::field_edit_to_bsn_value(world, entity, type_path, field_path, &json)
        else {
            warn_caller(world, format!("{id}: {entity} cannot take {json}"));
            return None;
        };
        authored.push((entity, path, value));
    }
    for (entity, field_path, json) in routed {
        crate::inspector::reflect_fields::try_route_pie_live_field_edit(
            world, entity, type_path, field_path, json,
        );
    }
    let mut edits = Vec::new();
    for (entity, path, value) in authored {
        match crate::commands::set_bsn_field_command(world, entity, type_path, &path, value) {
            Ok(edit) => edits.push(edit),
            Err(FieldEditSkip::Unchanged) => {
                crate::commands::mirror_document_component(world, entity, type_path);
            }
            Err(FieldEditSkip::Previewed) => {}
        }
    }
    Some(edits)
}

/// Whether an edit of `entity` goes to the running game: Live view shows it
/// projected from a game entity.
pub(crate) fn goes_to_the_game(world: &World, entity: Entity) -> bool {
    world
        .get_resource::<crate::pie_mirror::PieViewMode>()
        .is_some_and(|mode| *mode == crate::pie_mirror::PieViewMode::Live)
        && world
            .get_resource::<crate::pie_projection::PieProjection>()
            .and_then(|projection| crate::live_edits::live_bits_for_preview(projection, entity))
            .is_some()
}

fn has_scene_sprite(sprites: Query<(), (With<Sprite>, Without<crate::EditorEntity>)>) -> bool {
    !sprites.is_empty()
}

/// The scene sprite `entity=` names, else the selected sprites, or `None`
/// after telling the caller. A read-only remote entity is refused.
fn target_sprites(world: &mut World, params: &OperatorParameters, id: &str) -> Option<Vec<Entity>> {
    let named: Vec<Entity> = match params.as_entity("entity") {
        Some(entity) => vec![entity],
        None => world
            .get_resource::<crate::selection::Selection>()
            .map(|selection| selection.entities.clone())
            .unwrap_or_default(),
    };
    let is_sprite = |world: &World, entity: Entity| {
        world
            .get_entity(entity)
            .is_ok_and(|held| held.contains::<Sprite>() && !held.contains::<crate::EditorEntity>())
    };
    let sprites: Vec<Entity> = named
        .into_iter()
        .filter(|&entity| is_sprite(world, entity))
        .collect();
    if sprites.is_empty() {
        warn_caller(
            world,
            format!("{id}: name a sprite with entity=, or select one"),
        );
        return None;
    }
    if let Some(proxy) = sprites.iter().copied().find(|&entity| {
        world
            .get::<crate::remote::entity_browser::RemoteEntityProxy>(entity)
            .is_some()
    }) {
        warn_caller(world, format!("{id}: {proxy} is a read-only remote entity"));
        return None;
    }
    Some(sprites)
}

/// The pixel size `entity` draws from its image, or why it has none.
fn measured_size(world: &World, entity: Entity) -> Result<Vec2, &'static str> {
    let sprite = world.get::<Sprite>(entity).ok_or("is not a sprite")?;
    if sprite.image.id() == AssetId::default() {
        return Err("has no image to measure");
    }
    source_size(world, sprite).ok_or("has an image that is still loading or missing")
}

/// The size a new sprite drawing `sprite`'s image takes at the project's pixels
/// per unit: the image's pixel size over the ppu, as `sprite.set_pixels_per_unit`
/// sizes one. `None` at a ppu of 1, where a sprite already draws one world unit
/// per pixel, or while the size is unknown.
pub(crate) fn project_size(world: &World, sprite: &Sprite) -> Option<Vec2> {
    let ppu = world.resource::<crate::world_units::WorldUnits>().ppu();
    if ppu == 1.0 {
        return None;
    }
    source_size(world, sprite).map(|size| size / ppu)
}

/// The pixel size of what `sprite` draws: its rect, the atlas cell, the
/// loaded image, or while an image of the default asset source loads, the
/// size its file declares.
fn source_size(world: &World, sprite: &Sprite) -> Option<Vec2> {
    crate::sprite_stage::sprite_source_size(
        sprite,
        world.resource::<Assets<Image>>(),
        world.resource::<Assets<TextureAtlasLayout>>(),
    )
    .or_else(|| {
        if sprite.rect.is_some() || sprite.texture_atlas.is_some() {
            return None;
        }
        let path = sprite.image.path()?;
        if !matches!(path.source(), bevy::asset::io::AssetSourceId::Default) {
            return None;
        }
        let file = crate::entity_ops::get_assets_base_dir()?.join(path.path());
        let (width, height) = image::image_dimensions(file).ok()?;
        Some(Vec2::new(width as f32, height as f32))
    })
}
