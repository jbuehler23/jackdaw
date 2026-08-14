use crate::selection::Selection;

use bevy::reflect::{NamedField, UnnamedField};
use bevy::{
    ecs::lifecycle::Insert,
    ecs::reflect::{AppTypeRegistry, ReflectComponent},
    feathers::{
        controls::{
            ColorChannel, ColorPlaneValue, ColorSwatchValue, FeathersCheckbox, FeathersColorPlane,
            FeathersColorSlider, FeathersColorSwatch, FeathersListRow, FeathersListView,
            FeathersListViewProps, FeathersMenu, FeathersMenuButton, FeathersMenuItem,
            FeathersMenuPopup, FeathersTextInput, FeathersTextInputContainer, SliderBaseColor,
        },
        theme::ThemedText,
    },
    input::keyboard::{KeyCode, KeyboardInput},
    input_focus::{FocusLost, FocusedInput, InputFocus},
    math::Vec3A,
    prelude::*,
    reflect::ReflectRef,
    text::{EditableText, TextEdit},
    ui::Checked,
    ui_widgets::{Activate, Checkbox, SliderValue, ValueChange, observe},
};
use jackdaw_feathers::{
    number_input::{
        NumberInputPrecision, NumberInputStep, ScrubNumberInput, ScrubNumberInputValue,
        is_scrubbing_or_focused,
    },
    tokens,
};

// Axis sigil colors for vector component inputs. The colored left stripe and
// the X/Y/Z label are drawn by the number input widget itself.
use bevy::feathers::tokens as feathers_tokens;

/// Decimal places shown for drag-scrub float fields. Also seeds
/// `NumberInputPrecision`, which the scrubber uses to round during a drag
/// and to derive drag speed.
const NUMERIC_DISPLAY_PRECISION: i32 = 2;

/// Round an `f64` to [`NUMERIC_DISPLAY_PRECISION`] for display, scrubbing the
/// `f32 -> f64` widening noise out of values read back via reflection so the
/// field shows e.g. `0.1` rather than `0.10000000149011612`.
fn round_numeric_display(value: f64) -> f64 {
    let factor = 10f64.powi(NUMERIC_DISPLAY_PRECISION);
    (value * factor).round() / factor
}

/// Read a [`ScrubNumberInputValue`] as an `f64` regardless of variant. The
/// widget's own `as_f64` is private, so the inspector reads the public variants.
fn scrub_value_as_f64(value: &ScrubNumberInputValue) -> f64 {
    match value {
        ScrubNumberInputValue::F32(v) => *v as f64,
        ScrubNumberInputValue::F64(v) => *v,
        ScrubNumberInputValue::I32(v) => *v as f64,
        ScrubNumberInputValue::I64(v) => *v as f64,
    }
}

use super::{FieldBinding, InspectorFieldRow, MAX_REFLECT_DEPTH};

pub(crate) fn spawn_reflected_fields(
    commands: &mut Commands,
    parent: Entity,
    reflected: &dyn Reflect,
    depth: usize,
    base_path: String,
    source_entity: Entity,
    type_path: &str,
    entity_names: &Query<&Name>,
    type_registry: &AppTypeRegistry,
    editor_font: &Handle<Font>,
    icon_font: &Handle<Font>,
) {
    match reflected.reflect_ref() {
        ReflectRef::Struct(s) => {
            for i in 0..s.field_len() {
                let Some(name) = s.name_at(i) else {
                    continue;
                };
                let Some(value) = s.field_at(i) else {
                    continue;
                };
                let child_path = if base_path.is_empty() {
                    name.to_string()
                } else {
                    format!("{base_path}.{name}")
                };
                spawn_field_row(
                    commands,
                    parent,
                    name,
                    value,
                    depth,
                    child_path,
                    source_entity,
                    type_path,
                    entity_names,
                    type_registry,
                    editor_font,
                    icon_font,
                );
            }
        }
        ReflectRef::TupleStruct(ts) => {
            for i in 0..ts.field_len() {
                let Some(value) = ts.field(i) else {
                    continue;
                };
                let child_path = if base_path.is_empty() {
                    format!(".{i}")
                } else {
                    format!("{base_path}.{i}")
                };
                spawn_field_row(
                    commands,
                    parent,
                    &format!("{i}"),
                    value,
                    depth,
                    child_path,
                    source_entity,
                    type_path,
                    entity_names,
                    type_registry,
                    editor_font,
                    icon_font,
                );
            }
        }
        ReflectRef::Enum(e) => {
            spawn_enum_field(
                commands,
                parent,
                e,
                depth,
                base_path,
                source_entity,
                type_path,
                entity_names,
                type_registry,
                editor_font,
                icon_font,
            );
        }
        ReflectRef::List(list) => {
            spawn_list_expansion(
                commands,
                parent,
                list.len(),
                |i| list.get(i),
                depth,
                &base_path,
                source_entity,
                type_path,
                entity_names,
            );
        }
        ReflectRef::Array(array) => {
            spawn_list_expansion(
                commands,
                parent,
                array.len(),
                |i| array.get(i),
                depth,
                &base_path,
                source_entity,
                type_path,
                entity_names,
            );
        }
        ReflectRef::Map(map) => {
            spawn_text_row(
                commands,
                parent,
                &format!("{{ {} entries }}", map.len()),
                depth,
            );
            if !map.is_empty() {
                let lv = commands
                    .spawn_scene(FeathersListView::scene(FeathersListViewProps::default()))
                    .insert(ChildOf(parent))
                    .id();
                for (key, val) in map.iter() {
                    let item_entity = commands
                        .spawn_scene(FeathersListRow::scene())
                        .insert(ChildOf(lv))
                        .id();
                    let key_label = format_partial_reflect_value(key);
                    let child_path = if base_path.is_empty() {
                        format!("[{key_label}]")
                    } else {
                        format!("{base_path}[{key_label}]")
                    };
                    spawn_field_row(
                        commands,
                        item_entity,
                        &key_label,
                        val,
                        depth + 1,
                        child_path,
                        source_entity,
                        type_path,
                        entity_names,
                        type_registry,
                        editor_font,
                        icon_font,
                    );
                }
            }
        }
        ReflectRef::Set(set) => {
            spawn_text_row(
                commands,
                parent,
                &format!("{{ {} items }}", set.len()),
                depth,
            );
            if !set.is_empty() {
                let lv = commands
                    .spawn_scene(FeathersListView::scene(FeathersListViewProps::default()))
                    .insert(ChildOf(parent))
                    .id();
                for item in set.iter() {
                    let item_entity = commands
                        .spawn_scene(FeathersListRow::scene())
                        .insert(ChildOf(lv))
                        .id();
                    spawn_text_row(
                        commands,
                        item_entity,
                        &format_partial_reflect_value(item),
                        depth + 1,
                    );
                }
            }
        }
        ReflectRef::Tuple(tuple) => {
            for i in 0..tuple.field_len() {
                let Some(value) = tuple.field(i) else {
                    continue;
                };
                let child_path = if base_path.is_empty() {
                    format!(".{i}")
                } else {
                    format!("{base_path}.{i}")
                };
                spawn_field_row(
                    commands,
                    parent,
                    &format!("{i}"),
                    value,
                    depth,
                    child_path,
                    source_entity,
                    type_path,
                    entity_names,
                    type_registry,
                    editor_font,
                    icon_font,
                );
            }
        }
        ReflectRef::Opaque(_) => {
            let label = reflected
                .get_represented_type_info()
                .map(|info| {
                    let path = info.type_path_table().short_path();
                    format!("<{path}>")
                })
                .unwrap_or_else(|| "(opaque)".to_string());
            spawn_text_row(commands, parent, &label, depth);
        }
    }
}

fn is_editable_primitive(value: &dyn PartialReflect) -> bool {
    value.try_downcast_ref::<f32>().is_some()
        || value.try_downcast_ref::<f64>().is_some()
        || value.try_downcast_ref::<i32>().is_some()
        || value.try_downcast_ref::<u32>().is_some()
        || value.try_downcast_ref::<usize>().is_some()
        || value.try_downcast_ref::<i8>().is_some()
        || value.try_downcast_ref::<i16>().is_some()
        || value.try_downcast_ref::<i64>().is_some()
        || value.try_downcast_ref::<u8>().is_some()
        || value.try_downcast_ref::<u16>().is_some()
        || value.try_downcast_ref::<u64>().is_some()
        || value.try_downcast_ref::<bool>().is_some()
        || value.try_downcast_ref::<String>().is_some()
}

pub(crate) fn spawn_field_row(
    commands: &mut Commands,
    parent: Entity,
    name: &str,
    value: &dyn PartialReflect,
    depth: usize,
    field_path: String,
    source_entity: Entity,
    type_path: &str,
    entity_names: &Query<&Name>,
    type_registry: &AppTypeRegistry,
    editor_font: &Handle<Font>,
    icon_font: &Handle<Font>,
) {
    // Entity reference -> clickable link (before any other check)
    if let Some(&entity_val) = value.try_downcast_ref::<Entity>() {
        let left_padding = depth as f32 * tokens::SPACING_MD;
        let row = commands
            .spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(tokens::SPACING_XS),
                    padding: UiRect::left(px(left_padding)),
                    ..Default::default()
                },
                ChildOf(parent),
            ))
            .id();
        commands.spawn((
            Text::new(format!("{name}:")),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            Node {
                min_width: px(20.0),
                flex_shrink: 0.0,
                ..Default::default()
            },
            TextColor(tokens::TYPE_ENTITY),
            ChildOf(row),
        ));
        let label = entity_names
            .get(entity_val)
            .map(|n| format!("{} ({entity_val})", n.as_str()))
            .unwrap_or_else(|_| format!("{entity_val}"));
        spawn_entity_link(commands, row, entity_val, &label);
        return;
    }

    // List/Array -> expand with ListView
    if let ReflectRef::List(list) = value.reflect_ref() {
        spawn_text_row(
            commands,
            parent,
            &format!("{name}: [{} items]", list.len()),
            depth,
        );
        if !list.is_empty() {
            let lv = commands
                .spawn_scene(FeathersListView::scene(FeathersListViewProps::default()))
                .insert(ChildOf(parent))
                .id();
            for i in 0..list.len() {
                if let Some(item) = list.get(i) {
                    let item_entity = commands
                        .spawn_scene(FeathersListRow::scene())
                        .insert(ChildOf(lv))
                        .id();
                    let child_path = if field_path.is_empty() {
                        format!("[{i}]")
                    } else {
                        format!("{field_path}[{i}]")
                    };
                    spawn_list_item_value(
                        commands,
                        item_entity,
                        item,
                        depth + 1,
                        child_path,
                        source_entity,
                        type_path,
                        entity_names,
                    );
                }
            }
        }
        return;
    }
    if let ReflectRef::Array(array) = value.reflect_ref() {
        spawn_text_row(
            commands,
            parent,
            &format!("{name}: [{} items]", array.len()),
            depth,
        );
        if !array.is_empty() {
            let lv = commands
                .spawn_scene(FeathersListView::scene(FeathersListViewProps::default()))
                .insert(ChildOf(parent))
                .id();
            for i in 0..array.len() {
                if let Some(item) = array.get(i) {
                    let item_entity = commands
                        .spawn_scene(FeathersListRow::scene())
                        .insert(ChildOf(lv))
                        .id();
                    let child_path = if field_path.is_empty() {
                        format!("[{i}]")
                    } else {
                        format!("{field_path}[{i}]")
                    };
                    spawn_list_item_value(
                        commands,
                        item_entity,
                        item,
                        depth + 1,
                        child_path,
                        source_entity,
                        type_path,
                        entity_names,
                    );
                }
            }
        }
        return;
    }
    if let ReflectRef::Map(map) = value.reflect_ref() {
        spawn_text_row(
            commands,
            parent,
            &format!("{name}: {{ {} entries }}", map.len()),
            depth,
        );
        if !map.is_empty() {
            let lv = commands
                .spawn_scene(FeathersListView::scene(FeathersListViewProps::default()))
                .insert(ChildOf(parent))
                .id();
            for (key, val) in map.iter() {
                let item_entity = commands
                    .spawn_scene(FeathersListRow::scene())
                    .insert(ChildOf(lv))
                    .id();
                let key_label = format_partial_reflect_value(key);
                let child_path = if field_path.is_empty() {
                    format!("[{key_label}]")
                } else {
                    format!("{field_path}[{key_label}]")
                };
                spawn_field_row(
                    commands,
                    item_entity,
                    &key_label,
                    val,
                    depth + 1,
                    child_path,
                    source_entity,
                    type_path,
                    entity_names,
                    type_registry,
                    editor_font,
                    icon_font,
                );
            }
        }
        return;
    }
    if let ReflectRef::Set(set) = value.reflect_ref() {
        spawn_text_row(
            commands,
            parent,
            &format!("{name}: {{ {} items }}", set.len()),
            depth,
        );
        if !set.is_empty() {
            let lv = commands
                .spawn_scene(FeathersListView::scene(FeathersListViewProps::default()))
                .insert(ChildOf(parent))
                .id();
            for item in set.iter() {
                let item_entity = commands
                    .spawn_scene(FeathersListRow::scene())
                    .insert(ChildOf(lv))
                    .id();
                spawn_text_row(
                    commands,
                    item_entity,
                    &format_partial_reflect_value(item),
                    depth + 1,
                );
            }
        }
        return;
    }

    // Vec3 compact row with colored XYZ labels
    if let Some(vec3) = value.try_downcast_ref::<Vec3>() {
        spawn_vec3_row(
            commands,
            parent,
            name,
            vec3,
            field_path,
            source_entity,
            type_path,
            depth,
            false,
        );
        return;
    }

    // Vec2 compact row
    if let Some(vec2) = value.try_downcast_ref::<Vec2>() {
        spawn_vec2_row(
            commands,
            parent,
            name,
            vec2,
            field_path,
            source_entity,
            type_path,
            depth,
            false,
        );
        return;
    }

    // Quat compact row (XYZW)
    if let Some(quat) = value.try_downcast_ref::<Quat>() {
        spawn_vec4_row(
            commands,
            parent,
            name,
            quat.x as f64,
            quat.y as f64,
            quat.z as f64,
            quat.w as f64,
            field_path,
            source_entity,
            type_path,
            depth,
            false,
        );
        return;
    }

    // Vec4 compact row (XYZW)
    if let Some(vec4) = value.try_downcast_ref::<Vec4>() {
        spawn_vec4_row(
            commands,
            parent,
            name,
            vec4.x as f64,
            vec4.y as f64,
            vec4.z as f64,
            vec4.w as f64,
            field_path,
            source_entity,
            type_path,
            depth,
            false,
        );
        return;
    }

    // Vec3A is a SIMD version of Vec3, used in affine transformation matrices (global transforms)
    if let Some(vec3) = value.try_downcast_ref::<Vec3A>() {
        spawn_vec3_row(
            commands,
            parent,
            name,
            &vec3.to_vec3(),
            field_path,
            source_entity,
            type_path,
            depth,
            // A Vec3A is not editable!
            true,
        );
        return;
    }

    // Color field with picker
    if let Some(color) = value.try_downcast_ref::<Color>() {
        spawn_color_field(
            commands,
            parent,
            name,
            *color,
            field_path,
            source_entity,
            type_path,
            depth,
        );
        return;
    }

    // Bool toggle
    if let Some(&bool_val) = value.try_downcast_ref::<bool>() {
        spawn_bool_toggle(
            commands,
            parent,
            name,
            bool_val,
            field_path,
            source_entity,
            type_path,
            depth,
            editor_font,
        );
        return;
    }

    // Numeric fields -> drag input
    if let Some(&v) = value.try_downcast_ref::<f32>() {
        spawn_numeric_field(
            commands,
            parent,
            name,
            v as f64,
            field_path,
            source_entity,
            type_path,
            depth,
            false,
        );
        return;
    }
    if let Some(&v) = value.try_downcast_ref::<f64>() {
        spawn_numeric_field(
            commands,
            parent,
            name,
            v,
            field_path,
            source_entity,
            type_path,
            depth,
            false,
        );
        return;
    }

    // Integer fields -> integer-only input. Renders without a
    // trailing `.0` and refuses decimal point on commit, so
    // bitmask types (e.g. `CollisionLayers::filters`, `LayerMask`)
    // round-trip without lossy `f32` conversion.
    if let Some(&v) = value.try_downcast_ref::<i32>() {
        spawn_numeric_field(
            commands,
            parent,
            name,
            v as f64,
            field_path,
            source_entity,
            type_path,
            depth,
            true,
        );
        return;
    }
    if let Some(&v) = value.try_downcast_ref::<u32>() {
        spawn_numeric_field(
            commands,
            parent,
            name,
            v as f64,
            field_path,
            source_entity,
            type_path,
            depth,
            true,
        );
        return;
    }
    if let Some(&v) = value.try_downcast_ref::<usize>() {
        spawn_numeric_field(
            commands,
            parent,
            name,
            v as f64,
            field_path,
            source_entity,
            type_path,
            depth,
            true,
        );
        return;
    }
    if let Some(&v) = value.try_downcast_ref::<i8>() {
        spawn_numeric_field(
            commands,
            parent,
            name,
            v as f64,
            field_path,
            source_entity,
            type_path,
            depth,
            true,
        );
        return;
    }
    if let Some(&v) = value.try_downcast_ref::<i16>() {
        spawn_numeric_field(
            commands,
            parent,
            name,
            v as f64,
            field_path,
            source_entity,
            type_path,
            depth,
            true,
        );
        return;
    }
    if let Some(&v) = value.try_downcast_ref::<i64>() {
        spawn_numeric_field(
            commands,
            parent,
            name,
            v as f64,
            field_path,
            source_entity,
            type_path,
            depth,
            true,
        );
        return;
    }
    if let Some(&v) = value.try_downcast_ref::<u8>() {
        spawn_numeric_field(
            commands,
            parent,
            name,
            v as f64,
            field_path,
            source_entity,
            type_path,
            depth,
            true,
        );
        return;
    }
    if let Some(&v) = value.try_downcast_ref::<u16>() {
        spawn_numeric_field(
            commands,
            parent,
            name,
            v as f64,
            field_path,
            source_entity,
            type_path,
            depth,
            true,
        );
        return;
    }
    if let Some(&v) = value.try_downcast_ref::<u64>() {
        spawn_numeric_field(
            commands,
            parent,
            name,
            v as f64,
            field_path,
            source_entity,
            type_path,
            depth,
            true,
        );
        return;
    }

    // Enum fields -> select menu
    if let ReflectRef::Enum(e) = value.reflect_ref() {
        if value.try_as_reflect().is_some() {
            spawn_enum_field(
                commands,
                parent,
                e,
                depth,
                field_path,
                source_entity,
                type_path,
                entity_names,
                type_registry,
                editor_font,
                icon_font,
            );
            return;
        }
        // Fallback: just show variant name
        let text = format!("{name}: {}", e.variant_name());
        spawn_text_row(commands, parent, &text, depth);
        return;
    }

    // `ZoneId` is a transparent string newtype. Render its inner string as a plain
    // text field rather than recursing into the tuple struct (which would show a
    // nested `0:` field). Edits commit through the standard string path.
    #[cfg(feature = "multiplayer")]
    if let Some(zone) = value.try_downcast_ref::<jackdaw_multiplayer::ZoneId>() {
        spawn_editable_field(
            commands,
            parent,
            name,
            &zone.0,
            field_path,
            source_entity,
            type_path,
            depth,
        );
        return;
    }

    let is_compound = matches!(
        value.reflect_ref(),
        ReflectRef::Struct(_) | ReflectRef::TupleStruct(_) | ReflectRef::Tuple(_)
    );

    // Check for opaque types that shouldn't be recursed into
    if is_compound && is_opaque_type(value) {
        let text = format!("{name}: {}", format_partial_reflect_value(value));
        spawn_text_row(commands, parent, &text, depth);
        return;
    }

    if depth >= MAX_REFLECT_DEPTH || !is_compound {
        if is_editable_primitive(value) {
            spawn_editable_field(
                commands,
                parent,
                name,
                &format_partial_reflect_value(value),
                field_path,
                source_entity,
                type_path,
                depth,
            );
        } else {
            let text = format!("{name}: {}", format_partial_reflect_value(value));
            spawn_text_row(commands, parent, &text, depth);
        }
    } else {
        // Sub-header + recurse
        spawn_text_row(commands, parent, name, depth);

        let container = commands
            .spawn((Node {
                flex_direction: FlexDirection::Column,
                padding: UiRect::left(px(tokens::SPACING_LG)),
                ..Default::default()
            },))
            .insert(ChildOf(parent))
            .id();

        match value.reflect_ref() {
            ReflectRef::Struct(s) => {
                for i in 0..s.field_len() {
                    let Some(field_name) = s.name_at(i) else {
                        continue;
                    };
                    let Some(field_value) = s.field_at(i) else {
                        continue;
                    };
                    let child_path = format!("{field_path}.{field_name}");
                    spawn_field_row(
                        commands,
                        container,
                        field_name,
                        field_value,
                        depth + 1,
                        child_path,
                        source_entity,
                        type_path,
                        entity_names,
                        type_registry,
                        editor_font,
                        icon_font,
                    );
                }
            }
            ReflectRef::TupleStruct(ts) => {
                for i in 0..ts.field_len() {
                    let Some(field_value) = ts.field(i) else {
                        continue;
                    };
                    let child_path = format!("{field_path}.{i}");
                    spawn_field_row(
                        commands,
                        container,
                        &format!("{i}"),
                        field_value,
                        depth + 1,
                        child_path,
                        source_entity,
                        type_path,
                        entity_names,
                        type_registry,
                        editor_font,
                        icon_font,
                    );
                }
            }
            ReflectRef::Tuple(tuple) => {
                for i in 0..tuple.field_len() {
                    let Some(field_value) = tuple.field(i) else {
                        continue;
                    };
                    let child_path = format!("{field_path}.{i}");
                    spawn_field_row(
                        commands,
                        container,
                        &format!("{i}"),
                        field_value,
                        depth + 1,
                        child_path,
                        source_entity,
                        type_path,
                        entity_names,
                        type_registry,
                        editor_font,
                        icon_font,
                    );
                }
            }
            _ => {}
        }
    }
}

fn spawn_vec3_row(
    commands: &mut Commands,
    parent: Entity,
    name: &str,
    vec3: &Vec3,
    field_path: String,
    source_entity: Entity,
    type_path: &str,
    depth: usize,
    disabled: bool,
) {
    let left_padding = depth as f32 * tokens::SPACING_MD;
    // Column container: label above, axis inputs below.
    // Carries an `InspectorFieldRow` marker so row-level decorators
    // (e.g. the animation keyframe diamond) can hang off the root
    // field without being duplicated per axis.
    let col = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(tokens::SPACING_XS),
                padding: UiRect::left(px(left_padding)),
                width: Val::Percent(100.0),
                position_type: PositionType::Relative,
                ..Default::default()
            },
            InspectorFieldRow {
                source_entity,
                type_path: type_path.to_string(),
                field_path: field_path.clone(),
            },
            ChildOf(parent),
        ))
        .id();

    // Label row (small text above)
    commands.spawn((
        Text::new(name),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_TERTIARY),
        ChildOf(col),
    ));

    // Axis inputs row (X Y Z side by side)
    let axes_row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                column_gap: px(tokens::SPACING_MD),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(col),
        ))
        .id();

    spawn_axis_input(
        commands,
        axes_row,
        "X",
        vec3.x as f64,
        feathers_tokens::TEXT_INPUT_X_AXIS,
        format!("{field_path}.x"),
        source_entity,
        type_path,
        disabled,
    );
    spawn_axis_input(
        commands,
        axes_row,
        "Y",
        vec3.y as f64,
        feathers_tokens::TEXT_INPUT_Y_AXIS,
        format!("{field_path}.y"),
        source_entity,
        type_path,
        disabled,
    );
    spawn_axis_input(
        commands,
        axes_row,
        "Z",
        vec3.z as f64,
        feathers_tokens::TEXT_INPUT_Z_AXIS,
        format!("{field_path}.z"),
        source_entity,
        type_path,
        disabled,
    );
}

fn spawn_vec2_row(
    commands: &mut Commands,
    parent: Entity,
    name: &str,
    vec2: &Vec2,
    field_path: String,
    source_entity: Entity,
    type_path: &str,
    depth: usize,
    disabled: bool,
) {
    let left_padding = depth as f32 * tokens::SPACING_MD;
    let col = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(tokens::SPACING_XS),
                padding: UiRect::left(px(left_padding)),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();

    // Label
    commands.spawn((
        Text::new(name),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_TERTIARY),
        ChildOf(col),
    ));

    // Axis inputs row
    let axes_row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                column_gap: px(tokens::SPACING_MD),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(col),
        ))
        .id();

    spawn_axis_input(
        commands,
        axes_row,
        "X",
        vec2.x as f64,
        feathers_tokens::TEXT_INPUT_X_AXIS,
        format!("{field_path}.x"),
        source_entity,
        type_path,
        disabled,
    );
    spawn_axis_input(
        commands,
        axes_row,
        "Y",
        vec2.y as f64,
        feathers_tokens::TEXT_INPUT_Y_AXIS,
        format!("{field_path}.y"),
        source_entity,
        type_path,
        disabled,
    );
}

fn spawn_vec4_row(
    commands: &mut Commands,
    parent: Entity,
    name: &str,
    x: f64,
    y: f64,
    z: f64,
    w: f64,
    field_path: String,
    source_entity: Entity,
    type_path: &str,
    depth: usize,
    disabled: bool,
) {
    let left_padding = depth as f32 * tokens::SPACING_MD;
    let col = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(tokens::SPACING_XS),
                padding: UiRect::left(px(left_padding)),
                width: Val::Percent(100.0),
                position_type: PositionType::Relative,
                ..Default::default()
            },
            InspectorFieldRow {
                source_entity,
                type_path: type_path.to_string(),
                field_path: field_path.clone(),
            },
            ChildOf(parent),
        ))
        .id();

    // Label
    commands.spawn((
        Text::new(name),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_TERTIARY),
        ChildOf(col),
    ));

    // XYZW axis inputs row
    let axes_row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                column_gap: px(tokens::SPACING_SM),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(col),
        ))
        .id();

    spawn_axis_input(
        commands,
        axes_row,
        "X",
        x,
        feathers_tokens::TEXT_INPUT_X_AXIS,
        format!("{field_path}.x"),
        source_entity,
        type_path,
        disabled,
    );
    spawn_axis_input(
        commands,
        axes_row,
        "Y",
        y,
        feathers_tokens::TEXT_INPUT_Y_AXIS,
        format!("{field_path}.y"),
        source_entity,
        type_path,
        disabled,
    );
    spawn_axis_input(
        commands,
        axes_row,
        "Z",
        z,
        feathers_tokens::TEXT_INPUT_Z_AXIS,
        format!("{field_path}.z"),
        source_entity,
        type_path,
        disabled,
    );
    // No dedicated axis token for a 4th component; use the default sigil.
    spawn_axis_input(
        commands,
        axes_row,
        "W",
        w,
        feathers_tokens::TEXT_INPUT_BG,
        format!("{field_path}.w"),
        source_entity,
        type_path,
        disabled,
    );
}

fn spawn_axis_input(
    commands: &mut Commands,
    parent: Entity,
    label: &'static str,
    value: f64,
    sigil_color: bevy::feathers::theme::ThemeToken,
    field_path: String,
    source_entity: Entity,
    type_path: &str,
    disabled: bool,
) {
    // One float input per vector component. The widget draws the colored sigil
    // stripe and the X/Y/Z/W label; `ScrubNumberInputValue::F64` holds the
    // component value. The `FieldBinding` carries the `.x`/`.y`/`.z`/`.w`
    // subfield path, so `on_numeric_value_change_f64` writes back the component.
    let mut field = commands.spawn_scene(bsn! {
        @ScrubNumberInput { @sigil_color: {sigil_color}, @label_text: {label} }
    });
    field.insert((
        FieldBinding {
            source_entity,
            type_path: type_path.to_string(),
            field_path,
        },
        ChildOf(parent),
        ScrubNumberInputValue::F64(round_numeric_display(value)),
        NumberInputPrecision(NUMERIC_DISPLAY_PRECISION),
    ));
    if disabled {
        field.insert(bevy::ui::InteractionDisabled);
    }
}

fn spawn_bool_toggle(
    commands: &mut Commands,
    parent: Entity,
    name: &str,
    value: bool,
    field_path: String,
    source_entity: Entity,
    type_path: &str,
    depth: usize,
    editor_font: &Handle<Font>,
) {
    let left_padding = depth as f32 * tokens::SPACING_MD;
    // Bool fields stay as a row (label + checkbox side by side)
    let row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(tokens::SPACING_SM),
                padding: UiRect::left(px(left_padding)),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();

    commands.spawn((
        Text::new(format!("{name}:")),
        TextFont {
            font: editor_font.clone().into(),
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TYPE_BOOL),
        ChildOf(row),
    ));

    // The `FieldBinding` sits on the checkbox entity, so
    // `on_checkbox_commit` reads it from the `ValueChange` source directly.
    let mut cb = commands.spawn_scene(bsn! { @FeathersCheckbox });
    cb.insert((
        FieldBinding {
            source_entity,
            type_path: type_path.to_string(),
            field_path,
        },
        ChildOf(row),
    ));
    // The checkbox does not self-manage `Checked`; seed the initial state.
    // `on_checkbox_commit` and the refresh path keep it in sync thereafter.
    if value {
        cb.insert(Checked);
    }
}

/// Which channel a color sub-widget edits. `Plane` covers the red/blue plane
/// picker; the rest are the per-channel sliders.
#[derive(Clone, Copy, PartialEq)]
enum ColorPart {
    Plane,
    Red,
    Green,
    Blue,
    Alpha,
}

/// Links a color sub-widget back to its field root so the global
/// `ValueChange` observers can find the shared `ColorFieldState`.
#[derive(Component)]
pub(crate) struct ColorSubWidget {
    root: Entity,
    part: ColorPart,
}

/// The current sRGBA color for one inspector color field, held on the field
/// root. The native color widgets do not self-update their values, so this is
/// the app-owned source of truth that the observers read, mutate, and mirror
/// back onto every sub-widget.
#[derive(Component)]
struct ColorFieldState {
    rgba: [f32; 4],
}

/// Where a color field writes its committed sRGBA. Held on the field root and
/// invoked by `commit_color_field` with `(world, rgba, is_final)`. This is the
/// only binding-specific part of the color composite; different callers supply
/// their own commit closure while sharing every widget and observer.
#[derive(Component)]
struct ColorCommit(Box<dyn Fn(&mut World, [f32; 4], bool) + Send + Sync>);

/// Optional gate on a color field root. When present and it returns `true`, the
/// shared plane/slider observers drop the edit before touching `ColorFieldState`
/// or the sub-widgets, so a read-only field never moves. Absent means always
/// editable. Callers that need the read-only case (reflect fields skip remote
/// proxy targets) attach this; the observers stay binding-agnostic.
#[derive(Component)]
struct ColorCommitSkip(Box<dyn Fn(&World) -> bool + Send + Sync>);

/// True when a color field root carries a `ColorCommitSkip` that vetoes the
/// edit. Runs against the read-only world before an observer mutates state.
fn color_edit_skipped(world: &World, root: Entity) -> bool {
    world
        .get::<ColorCommitSkip>(root)
        .is_some_and(|skip| skip.0(world))
}

/// Marks the inner text entry of a color field's hex input and links it to the
/// field root. The hex commit observers self-filter on this marker so the
/// generic string-field commit path never runs for the hex box.
#[derive(Component)]
pub(crate) struct HexColorInput {
    root: Entity,
}

/// Spawn a color-picker composite (label + swatch preview + hex entry + red/blue
/// plane + RGBA sliders) under `parent`, seeded to `initial` sRGBA. The composite
/// is binding-agnostic: every widget, the shared `ColorFieldState`, and the sync
/// observers are the same regardless of where the value goes. `commit` receives
/// `(world, rgba, is_final)` each time the color settles, so callers route the
/// result wherever they need without touching the shared observer path. Returns
/// the field root, which carries `ColorFieldState` and `ColorCommit`.
pub(crate) fn spawn_color_picker(
    commands: &mut Commands,
    parent: Entity,
    initial: [f32; 4],
    name: &str,
    left_padding: f32,
    commit: impl Fn(&mut World, [f32; 4], bool) + Send + Sync + 'static,
) -> Entity {
    let rgba = initial;
    let srgba = Srgba::new(rgba[0], rgba[1], rgba[2], rgba[3]);

    // Field root: holds the shared color state and the commit callback that the
    // sub-widget observers reach through `ColorSubWidget.root`.
    let root = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(tokens::SPACING_XS),
                padding: UiRect::left(px(left_padding)),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ColorFieldState { rgba },
            ColorCommit(Box::new(commit)),
            ChildOf(parent),
        ))
        .id();

    // Label + swatch preview on one row.
    let header = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::SpaceBetween,
                column_gap: px(tokens::SPACING_SM),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(root),
        ))
        .id();

    commands.spawn((
        Text::new(format!("{name}:")),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_TERTIARY),
        ChildOf(header),
    ));

    // Hex entry mirrors the current color as a 6- or 8-digit hex string and
    // parses one back on commit. `HexColorInput` lands on the inner text entry
    // via a deferred insert once the scene tree spawns.
    let hex_container = commands
        .spawn_scene(hex_input_scene())
        .insert((PendingFieldText(srgba.to_hex()), ChildOf(header)))
        .id();
    commands.queue(move |world: &mut World| {
        let mut descendants: Vec<Entity> = Vec::new();
        if let Ok(children) = world.query::<&Children>().get(world, hex_container) {
            descendants.extend(children.iter());
        }
        while let Some(entity) = descendants.pop() {
            if world.get::<EditableText>(entity).is_some() {
                world.entity_mut(entity).insert(HexColorInput { root });
                break;
            }
            if let Ok(children) = world.query::<&Children>().get(world, entity) {
                descendants.extend(children.iter());
            }
        }
    });

    commands.spawn_scene(bsn! { @FeathersColorSwatch }).insert((
        ColorSwatchValue(Color::Srgba(srgba)),
        ColorSubWidget {
            root,
            part: ColorPart::Plane,
        },
        ChildOf(header),
    ));

    // Red/blue plane picker. The plane's fixed channel is green (`z`); its `x`
    // is red, `y` is blue.
    commands
        .spawn_scene(bsn! { @FeathersColorPlane::RedBlue })
        .insert((
            ColorPlaneValue(Vec3::new(rgba[0], rgba[2], rgba[1])),
            ColorSubWidget {
                root,
                part: ColorPart::Plane,
            },
            ChildOf(root),
        ));

    // One slider per channel. `SliderBaseColor` drives the gradient; the
    // observers keep it and `SliderValue` in sync with `ColorFieldState`.
    for (part, channel, value) in [
        (ColorPart::Red, ColorChannel::Red, rgba[0]),
        (ColorPart::Green, ColorChannel::Green, rgba[1]),
        (ColorPart::Blue, ColorChannel::Blue, rgba[2]),
        (ColorPart::Alpha, ColorChannel::Alpha, rgba[3]),
    ] {
        commands
            .spawn_scene(bsn! {
                @FeathersColorSlider {
                    @value: {value},
                    @channel: {channel},
                }
            })
            .insert((
                SliderBaseColor(Color::Srgba(srgba)),
                ColorSubWidget { root, part },
                ChildOf(root),
            ));
    }

    root
}

fn spawn_color_field(
    commands: &mut Commands,
    parent: Entity,
    name: &str,
    color: Color,
    field_path: String,
    source_entity: Entity,
    type_path: &str,
    depth: usize,
) {
    let left_padding = depth as f32 * tokens::SPACING_MD;

    let srgba = color.to_srgba();
    let rgba = [srgba.red, srgba.green, srgba.blue, srgba.alpha];

    let tp = type_path.to_string();
    let path = field_path;

    let commit = move |world: &mut World, rgba: [f32; 4], is_final: bool| {
        if is_final {
            apply_color_with_undo(world, source_entity, &tp, &path, rgba);
        } else {
            let value_str = serde_json::to_string(&rgba).unwrap_or_default();
            apply_field_value_live(world, source_entity, &tp, &path, &value_str);
        }
    };

    let root = spawn_color_picker(commands, parent, rgba, name, left_padding, commit);

    // The plane/slider drag observers skip read-only remote proxy targets. Held
    // as a `ColorCommitSkip` on the root so the shared observers never touch
    // `FieldBinding`; the hex commit path is unaffected, matching the prior
    // behavior where only the drag observers guarded on the proxy.
    commands
        .entity(root)
        .insert(ColorCommitSkip(Box::new(move |world: &World| {
            world
                .get::<crate::remote::entity_browser::RemoteEntityProxy>(source_entity)
                .is_some()
        })));
}

/// Container framing the color field's hex `FeathersTextInput`. The inner text
/// entry carries `HexColorInput` and its own commit observers, so the generic
/// string-field path never handles it. `PendingFieldText` on the container
/// seeds the initial hex string once the inner entry spawns.
fn hex_input_scene() -> impl Scene {
    bsn! {
        @FeathersTextInputContainer
        Node { flex_grow: 0.0 }
        on(seed_string_field)
        Children [
            (
                @FeathersTextInput {
                    @visible_width: 10f32,
                    @max_characters: 9usize,
                }
                on(hex_input_on_enter_key)
                on(hex_input_on_focus_lost)
            )
        ]
    }
}

/// Parse the hex string in a color field's hex entry, and on success adopt it
/// into `ColorFieldState`, repaint the composite, and commit one undo-backed
/// change. An unparseable string is ignored; the next `sync_color_widgets`
/// restores the valid text.
fn commit_hex_input(world: &mut World, hex_entity: Entity, text: String) {
    let Some(hex) = world.get::<HexColorInput>(hex_entity) else {
        return;
    };
    let root = hex.root;

    let Ok(parsed) = Srgba::hex(text) else {
        return;
    };
    let rgba = [parsed.red, parsed.green, parsed.blue, parsed.alpha];

    if let Some(mut state) = world.get_mut::<ColorFieldState>(root) {
        state.rgba = rgba;
    }
    sync_color_widgets(world, root);
    commit_color_field(world, root, true);
}

/// Commit the hex entry when Enter is pressed.
fn hex_input_on_enter_key(
    key_input: On<FocusedInput<KeyboardInput>>,
    q_text: Query<&EditableText>,
    q_hex: Query<(), With<HexColorInput>>,
    mut commands: Commands,
) {
    if key_input.input.key_code != KeyCode::Enter {
        return;
    }
    let hex_entity = key_input.event_target();
    if !q_hex.contains(hex_entity) {
        return;
    }
    if let Ok(editable) = q_text.get(hex_entity) {
        let text = editable.value().to_string();
        commands.queue(move |world: &mut World| {
            commit_hex_input(world, hex_entity, text);
        });
    }
}

/// Commit the hex entry when it loses focus.
fn hex_input_on_focus_lost(
    focus_lost: On<FocusLost>,
    q_text: Query<&EditableText>,
    q_hex: Query<(), With<HexColorInput>>,
    mut commands: Commands,
) {
    let hex_entity = focus_lost.event_target();
    if !q_hex.contains(hex_entity) {
        return;
    }
    if let Ok(editable) = q_text.get(hex_entity) {
        let text = editable.value().to_string();
        commands.queue(move |world: &mut World| {
            commit_hex_input(world, hex_entity, text);
        });
    }
}

/// Repaint every color sub-widget under `root` from the field's
/// `ColorFieldState`. Called after an observer mutates a channel so the swatch,
/// plane, and the other sliders stay consistent within one gesture.
fn sync_color_widgets(world: &mut World, root: Entity) {
    let Some(state) = world.get::<ColorFieldState>(root) else {
        return;
    };
    let rgba = state.rgba;
    let srgba = Srgba::new(rgba[0], rgba[1], rgba[2], rgba[3]);
    let color = Color::Srgba(srgba);

    let mut widgets: Vec<(Entity, ColorPart)> = Vec::new();
    let mut query = world.query::<(Entity, &ColorSubWidget)>();
    for (entity, sub) in query.iter(world) {
        if sub.root == root {
            widgets.push((entity, sub.part));
        }
    }

    for (entity, part) in widgets {
        let Ok(mut ent) = world.get_entity_mut(entity) else {
            continue;
        };
        match part {
            ColorPart::Plane => {
                // The swatch and the plane both carry `ColorPart::Plane`; only
                // the plane has a `ColorPlaneValue`, only the swatch a
                // `ColorSwatchValue`, so the branch that matches updates.
                if ent.contains::<ColorPlaneValue>() {
                    ent.insert(ColorPlaneValue(Vec3::new(rgba[0], rgba[2], rgba[1])));
                }
                if ent.contains::<ColorSwatchValue>() {
                    ent.insert(ColorSwatchValue(color));
                }
            }
            ColorPart::Red => {
                ent.insert((SliderValue(rgba[0]), SliderBaseColor(color)));
            }
            ColorPart::Green => {
                ent.insert((SliderValue(rgba[1]), SliderBaseColor(color)));
            }
            ColorPart::Blue => {
                ent.insert((SliderValue(rgba[2]), SliderBaseColor(color)));
            }
            ColorPart::Alpha => {
                ent.insert((SliderValue(rgba[3]), SliderBaseColor(color)));
            }
        }
    }

    // Mirror the color into the hex entry, unless the user is typing in it.
    let focused = world.resource::<InputFocus>().get();
    let mut hex_entity = None;
    let mut hex_query = world.query::<(Entity, &HexColorInput)>();
    for (entity, hex) in hex_query.iter(world) {
        if hex.root == root {
            hex_entity = Some(entity);
            break;
        }
    }
    if let Some(hex_entity) = hex_entity
        && focused != Some(hex_entity)
        && let Ok(mut editable) = world
            .query::<&mut EditableText>()
            .get_mut(world, hex_entity)
    {
        editable.queue_edit(TextEdit::SelectAll);
        editable.queue_edit(TextEdit::Insert(srgba.to_hex().into()));
    }
}

/// Commit the current `ColorFieldState` under `root` through the field's
/// `ColorCommit` callback. A live drag passes `is_final == false`; the final
/// event passes `true`. The callback decides where the color goes, so this is
/// binding-agnostic.
fn commit_color_field(world: &mut World, root: Entity, is_final: bool) {
    let Some(state) = world.get::<ColorFieldState>(root) else {
        return;
    };
    let rgba = state.rgba;

    let Some(commit) = world.entity_mut(root).take::<ColorCommit>() else {
        return;
    };
    commit.0(world, rgba, is_final);
    world.entity_mut(root).insert(commit);
}

/// Write path for the red/blue color plane. Updates the red and blue channels
/// of the field's `ColorFieldState`, mirrors the color across the composite,
/// then commits (live while dragging, undo-backed on release).
pub(crate) fn on_color_plane_change(
    event: On<ValueChange<Vec2>>,
    sub_widgets: Query<&ColorSubWidget>,
    mut commands: Commands,
) {
    let Ok(sub) = sub_widgets.get(event.source) else {
        return;
    };
    let root = sub.root;
    let value = event.value;
    let is_final = event.is_final;
    commands.queue(move |world: &mut World| {
        if color_edit_skipped(world, root) {
            return;
        }
        if let Some(mut state) = world.get_mut::<ColorFieldState>(root) {
            state.rgba[0] = value.x;
            state.rgba[2] = value.y;
        }
        sync_color_widgets(world, root);
        commit_color_field(world, root, is_final);
    });
}

/// Write path for the per-channel color sliders. Reads which channel the
/// slider edits off `ColorSubWidget.part`, updates it, mirrors the composite,
/// and commits.
pub(crate) fn on_color_slider_change(
    event: On<ValueChange<f32>>,
    sub_widgets: Query<&ColorSubWidget>,
    mut commands: Commands,
) {
    let Ok(sub) = sub_widgets.get(event.source) else {
        return;
    };
    let root = sub.root;
    let part = sub.part;
    let channel = match part {
        ColorPart::Red => 0,
        ColorPart::Green => 1,
        ColorPart::Blue => 2,
        ColorPart::Alpha => 3,
        // The plane carries `ColorPart::Plane` and never emits `ValueChange<f32>`.
        ColorPart::Plane => return,
    };
    let value = event.value;
    let is_final = event.is_final;
    commands.queue(move |world: &mut World| {
        if color_edit_skipped(world, root) {
            return;
        }
        if let Some(mut state) = world.get_mut::<ColorFieldState>(root) {
            state.rgba[channel] = value;
        }
        sync_color_widgets(world, root);
        commit_color_field(world, root, is_final);
    });
}

/// Apply a color change with undo support (propagates to all selected entities).
fn apply_color_with_undo(
    world: &mut World,
    _entity: Entity,
    type_path: &str,
    field_path: &str,
    new_rgba: [f32; 4],
) {
    let registry = world.resource::<AppTypeRegistry>().clone();

    // Live edits need the color in canonical reflect form (a `Color`
    // enum like `{"LinearRgba": {..}}`), not the picker's raw sRGBA
    // array, so the game can deserialize the full component.
    if *world.resource::<crate::pie_mirror::PieViewMode>() == crate::pie_mirror::PieViewMode::Live {
        let srgba = Srgba::new(new_rgba[0], new_rgba[1], new_rgba[2], new_rgba[3]);
        let canonical = {
            let reg = registry.read();
            color_to_canonical_json(Color::Srgba(srgba), &reg)
        };
        try_route_pie_live_field_edit(world, _entity, type_path, field_path, canonical);
        return;
    }

    // The canonical reflect JSON (`{"Srgba": {..}}`) deserializes into any
    // `Color`-typed field; the picker's raw sRGBA array does not.
    let srgba = Srgba::new(new_rgba[0], new_rgba[1], new_rgba[2], new_rgba[3]);
    let new_json = {
        let reg = registry.read();
        color_to_canonical_json(Color::Srgba(srgba), &reg)
    };

    crate::commands::field_edit_commit(
        world,
        type_path,
        field_path,
        &new_json,
        "Set color on multiple entities",
    );
}

fn spawn_numeric_field(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    value: f64,
    field_path: String,
    source_entity: Entity,
    type_path: &str,
    depth: usize,
    is_integer: bool,
) {
    let left_padding = depth as f32 * tokens::SPACING_MD;
    let col = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(tokens::SPACING_XS),
                padding: UiRect::left(px(left_padding)),
                width: Val::Percent(100.0),
                position_type: PositionType::Relative,
                ..Default::default()
            },
            InspectorFieldRow {
                source_entity,
                type_path: type_path.to_string(),
                field_path: field_path.clone(),
            },
            ChildOf(parent),
        ))
        .id();

    // Label above
    commands.spawn((
        Text::new(label),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TYPE_NUMERIC),
        ChildOf(col),
    ));

    // The `FieldBinding` sits on the widget root, so the `ValueChange`
    // observers read it from `event.source`. The widget emits
    // `ValueChange<T>` but never self-updates `ScrubNumberInputValue`, so
    // the observers re-insert it and the refresh path syncs it from ECS.
    //
    // Integer source types (u32, i64, ...) funnel into the `I64` variant and
    // float types into `F64`. Without this split a `u32` bitmask like
    // `CollisionLayers::filters` would round through `f32` on commit.
    let mut field = commands.spawn_scene(bsn! { @ScrubNumberInput });
    field.insert((
        FieldBinding {
            source_entity,
            type_path: type_path.to_string(),
            field_path,
        },
        ChildOf(col),
    ));
    // The emitted `ValueChange<T>` type follows the value variant: `I64`
    // emits `ValueChange<i64>` and `F64` emits `ValueChange<f64>`. The
    // variant is the only format selector needed here.
    if is_integer {
        field.insert((
            ScrubNumberInputValue::I64(value as i64),
            NumberInputStep(1.0),
        ));
    } else {
        field.insert((
            ScrubNumberInputValue::F64(round_numeric_display(value)),
            NumberInputPrecision(NUMERIC_DISPLAY_PRECISION),
        ));
    }
}

fn spawn_editable_field(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    current_value: &str,
    field_path: String,
    source_entity: Entity,
    type_path: &str,
    depth: usize,
) {
    let left_padding = depth as f32 * tokens::SPACING_MD;

    let col = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(tokens::SPACING_XS),
                padding: UiRect::left(px(left_padding)),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();

    // Label above
    commands.spawn((
        Text::new(label),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_TERTIARY),
        ChildOf(col),
    ));

    // Input below
    spawn_string_input(
        commands,
        col,
        current_value,
        field_path,
        source_entity,
        type_path,
    );
}

/// When the inspector is in PIE Live mode, apply a field edit to the selected
/// preview entity immediately (so the viewport reflects the change) and stream
/// a `SetComponent` to the focused running game instance. Returns `true` when
/// handled as a live edit so the caller skips the authored `SetBsnField` path.
///
/// The preview entity IS the normal `Selection` entity in Live mode (the
/// projection has already applied the live overlay to it). A reverse-lookup
/// through `PieProjection.by_bits` finds which game-side bits to address.
fn try_route_pie_live_field_edit(
    world: &mut World,
    source_entity: Entity,
    type_path: &str,
    field_path: &str,
    field_value: serde_json::Value,
) -> bool {
    use crate::pie_mirror::PieViewMode;
    use jackdaw_pie_protocol::ControlEvent;

    if *world.resource::<PieViewMode>() != PieViewMode::Live {
        return false;
    }

    // Find the game-entity bits that correspond to this preview entity.
    let bits = crate::live_edits::live_bits_for_preview(
        world.resource::<crate::pie_projection::PieProjection>(),
        source_entity,
    );
    let Some(bits) = bits else {
        // Not a projected live entity; fall through to the authored path.
        return false;
    };

    // Read the full component value from the preview entity via reflection,
    // then merge the edited field into it so the game receives a complete
    // canonical component JSON.
    let registry = world.resource::<AppTypeRegistry>().clone();
    let mut full_value =
        crate::live_edits::serialize_component_json(world, source_entity, type_path);

    // Log the field as it lands in the merged component (post-normalization),
    // not the raw inspector input; fall back to the input when the field
    // cannot be extracted back out.
    let field_value_for_log;
    {
        let reg = registry.read();
        let raw = field_value.clone();
        crate::component_json::set_field_in_component_json(
            &mut full_value,
            type_path,
            field_path,
            field_value,
            &reg,
        );
        field_value_for_log = crate::component_json::get_field_in_component_json(
            &full_value,
            type_path,
            field_path,
            &reg,
        )
        .cloned()
        .unwrap_or(raw);
    }

    crate::pie::send_control_to_focused(
        world,
        ControlEvent::SetComponent {
            entity: bits,
            type_path: type_path.to_string(),
            value: full_value,
        },
    );
    crate::live_edits::record_live_edit(
        world,
        source_entity,
        bits,
        type_path,
        field_path,
        field_value_for_log,
    );
    true
}

/// Serialize a `Color` to its canonical reflect JSON (e.g.
/// `{"LinearRgba": {..}}`) so a live color edit round-trips through the
/// game's full-component deserializer. Falls back to the raw sRGBA array
/// when `Color` is not registered.
fn color_to_canonical_json(
    color: Color,
    registry: &bevy::reflect::TypeRegistry,
) -> serde_json::Value {
    use bevy::reflect::serde::TypedReflectSerializer;
    let serializer = TypedReflectSerializer::new(color.as_partial_reflect(), registry);
    serde_json::to_value(&serializer).unwrap_or_else(|_| {
        let srgba = color.to_srgba();
        serde_json::json!([srgba.red, srgba.green, srgba.blue, srgba.alpha])
    })
}

/// Apply a field value change with undo support via the field-edit lifecycle
/// ([`crate::commands::field_edit_commit`]). Propagates to the current selection.
fn apply_field_value_with_undo(
    world: &mut World,
    entity: Entity,
    type_path: &str,
    field_path: &str,
    new_value_str: &str,
) {
    let new_json = parse_to_json_value(new_value_str);
    if try_route_pie_live_field_edit(world, entity, type_path, field_path, new_json.clone()) {
        return;
    }

    crate::commands::field_edit_commit(
        world,
        type_path,
        field_path,
        &new_json,
        "Set field on multiple entities",
    );
}

/// Parse a text field string to the most appropriate JSON value. Integer-looking
/// input becomes a JSON integer, not a float: an integer field (`u64`, `u32`, ...)
/// rejects a float like `1.0` on reflect-deserialize and the edit silently reverts,
/// whereas an integer coerces cleanly into float fields too, so preferring it is safe.
fn parse_to_json_value(s: &str) -> serde_json::Value {
    if let Ok(v) = s.parse::<i64>() {
        serde_json::json!(v)
    } else if let Ok(v) = s.parse::<u64>() {
        serde_json::json!(v)
    } else if let Ok(v) = s.parse::<f64>() {
        serde_json::json!(v)
    } else if let Ok(v) = s.parse::<bool>() {
        serde_json::json!(v)
    } else {
        serde_json::Value::String(s.to_string())
    }
}

/// Parse a string value into a reflected value, returning true on success.
fn spawn_entity_link(commands: &mut Commands, parent: Entity, target: Entity, label: &str) {
    commands.spawn((
        Text::new(label),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_ACCENT),
        ChildOf(parent),
        observe(
            move |_: On<Pointer<Click>>,
                  mut commands: Commands,
                  mut selection: ResMut<Selection>| {
                selection.select_single(&mut commands, target);
            },
        ),
        observe(
            move |hover: On<Pointer<Over>>, mut q: Query<&mut TextColor>| {
                if let Ok(mut c) = q.get_mut(hover.event_target()) {
                    c.0 = tokens::TEXT_ACCENT_HOVER;
                }
            },
        ),
        observe(move |out: On<Pointer<Out>>, mut q: Query<&mut TextColor>| {
            if let Ok(mut c) = q.get_mut(out.event_target()) {
                c.0 = tokens::TEXT_ACCENT;
            }
        }),
    ));
}

fn spawn_list_expansion<'a>(
    commands: &mut Commands,
    parent: Entity,
    len: usize,
    get_item: impl Fn(usize) -> Option<&'a dyn PartialReflect>,
    depth: usize,
    base_path: &str,
    source_entity: Entity,
    type_path: &str,
    entity_names: &Query<&Name>,
) {
    spawn_text_row(commands, parent, &format!("[{len} items]"), depth);
    if len == 0 {
        return;
    }
    let lv = commands
        .spawn_scene(FeathersListView::scene(FeathersListViewProps::default()))
        .insert(ChildOf(parent))
        .id();
    for i in 0..len {
        if let Some(item) = get_item(i) {
            let item_entity = commands
                .spawn_scene(FeathersListRow::scene())
                .insert(ChildOf(lv))
                .id();
            let child_path = if base_path.is_empty() {
                format!("[{i}]")
            } else {
                format!("{base_path}[{i}]")
            };
            spawn_list_item_value(
                commands,
                item_entity,
                item,
                depth + 1,
                child_path,
                source_entity,
                type_path,
                entity_names,
            );
        }
    }
}

fn spawn_list_item_value(
    commands: &mut Commands,
    parent: Entity,
    value: &dyn PartialReflect,
    depth: usize,
    field_path: String,
    source_entity: Entity,
    type_path: &str,
    entity_names: &Query<&Name>,
) {
    // Entity -> clickable link
    if let Some(&entity_val) = value.try_downcast_ref::<Entity>() {
        let label = entity_names
            .get(entity_val)
            .map(|n| format!("{} ({entity_val})", n.as_str()))
            .unwrap_or_else(|_| format!("{entity_val}"));
        spawn_entity_link(commands, parent, entity_val, &label);
        return;
    }
    // Editable primitive -> inline text input
    if is_editable_primitive(value) {
        spawn_inline_editable(
            commands,
            parent,
            &format_partial_reflect_value(value),
            field_path,
            source_entity,
            type_path,
        );
        return;
    }
    // Compound -> recurse (list items don't have type_registry context, show as text)
    if let Some(reflected) = value.try_as_reflect() {
        // For list items we don't have type_registry context, so show text
        let text = format_reflect_value(reflected);
        spawn_text_row(commands, parent, &text, depth);
        return;
    }
    // Fallback -> plain text
    spawn_text_row(
        commands,
        parent,
        &format_partial_reflect_value(value),
        depth,
    );
}

fn spawn_inline_editable(
    commands: &mut Commands,
    parent: Entity,
    current_value: &str,
    field_path: String,
    source_entity: Entity,
    type_path: &str,
) {
    spawn_string_input(
        commands,
        parent,
        current_value,
        field_path,
        source_entity,
        type_path,
    );
}

/// Initial text staged on a string field's container. The [`Insert`]-triggered
/// [`seed_string_field`] observer reads it once the child text entry spawns
/// and writes it into the editable buffer, then removes it.
#[derive(Component)]
struct PendingFieldText(String);

/// Spawn a `FeathersTextInput` bound to a reflected string field. The
/// `FieldBinding` and initial text ride on the container entity; the inner
/// text entry emits `ValueChange<String>` on Enter or blur, which
/// `on_text_edit_commit` writes back through `apply_field_value_with_undo`.
fn spawn_string_input(
    commands: &mut Commands,
    parent: Entity,
    current_value: &str,
    field_path: String,
    source_entity: Entity,
    type_path: &str,
) {
    commands.spawn_scene(string_field_scene()).insert((
        FieldBinding {
            source_entity,
            type_path: type_path.to_string(),
            field_path,
        },
        PendingFieldText(current_value.to_string()),
        ChildOf(parent),
    ));
}

/// Container framing a `FeathersTextInput`. The inner text entry drives the
/// commit observers; the container holds the field binding and staged value,
/// and seeds the text buffer once the staged value lands.
fn string_field_scene() -> impl Scene {
    bsn! {
        @FeathersTextInputContainer
        on(seed_string_field)
        Children [
            @FeathersTextInput
            on(string_field_on_enter_key)
            on(string_field_on_focus_lost)
        ]
    }
}

/// Write the staged text into the editable buffer once the field binding's
/// value is inserted on the container, then clear the staged value so a later
/// inspector refresh does not re-seed it. Fires on the container after the
/// scene tree (including the child text entry) has spawned.
fn seed_string_field(
    inserted: On<Insert, PendingFieldText>,
    q_children: Query<&Children>,
    q_pending: Query<&PendingFieldText>,
    mut q_text: Query<&mut EditableText>,
    mut commands: Commands,
) {
    let container = inserted.event_target();
    let Ok(pending) = q_pending.get(container) else {
        return;
    };
    let text_id = q_children
        .iter_descendants(container)
        .find(|e| q_text.contains(*e));
    if let Some(text_id) = text_id
        && let Ok(mut editable) = q_text.get_mut(text_id)
    {
        editable.queue_edit(TextEdit::SelectAll);
        editable.queue_edit(TextEdit::Insert(pending.0.clone().into()));
    }
    commands.entity(container).remove::<PendingFieldText>();
}

/// Emit a final `ValueChange<String>` when Enter is pressed in a string field.
fn string_field_on_enter_key(
    key_input: On<FocusedInput<KeyboardInput>>,
    q_text: Query<&EditableText>,
    mut commands: Commands,
) {
    if key_input.input.key_code != KeyCode::Enter {
        return;
    }
    let text_id = key_input.event_target();
    if let Ok(editable) = q_text.get(text_id) {
        commands.trigger(ValueChange {
            source: text_id,
            value: editable.value().to_string(),
            is_final: true,
        });
    }
}

/// Emit a final `ValueChange<String>` when a string field loses focus.
fn string_field_on_focus_lost(
    focus_lost: On<FocusLost>,
    q_text: Query<&EditableText>,
    mut commands: Commands,
) {
    let text_id = focus_lost.event_target();
    if let Ok(editable) = q_text.get(text_id) {
        commands.trigger(ValueChange {
            source: text_id,
            value: editable.value().to_string(),
            is_final: true,
        });
    }
}

fn spawn_text_row(commands: &mut Commands, parent: Entity, text: &str, depth: usize) {
    let left_padding = depth as f32 * tokens::SPACING_MD;
    commands.spawn((
        Node {
            padding: UiRect::left(px(left_padding)),
            ..Default::default()
        },
        Text::new(text),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        ThemedText,
        ChildOf(parent),
    ));
}

fn format_reflect_value(value: &dyn Reflect) -> String {
    format_partial_reflect_value(value.as_partial_reflect())
}

fn format_partial_reflect_value(value: &dyn PartialReflect) -> String {
    if let Some(v) = value.try_downcast_ref::<f32>() {
        return format!("{v:.3}");
    }
    if let Some(v) = value.try_downcast_ref::<f64>() {
        return format!("{v:.3}");
    }
    if let Some(v) = value.try_downcast_ref::<bool>() {
        return format!("{v}");
    }
    if let Some(v) = value.try_downcast_ref::<String>() {
        return format!("\"{v}\"");
    }
    if let Some(v) = value.try_downcast_ref::<i32>() {
        return format!("{v}");
    }
    if let Some(v) = value.try_downcast_ref::<u32>() {
        return format!("{v}");
    }
    if let Some(v) = value.try_downcast_ref::<usize>() {
        return format!("{v}");
    }
    if let Some(v) = value.try_downcast_ref::<i8>() {
        return format!("{v}");
    }
    if let Some(v) = value.try_downcast_ref::<i16>() {
        return format!("{v}");
    }
    if let Some(v) = value.try_downcast_ref::<i64>() {
        return format!("{v}");
    }
    if let Some(v) = value.try_downcast_ref::<u8>() {
        return format!("{v}");
    }
    if let Some(v) = value.try_downcast_ref::<u16>() {
        return format!("{v}");
    }
    if let Some(v) = value.try_downcast_ref::<u64>() {
        return format!("{v}");
    }

    // Handle<T> and other opaque types -> clean format
    if is_opaque_type(value) {
        return "<opaque>".to_string();
    }

    // Fallback: show type name if available, otherwise Debug
    if let Some(info) = value.get_represented_type_info() {
        return format!("<{}>", info.type_path_table().short_path());
    }
    format!("{value:?}")
}

/// Handle `ValueChange<String>` for inspector string fields. The committed
/// text is applied verbatim. The event fires on the text entry, so the field
/// binding is found by walking up to its container. Numeric and
/// vector-component fields go through `ScrubNumberInput` and its
/// `on_numeric_value_change_*` observers instead.
pub(crate) fn on_text_edit_commit(
    event: On<ValueChange<String>>,
    bindings: Query<&FieldBinding>,
    child_of_query: Query<&ChildOf>,
    mut commands: Commands,
    remote_proxies: Query<(), With<crate::remote::entity_browser::RemoteEntityProxy>>,
) {
    // Only the drag/typing-finished commit writes back.
    if !event.is_final {
        return;
    }

    // Walk up from the source entity to find a FieldBinding.
    // Check the entity itself first, then walk up through parents.
    let mut current = event.source;
    let mut found = None;

    // Check the source entity itself
    if let Ok(binding) = bindings.get(current) {
        found = Some((
            binding.source_entity,
            binding.type_path.clone(),
            binding.field_path.clone(),
        ));
    }

    // Walk up through parents (up to 6 levels for deeply nested UI)
    if found.is_none() {
        for _ in 0..6 {
            let Ok(child_of) = child_of_query.get(current) else {
                break;
            };
            if let Ok(binding) = bindings.get(child_of.parent()) {
                found = Some((
                    binding.source_entity,
                    binding.type_path.clone(),
                    binding.field_path.clone(),
                ));
                break;
            }
            current = child_of.parent();
        }
    }

    // No binding on this widget: another `ValueChange<String>` producer, not an
    // inspector string field.
    let Some((source_entity, tp, path)) = found else {
        return;
    };

    // Skip edits targeting remote proxy entities (read-only inspector)
    if remote_proxies.contains(source_entity) {
        return;
    }

    let value_str = event.value.clone();
    commands.queue(move |world: &mut World| {
        apply_field_value_with_undo(world, source_entity, &tp, &path, &value_str);
    });
}

/// Apply a drag-tick (non-final) numeric edit via [`crate::commands::field_edit_preview`].
/// Live ECS only, no document write or minting of undo. In PIE Live mode it routes
/// through the live-edit stream instead.
fn apply_field_value_live(
    world: &mut World,
    source_entity: Entity,
    type_path: &str,
    field_path: &str,
    new_value_str: &str,
) {
    let new_json = parse_to_json_value(new_value_str);
    if try_route_pie_live_field_edit(
        world,
        source_entity,
        type_path,
        field_path,
        new_json.clone(),
    ) {
        return;
    }
    crate::commands::field_edit_preview(world, type_path, field_path, &new_json);
}

/// Write path for float drag-scrub fields. The widget emits
/// `ValueChange<f64>` on every drag tick with `is_final == false`, and once
/// on drag-end, Enter, or blur with `is_final == true`, but never
/// self-updates its value. This re-inserts `ScrubNumberInputValue` to move
/// the display, applies live to the ECS each tick, and pushes the
/// undo-backed `SetBsnField` only on the final event, giving one undo entry
/// per drag.
pub(crate) fn on_numeric_value_change_f64(
    event: On<ValueChange<f64>>,
    bindings: Query<&FieldBinding>,
    mut commands: Commands,
    remote_proxies: Query<(), With<crate::remote::entity_browser::RemoteEntityProxy>>,
) {
    let source = event.source;
    let Ok(binding) = bindings.get(source) else {
        return;
    };
    let target = binding.source_entity;
    if remote_proxies.contains(target) {
        return;
    }
    let tp = binding.type_path.clone();
    let path = binding.field_path.clone();
    let value = event.value;
    let is_final = event.is_final;

    // The widget does not self-update its value; re-insert it so the field
    // tracks the drag.
    commands
        .entity(source)
        .insert(ScrubNumberInputValue::F64(value));

    let value_str = format!("{value}");
    commands.queue(move |world: &mut World| {
        if is_final {
            apply_field_value_with_undo(world, target, &tp, &path, &value_str);
        } else {
            apply_field_value_live(world, target, &tp, &path, &value_str);
        }
    });
}

/// Write path for integer drag-scrub fields, handling the `ValueChange<i64>`
/// the `I64` format emits. See [`on_numeric_value_change_f64`] for the same
/// logic on floats.
pub(crate) fn on_numeric_value_change_i64(
    event: On<ValueChange<i64>>,
    bindings: Query<&FieldBinding>,
    mut commands: Commands,
    remote_proxies: Query<(), With<crate::remote::entity_browser::RemoteEntityProxy>>,
) {
    let source = event.source;
    let Ok(binding) = bindings.get(source) else {
        return;
    };
    let target = binding.source_entity;
    if remote_proxies.contains(target) {
        return;
    }
    let tp = binding.type_path.clone();
    let path = binding.field_path.clone();
    let value = event.value;
    let is_final = event.is_final;

    // The widget does not self-update its value; re-insert it so the field
    // tracks the drag.
    commands
        .entity(source)
        .insert(ScrubNumberInputValue::I64(value));

    let value_str = format!("{value}");
    commands.queue(move |world: &mut World| {
        if is_final {
            apply_field_value_with_undo(world, target, &tp, &path, &value_str);
        } else {
            apply_field_value_live(world, target, &tp, &path, &value_str);
        }
    });
}

pub(crate) fn on_checkbox_commit(
    event: On<ValueChange<bool>>,
    bindings: Query<&FieldBinding>,
    mut commands: Commands,
    remote_proxies: Query<(), With<crate::remote::entity_browser::RemoteEntityProxy>>,
) {
    // This global observer sees `ValueChange<bool>` for every checkbox, so
    // the `FieldBinding` lookup self-filters to inspector fields.
    let target = event.source;
    let Ok(binding) = bindings.get(target) else {
        return;
    };
    let source = binding.source_entity;

    // Skip edits targeting remote proxy entities (read-only inspector)
    if remote_proxies.contains(source) {
        return;
    }
    let tp = binding.type_path.clone();
    let path = binding.field_path.clone();
    let checked = event.value;
    // The checkbox does not self-update `Checked`; reflect the new value so
    // the box renders the change. Feathers styles the box off `Checked`.
    if checked {
        commands.entity(target).insert(Checked);
    } else {
        commands.entity(target).remove::<Checked>();
    }
    let val = format!("{checked}");
    commands.queue(move |world: &mut World| {
        apply_field_value_with_undo(world, source, &tp, &path, &val);
    });
}

/// True when any component on `entity_ref` changed within the tick window
/// `(last_run, this_run]`. Gates the reflection-based field/enum refresh on real
/// data changes (undo, gizmos, operators, AST live-edit all mark the component
/// changed) instead of polling reflection every frame.
fn entity_components_changed(
    entity_ref: bevy::ecs::world::EntityRef,
    last_run: bevy::ecs::change_detection::Tick,
    this_run: bevy::ecs::change_detection::Tick,
) -> bool {
    entity_ref.archetype().components().iter().any(|&id| {
        entity_ref
            .get_change_ticks_by_id(id)
            .is_some_and(|ticks| ticks.is_changed(last_run, this_run))
    })
}

/// Refreshes inspector field values using reflection -- handles all component types generically.
/// Uses exclusive world access to avoid query conflicts.
pub(crate) fn refresh_inspector_fields(
    world: &mut World,
    mut last_run: Local<Option<bevy::ecs::change_detection::Tick>>,
) {
    let this_run = world.read_change_tick();
    let prev_run = last_run.replace(this_run);

    let Some(primary) = world.resource::<Selection>().primary() else {
        return;
    };

    // Proper change detection: field values are read from the inspected
    // entity's components, so only do the reflect + value sync when one of those
    // components actually changed since this system last ran. Skips the
    // per-frame reflect + string churn for a static entity; always refreshes on
    // the first run (`prev_run` is None).
    if let Some(prev_run) = prev_run {
        let Ok(entity_ref) = world.get_entity(primary) else {
            return;
        };
        if !entity_components_changed(entity_ref, prev_run, this_run) {
            return;
        }
    }

    let type_registry = world.resource::<AppTypeRegistry>().clone();
    let registry = type_registry.read();

    // Collect drag-scrub binding info: the `ScrubNumberInput` root entity and
    // its current value. This covers scalar numeric fields and the per-axis
    // vector component inputs (both carry `ScrubNumberInput` + `FieldBinding`).
    // Refresh re-inserts the value when the reflected ECS value diverges, as
    // after an undo/redo, gizmo edit, or external edit.
    let mut scrub_lookups: Vec<(Entity, String, String, ScrubNumberInputValue)> = Vec::new();
    let mut scrub_query = world
        .query_filtered::<(Entity, &FieldBinding, &ScrubNumberInputValue), With<ScrubNumberInput>>(
        );
    for (entity, binding, value) in scrub_query.iter(world) {
        if binding.source_entity == primary {
            scrub_lookups.push((
                entity,
                binding.type_path.clone(),
                binding.field_path.clone(),
                *value,
            ));
        }
    }

    // Collect checkbox binding info and current state. Checkboxes carry
    // `Checkbox` and the field's `FieldBinding`, and current state is
    // `Has<Checked>`. The `With<Checkbox>` filter keeps numeric `FieldBinding`
    // rows out.
    let mut bool_lookups: Vec<(Entity, String, String, bool)> = Vec::new();
    let mut checkbox_query =
        world.query_filtered::<(Entity, &FieldBinding, Has<Checked>), With<Checkbox>>();
    for (entity, binding, checked) in checkbox_query.iter(world) {
        if binding.source_entity == primary {
            bool_lookups.push((
                entity,
                binding.type_path.clone(),
                binding.field_path.clone(),
                checked,
            ));
        }
    }

    if scrub_lookups.is_empty() && bool_lookups.is_empty() {
        return;
    }

    let mut scrub_updates: Vec<(Entity, ScrubNumberInputValue)> = Vec::new();
    let mut bool_updates: Vec<(Entity, bool)> = Vec::new();
    let Ok(entity_ref) = world.get_entity(primary) else {
        return;
    };

    for (ui_entity, comp_type_path, field_path, current) in &scrub_lookups {
        let Some(registration) = registry.get_with_type_path(comp_type_path) else {
            continue;
        };
        let Some(reflect_component) = registration.data::<ReflectComponent>() else {
            continue;
        };
        let Some(reflected) = reflect_component.reflect(entity_ref) else {
            continue;
        };
        let Ok(field) = reflected.reflect_path(field_path.as_str()) else {
            continue;
        };
        let Some(value) = reflect_field_to_f64(field) else {
            continue;
        };

        // Preserve the field's variant. Integer fields stay `I64` and compare
        // exactly; everything else is treated as `F64` and compared with the
        // display epsilon.
        let update = match current {
            ScrubNumberInputValue::I64(cur) => {
                let new_i = value.round() as i64;
                (new_i != *cur).then_some(ScrubNumberInputValue::I64(new_i))
            }
            _ => {
                let new_f = round_numeric_display(value);
                ((new_f - scrub_value_as_f64(current)).abs() > 0.005)
                    .then_some(ScrubNumberInputValue::F64(new_f))
            }
        };
        if let Some(update) = update {
            scrub_updates.push((*ui_entity, update));
        }
    }

    for (ui_entity, comp_type_path, field_path, current_checked) in &bool_lookups {
        let Some(registration) = registry.get_with_type_path(comp_type_path) else {
            continue;
        };
        let Some(reflect_component) = registration.data::<ReflectComponent>() else {
            continue;
        };
        let Some(reflected) = reflect_component.reflect(entity_ref) else {
            continue;
        };
        let Ok(field) = reflected.reflect_path(field_path.as_str()) else {
            continue;
        };
        if let Some(&val) = field.try_downcast_ref::<bool>()
            && val != *current_checked
        {
            bool_updates.push((*ui_entity, val));
        }
    }

    drop(registry);

    let input_focus = world.resource::<InputFocus>().get();

    // Apply drag-scrub updates by re-inserting `ScrubNumberInputValue`; the
    // widget's own insert observer repaints the text. Skip while the user is
    // mid-scrub or typing in the field so the refresh never clobbers an
    // in-progress edit.
    for (entity, value) in scrub_updates {
        if is_scrubbing_or_focused(world, entity, input_focus) {
            continue;
        }
        world.entity_mut(entity).insert(value);
    }

    // Apply bool updates by inserting or removing `Checked`; the checkbox
    // renders off `Checked`. Undo/redo and external edits land here.
    for (entity, value) in bool_updates {
        let mut ent = world.entity_mut(entity);
        if value {
            ent.insert(Checked);
        } else {
            ent.remove::<Checked>();
        }
    }
}

/// HACK: polling-based enum-variant refresh. Compares each `EnumVariantHost`'s
/// stored variant name against the ECS value via reflection every frame, and
/// rebuilds the subtree (select menu + field rows) in place when they differ.
///
/// This covers all update sources uniformly (user click, undo/redo, external
/// edits) via a single ECS-mismatch trigger, but the per-frame poll is wasteful
/// and also races slightly with user interaction. Replace with proper observer-
/// based reactivity (component change hooks / mutation events) once the
/// underlying AST sync fires observable events we can subscribe to.
pub(crate) fn refresh_enum_variants(
    mut commands: Commands,
    selection: Res<Selection>,
    type_registry: Res<AppTypeRegistry>,
    entity_names: Query<&Name>,
    editor_font: Res<jackdaw_feathers::icons::EditorFont>,
    icon_font: Res<jackdaw_feathers::icons::IconFont>,
    mut hosts: Query<(Entity, &mut super::EnumVariantHost, &Children)>,
    // `Without<EnumVariantHost>` makes this query disjoint from `hosts` so the
    // two queries can coexist  -- we only ever need to read the selected source
    // entity, never a UI container.
    entity_query: Query<bevy::ecs::world::EntityRef, Without<super::EnumVariantHost>>,
    dirty_sources: Query<(), With<super::InspectorDirty>>,
    system_ticks: bevy::ecs::system::SystemChangeTick,
) {
    let Some(primary) = selection.primary() else {
        return;
    };
    // Skip if an inspector rebuild is pending for the primary; its
    // cascade-despawn would race our spawns and log `ChildOf(X)
    // relates to an entity that does not exist` WARNs.
    if dirty_sources.contains(primary) {
        return;
    }
    let registry_guard = type_registry.read();
    let Ok(entity_ref) = entity_query.get(primary) else {
        return;
    };

    // Proper change detection: only re-check the reflected enum value when a
    // component on the inspected entity changed since our last run (or the
    // selection just changed), instead of the per-frame reflect poll the doc
    // comment above flags as wasteful.
    if !selection.is_changed()
        && !entity_components_changed(entity_ref, system_ticks.last_run(), system_ticks.this_run())
    {
        return;
    }

    for (container, mut host, children) in &mut hosts {
        if host.source_entity != primary {
            continue;
        }

        // Resolve the enum via reflection
        let Some(registration) = registry_guard.get_with_type_path(host.type_path.as_str()) else {
            continue;
        };
        let Some(reflect_component) = registration.data::<ReflectComponent>() else {
            continue;
        };
        let Some(reflected) = reflect_component.reflect(entity_ref) else {
            continue;
        };

        let enum_partial: &dyn bevy::reflect::PartialReflect = if host.field_path.is_empty() {
            reflected.as_partial_reflect()
        } else {
            let Ok(field) = reflected.reflect_path(host.field_path.as_str()) else {
                continue;
            };
            field
        };

        let ReflectRef::Enum(e) = enum_partial.reflect_ref() else {
            continue;
        };
        if e.variant_name() == host.current_variant {
            continue;
        }

        // Despawn the old children (select menu + field rows) and repopulate
        for child in children.iter() {
            commands.entity(child).despawn();
        }

        let new_variant = e.variant_name().to_string();
        spawn_variant_contents(
            &mut commands,
            container,
            &host,
            e,
            &entity_names,
            &type_registry,
            &editor_font.0,
            &icon_font.0,
        );

        host.current_variant = new_variant;
    }
}

fn reflect_field_to_f64(field: &dyn PartialReflect) -> Option<f64> {
    if let Some(&v) = field.try_downcast_ref::<f32>() {
        Some(v as f64)
    } else if let Some(&v) = field.try_downcast_ref::<f64>() {
        Some(v)
    } else if let Some(&v) = field.try_downcast_ref::<i32>() {
        Some(v as f64)
    } else if let Some(&v) = field.try_downcast_ref::<u32>() {
        Some(v as f64)
    } else if let Some(&v) = field.try_downcast_ref::<usize>() {
        Some(v as f64)
    } else if let Some(&v) = field.try_downcast_ref::<i8>() {
        Some(v as f64)
    } else if let Some(&v) = field.try_downcast_ref::<i16>() {
        Some(v as f64)
    } else if let Some(&v) = field.try_downcast_ref::<i64>() {
        Some(v as f64)
    } else if let Some(&v) = field.try_downcast_ref::<u8>() {
        Some(v as f64)
    } else if let Some(&v) = field.try_downcast_ref::<u16>() {
        Some(v as f64)
    } else if let Some(&v) = field.try_downcast_ref::<u64>() {
        Some(v as f64)
    } else {
        None
    }
}

/// Check if a type is opaque and shouldn't be recursed into for inspection.
fn is_opaque_type(value: &dyn PartialReflect) -> bool {
    let Some(type_info) = value.get_represented_type_info() else {
        return false;
    };
    let type_path = type_info.type_path();
    type_path.starts_with("bevy_asset::handle::Handle")
        || type_path.starts_with("bevy_asset::id::AssetId")
        || type_path.contains("Cow<")
}

/// Spawn a select menu for enum fields, supporting unit-only enums with undo.
fn spawn_enum_field(
    commands: &mut Commands,
    parent: Entity,
    enum_ref: &dyn bevy::reflect::enums::Enum,
    depth: usize,
    field_path: String,
    source_entity: Entity,
    type_path: &str,
    entity_names: &Query<&Name>,
    type_registry: &AppTypeRegistry,
    editor_font: &Handle<Font>,
    icon_font: &Handle<Font>,
) {
    let current_variant = enum_ref.variant_name().to_string();

    // Try to get variant names from type info
    let Some(type_info) = enum_ref.get_represented_type_info() else {
        spawn_text_row(
            commands,
            parent,
            &format!("variant: {current_variant}"),
            depth,
        );
        return;
    };
    let bevy::reflect::TypeInfo::Enum(enum_info) = type_info else {
        spawn_text_row(
            commands,
            parent,
            &format!("variant: {current_variant}"),
            depth,
        );
        return;
    };

    let variant_names: Vec<String> = enum_info
        .variant_names()
        .iter()
        .map(std::string::ToString::to_string)
        .collect();

    if variant_names.is_empty() {
        spawn_text_row(
            commands,
            parent,
            &format!("variant: {current_variant}"),
            depth,
        );
        return;
    }

    // Check if all variants are unit variants
    let all_unit = (0..enum_info.variant_len()).all(|i| {
        enum_info
            .variant_at(i)
            .map(|v| matches!(v, bevy::reflect::enums::VariantInfo::Unit(_)))
            .unwrap_or(false)
    });

    let left_padding = depth as f32 * tokens::SPACING_MD;

    if all_unit {
        // Menu-button + popup in a row for unit-only enums.
        let row = commands
            .spawn((
                Node {
                    flex_direction: FlexDirection::Row,
                    align_items: AlignItems::Center,
                    column_gap: px(tokens::SPACING_XS),
                    padding: UiRect::left(px(left_padding)),
                    width: Val::Percent(100.0),
                    ..Default::default()
                },
                ChildOf(parent),
            ))
            .id();

        spawn_enum_menu(
            commands,
            row,
            &variant_names,
            &current_variant,
            &field_path,
            source_entity,
            type_path,
        );
    } else {
        // Container + select menu + field rows. Tagged with `EnumVariantHost`
        // so `refresh_enum_variants` can swap in new field rows when the ECS
        // variant changes (user click, undo, redo, external edit).
        let container = commands
            .spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::left(px(left_padding)),
                    row_gap: px(tokens::SPACING_XS),
                    width: Val::Percent(100.0),
                    ..Default::default()
                },
                ChildOf(parent),
            ))
            .id();

        let host = super::EnumVariantHost {
            source_entity,
            type_path: type_path.to_string(),
            field_path: field_path.clone(),
            depth,
            current_variant: enum_ref.variant_name().to_string(),
        };
        spawn_variant_contents(
            commands,
            container,
            &host,
            enum_ref,
            entity_names,
            type_registry,
            editor_font,
            icon_font,
        );
        commands.entity(container).insert(host);
    }
}

/// Spawn a select menu for an enum field: a `FeathersMenu` holding a
/// `FeathersMenuButton` (caption = `current_variant`) and a `FeathersMenuPopup`
/// with one `FeathersMenuItem` per name in `variant_names`. Picking an item
/// fires `Activate`, whose observer captures that item's variant name and calls
/// `apply_enum_variant_with_undo`. The menu is parented to `parent`.
fn spawn_enum_menu(
    commands: &mut Commands,
    parent: Entity,
    variant_names: &[String],
    current_variant: &str,
    field_path: &str,
    source_entity: Entity,
    type_path: &str,
) {
    let menu = commands
        .spawn_scene(bsn! { @FeathersMenu })
        .insert(ChildOf(parent))
        .id();

    // Button caption shows the current variant. The struct/tuple refresh path
    // rebuilds the whole menu, so the caption tracks the live variant without a
    // separate text rewrite.
    commands
        .spawn_scene(bsn! {
            @FeathersMenuButton {
                @caption: bsn! { Text({current_variant.to_string()}) ThemedText },
            }
        })
        .insert(ChildOf(menu));

    let popup = commands
        .spawn_scene(bsn! { @FeathersMenuPopup })
        .insert(ChildOf(menu))
        .id();

    // One item per variant. Each closure owns its variant name so `Activate`
    // writes back the picked variant.
    for name in variant_names {
        let variant_name = name.clone();
        let path = field_path.to_string();
        let tp = type_path.to_string();
        commands
            .spawn_scene(bsn! {
                @FeathersMenuItem {
                    @caption: bsn! { Text({name.clone()}) ThemedText },
                }
            })
            .insert(ChildOf(popup))
            .observe(move |_activate: On<Activate>, mut commands: Commands| {
                let variant_name = variant_name.clone();
                let path = path.clone();
                let tp = tp.clone();
                commands.queue(move |world: &mut World| {
                    apply_enum_variant_with_undo(world, source_entity, &tp, &path, &variant_name);
                });
            });
    }
}

/// Spawn the select menu + current-variant field rows as children of
/// `container`. Called both on initial UI build and from `refresh_enum_variants`
/// after despawning the old children. `host` bundles `source_entity`,
/// `type_path`, `field_path`, `depth` so those four don't appear as separate
/// arguments.
pub(super) fn spawn_variant_contents(
    commands: &mut Commands,
    container: Entity,
    host: &super::EnumVariantHost,
    enum_ref: &dyn bevy::reflect::enums::Enum,
    entity_names: &Query<&Name>,
    type_registry: &AppTypeRegistry,
    editor_font: &Handle<Font>,
    icon_font: &Handle<Font>,
) {
    // Variant names come straight from reflect.
    let Some(type_info) = enum_ref.get_represented_type_info() else {
        return;
    };
    let bevy::reflect::TypeInfo::Enum(enum_info) = type_info else {
        return;
    };

    let variant_names: Vec<String> = enum_info
        .variant_names()
        .iter()
        .map(std::string::ToString::to_string)
        .collect();
    if variant_names.is_empty() {
        return;
    }
    let current_variant = enum_ref.variant_name();

    spawn_enum_menu(
        commands,
        container,
        &variant_names,
        current_variant,
        &host.field_path,
        host.source_entity,
        &host.type_path,
    );

    // Spawn a row for each field of the current variant
    let variant_field_count = enum_ref.field_len();
    for i in 0..variant_field_count {
        let Some(field_value) = enum_ref.field_at(i) else {
            continue;
        };
        let field_name = enum_ref
            .name_at(i)
            .map(std::string::ToString::to_string)
            .unwrap_or_else(|| format!("{i}"));
        let child_path = if host.field_path.is_empty() {
            field_name.clone()
        } else {
            format!("{}.{}", host.field_path, field_name)
        };
        spawn_field_row(
            commands,
            container,
            &field_name,
            field_value,
            host.depth + 1,
            child_path,
            host.source_entity,
            &host.type_path,
            entity_names,
            type_registry,
            editor_font,
            icon_font,
        );
    }
}

/// Apply an enum variant change with undo support.
fn apply_enum_variant_with_undo(
    world: &mut World,
    _entity: Entity,
    type_path: &str,
    field_path: &str,
    variant_name: &str,
) {
    let registry = world.resource::<AppTypeRegistry>().clone();

    // Build JSON that the TypedReflectDeserializer can round-trip. A bare
    // variant-name string only works for *unit* variants; struct/tuple variants
    // need `{"VariantName": {fields}}` / `{"VariantName": [items]}` with the
    // fields populated from each field type's `ReflectDefault`.
    let new_json = {
        let reg = registry.read();
        resolve_enum_info(type_path, field_path, &reg)
            .and_then(|enum_info| build_variant_default_json(enum_info, variant_name, &reg))
            .unwrap_or_else(|| serde_json::Value::String(variant_name.to_string()))
    };

    if try_route_pie_live_field_edit(world, _entity, type_path, field_path, new_json.clone()) {
        return;
    }

    crate::commands::field_edit_commit(
        world,
        type_path,
        field_path,
        &new_json,
        "Set enum on multiple entities",
    );
    // No need to flag anything -- `refresh_enum_variants` detects the ECS
    // variant change and rebuilds the affected subtree automatically. Same
    // goes for undo/redo since the command framework mutates the ECS too.
}

/// Walk from a component type through a dotted field path to find the enum `TypeInfo`
/// at that location. Returns `None` if the path doesn't terminate on an enum.
fn resolve_enum_info<'a>(
    type_path: &str,
    field_path: &str,
    registry: &'a bevy::reflect::TypeRegistry,
) -> Option<&'a bevy::reflect::enums::EnumInfo> {
    use bevy::reflect::TypeInfo;

    let mut current_reg = registry.get_with_type_path(type_path)?;
    let mut current_info = current_reg.type_info();

    for segment in field_path.split('.').filter(|s| !s.is_empty()) {
        let field_type_id = match current_info {
            TypeInfo::Struct(s) => s.field(segment).map(NamedField::type_id)?,
            TypeInfo::TupleStruct(ts) => {
                let idx: usize = segment.parse().ok()?;
                ts.field_at(idx).map(UnnamedField::type_id)?
            }
            _ => return None,
        };
        current_reg = registry.get(field_type_id)?;
        current_info = current_reg.type_info();
    }

    if let TypeInfo::Enum(enum_info) = current_info {
        Some(enum_info)
    } else {
        None
    }
}

/// Build JSON in Bevy's reflect-serialization format for a freshly-constructed
/// default of the named enum variant. Returns `None` if any field type lacks
/// `ReflectDefault`.
fn build_variant_default_json(
    enum_info: &bevy::reflect::enums::EnumInfo,
    variant_name: &str,
    registry: &bevy::reflect::TypeRegistry,
) -> Option<serde_json::Value> {
    use bevy::reflect::enums::VariantInfo;
    use bevy::reflect::{prelude::ReflectDefault, serde::TypedReflectSerializer};

    let variant = enum_info.variant(variant_name)?;

    match variant {
        VariantInfo::Unit(_) => Some(serde_json::Value::String(variant_name.to_string())),
        VariantInfo::Struct(struct_info) => {
            let mut fields = serde_json::Map::new();
            for i in 0..struct_info.field_len() {
                let field = struct_info.field_at(i)?;
                let field_reg = registry.get(field.type_id())?;
                let default = field_reg.data::<ReflectDefault>()?.default();
                let serializer =
                    TypedReflectSerializer::new(default.as_ref().as_partial_reflect(), registry);
                let value = serde_json::to_value(&serializer).ok()?;
                fields.insert(field.name().to_string(), value);
            }
            let mut outer = serde_json::Map::new();
            outer.insert(variant_name.to_string(), serde_json::Value::Object(fields));
            Some(serde_json::Value::Object(outer))
        }
        VariantInfo::Tuple(tuple_info) => {
            let mut values = Vec::with_capacity(tuple_info.field_len());
            for i in 0..tuple_info.field_len() {
                let field = tuple_info.field_at(i)?;
                let field_reg = registry.get(field.type_id())?;
                let default = field_reg.data::<ReflectDefault>()?.default();
                let serializer =
                    TypedReflectSerializer::new(default.as_ref().as_partial_reflect(), registry);
                values.push(serde_json::to_value(&serializer).ok()?);
            }
            let mut outer = serde_json::Map::new();
            outer.insert(variant_name.to_string(), serde_json::Value::Array(values));
            Some(serde_json::Value::Object(outer))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_to_json_value;
    use serde_json::Value;

    #[test]
    fn integer_input_parses_to_json_integer() {
        // An integer component field (e.g. `SpawnPoint.zone: u64`) only round-trips
        // when the edit is a JSON integer; a float like `1.0` fails reflect
        // deserialization and the value silently reverts to its default.
        let v = parse_to_json_value("1");
        assert_eq!(v.as_u64(), Some(1), "got {v:?}");
        assert!(!v.is_f64(), "integer input must not become a float: {v:?}");
    }

    #[test]
    fn decimal_input_parses_to_json_float() {
        assert_eq!(parse_to_json_value("1.5").as_f64(), Some(1.5));
    }

    #[test]
    fn non_numeric_input_parses_to_bool_or_string() {
        assert_eq!(parse_to_json_value("true"), Value::Bool(true));
        assert_eq!(
            parse_to_json_value("north_gate"),
            Value::String("north_gate".to_string())
        );
    }

    // Pins the change-detection gate that replaced the per-frame reflection
    // poll in `refresh_inspector_fields` / `refresh_enum_variants`: a component
    // mutated between two runs must be detected, and an unchanged entity must
    // not be. Ticks advance between system runs in reality, so the test
    // increments the world tick between capturing `last_run` and mutating.
    #[test]
    fn entity_components_changed_tracks_mutations() {
        use super::entity_components_changed;
        use bevy::prelude::*;

        #[derive(Component)]
        struct Probe(u32);

        let mut world = World::new();
        let e = world.spawn(Probe(1)).id();

        let last_run = world.change_tick();
        world.increment_change_tick();
        world.entity_mut(e).get_mut::<Probe>().unwrap().0 = 2;
        world.increment_change_tick();
        let this_run = world.change_tick();
        assert!(
            entity_components_changed(world.entity(e), last_run, this_run),
            "a mutation between runs must be detected"
        );

        world.increment_change_tick();
        let later = world.change_tick();
        assert!(
            !entity_components_changed(world.entity(e), this_run, later),
            "no mutation since `this_run` means no change"
        );
    }
}
