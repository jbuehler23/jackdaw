//! The `PixelCanvas` card: a resolution preset, then the canvas size, how it
//! scales to the window, the letterbox colour and pixel snapping, and any
//! other canvas field after them.

use bevy::{ecs::system::SystemState, prelude::*};
use jackdaw_feathers::icons::{EditorFont, IconFont};
use jackdaw_scene_types::{PIXEL_CANVAS_TYPE_PATH, PixelCanvas};

/// The canvas fields in card order, with the labels that differ from the
/// field name.
const ROW_ORDER: [(&str, Option<&str>); 4] = [
    ("resolution", Some("Size")),
    ("scale", None),
    ("letterbox", None),
    ("pixel_snap", None),
];

/// Fill the `PixelCanvas` card body for `source`. World-exclusive and deferred
/// by the caller.
pub(crate) fn fill_pixel_canvas_card_body(world: &mut World, source: Entity, body: Entity) {
    if world.get_entity(body).is_err() {
        return;
    }
    let Some(canvas) = world.get::<PixelCanvas>(source).copied() else {
        return;
    };
    let icon_font = (world.get_resource::<IconFont>()).map_or_else(default, |f| f.0.clone());
    let editor_font = (world.get_resource::<EditorFont>()).map_or_else(default, |f| f.0.clone());
    let registry = (world.get_resource::<AppTypeRegistry>())
        .cloned()
        .unwrap_or_default();

    let mut state: SystemState<(Commands, Query<&Name>)> = SystemState::new(world);
    if let Ok((mut commands, names)) = state.get_mut(world) {
        crate::pixel_camera::spawn_preset_row(&mut commands, body, source, canvas.resolution);
        let mut rows: Vec<(&str, Option<&str>)> = ROW_ORDER.to_vec();
        rows.extend(
            (0..canvas.field_len())
                .filter_map(|index| canvas.name_at(index))
                .filter(|name| !ROW_ORDER.iter().any(|(listed, _)| listed == name))
                .map(|name| (name, None)),
        );
        for (field, label) in rows {
            let Some(value) = canvas.field(field) else {
                continue;
            };
            let label = label.map_or_else(|| super::sentence(field), str::to_string);
            super::reflect_fields::spawn_card_field_row(
                &mut commands,
                body,
                &label,
                value,
                field,
                source,
                PIXEL_CANVAS_TYPE_PATH,
                &names,
                &registry,
                &editor_font,
                &icon_font,
            );
        }
    }
    state.apply(world);
    world.flush();
}
