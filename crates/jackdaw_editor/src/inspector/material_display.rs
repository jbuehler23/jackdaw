use bevy::prelude::*;
use jackdaw_feathers::{
    text_edit::{self, TextEditCommitEvent, TextEditProps},
    tokens,
};

/// Marker for material field UI entities
#[derive(Component)]
struct MaterialFieldMarker;

/// Spawn material fields in a deferred command to access `Assets<StandardMaterial>`.
pub(super) fn spawn_material_display_deferred(
    commands: &mut Commands,
    body_entity: Entity,
    source_entity: Entity,
) {
    commands.queue(move |world: &mut World| {
        spawn_material_fields(world, body_entity, source_entity);
    });
}

fn spawn_material_fields(world: &mut World, body_entity: Entity, source_entity: Entity) {
    // Look up the Handle<StandardMaterial> from MeshMaterial3d
    let handle = {
        let Some(mat) = world.get::<MeshMaterial3d<StandardMaterial>>(source_entity) else {
            return;
        };
        mat.0.clone()
    };

    let mat_data = {
        let materials = world.resource::<Assets<StandardMaterial>>();
        materials.get(&handle).map(|material| {
            (
                material.base_color,
                material.metallic,
                material.perceptual_roughness,
                material.reflectance,
                material.emissive,
                format!("{:?}", material.alpha_mode),
            )
        })
    };

    let Some((base_color, metallic, perceptual_roughness, reflectance, emissive, alpha_mode_str)) =
        mat_data
    else {
        world.spawn((
            Text::new("(material not loaded)"),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            ChildOf(body_entity),
        ));
        return;
    };

    // base_color (Color picker)
    {
        let srgba = base_color.to_srgba();
        let row = world
            .spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: Val::Px(tokens::SPACING_XS),
                    ..Default::default()
                },
                ChildOf(body_entity),
            ))
            .id();
        world.spawn((
            Text::new("base_color:"),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            Node {
                min_width: Val::Px(20.0),
                flex_shrink: 0.0,
                ..Default::default()
            },
            ChildOf(row),
        ));
        let rgba = [srgba.red, srgba.green, srgba.blue, srgba.alpha];
        let picker = world
            .spawn((
                jackdaw_feathers::color_picker::color_picker(
                    jackdaw_feathers::color_picker::ColorPickerProps::new().with_color(rgba),
                ),
                MaterialFieldMarker,
                ChildOf(row),
            ))
            .id();
        world.entity_mut(picker).observe(
            move |event: On<jackdaw_feathers::color_picker::ColorPickerCommitEvent>,
                  mut materials: ResMut<Assets<StandardMaterial>>,
                  mat_query: Query<&MeshMaterial3d<StandardMaterial>>| {
                let Ok(mat_comp) = mat_query.get(source_entity) else {
                    return;
                };
                if let Some(material) = materials.get_mut(&mat_comp.0) {
                    let c = event.color;
                    material.base_color = Color::srgba(c[0], c[1], c[2], c[3]);
                }
            },
        );
    }

    // metallic (f32 numeric input)
    spawn_material_numeric_field(
        world,
        body_entity,
        "metallic",
        metallic as f64,
        source_entity,
        |mat, val| {
            mat.metallic = val as f32;
        },
    );

    // perceptual_roughness
    spawn_material_numeric_field(
        world,
        body_entity,
        "roughness",
        perceptual_roughness as f64,
        source_entity,
        |mat, val| {
            mat.perceptual_roughness = val as f32;
        },
    );

    // reflectance
    spawn_material_numeric_field(
        world,
        body_entity,
        "reflectance",
        reflectance as f64,
        source_entity,
        |mat, val| {
            mat.reflectance = val as f32;
        },
    );

    // emissive (show as text for now - it's LinearRgba which is complex)
    {
        let emissive_text = format!(
            "emissive: ({:.2}, {:.2}, {:.2})",
            emissive.red, emissive.green, emissive.blue
        );
        world.spawn((
            Text::new(emissive_text),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            ChildOf(body_entity),
        ));
    }

    // alpha_mode (read-only for now)
    world.spawn((
        Text::new(format!("alpha_mode: {alpha_mode_str}")),
        TextFont {
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(body_entity),
    ));
}

/// Binding that links a material `text_edit` to a source entity and material field mutator.
#[derive(Component)]
pub(super) struct MaterialFieldBinding {
    pub(super) source_entity: Entity,
    pub(super) apply_fn: fn(&mut StandardMaterial, f64),
}

/// Handle `TextEditCommitEvent` for material field bindings.
pub(super) fn on_material_text_commit(
    event: On<TextEditCommitEvent>,
    bindings: Query<&MaterialFieldBinding>,
    child_of_query: Query<&ChildOf>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mat_query: Query<&MeshMaterial3d<StandardMaterial>>,
) {
    let mut current = event.entity;
    for _ in 0..4 {
        let Ok(child_of) = child_of_query.get(current) else {
            break;
        };
        if let Ok(binding) = bindings.get(child_of.parent()) {
            let value: f64 = event.text.parse().unwrap_or(0.0);
            let Ok(mat_comp) = mat_query.get(binding.source_entity) else {
                return;
            };
            if let Some(material) = materials.get_mut(&mat_comp.0) {
                (binding.apply_fn)(material, value);
            }
            return;
        }
        current = child_of.parent();
    }
}

fn spawn_material_numeric_field(
    world: &mut World,
    parent: Entity,
    label: &str,
    value: f64,
    source_entity: Entity,
    apply_fn: fn(&mut StandardMaterial, f64),
) {
    let row = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_XS),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();

    world.spawn((
        Text::new(format!("{label}:")),
        TextFont {
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Node {
            min_width: Val::Px(20.0),
            flex_shrink: 0.0,
            ..Default::default()
        },
        ChildOf(row),
    ));

    world.spawn((
        text_edit::text_edit(
            TextEditProps::default()
                .numeric_f32()
                .grow()
                .with_default_value(value.to_string()),
        ),
        MaterialFieldBinding {
            source_entity,
            apply_fn,
        },
        ChildOf(row),
    ));
}
