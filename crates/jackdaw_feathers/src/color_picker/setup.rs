use bevy::picking::prelude::Pickable;
use bevy::prelude::*;

use super::color_math::hsv_to_rgb;
use super::controls::{
    on_control_drag, on_control_drag_end, on_control_drag_start, on_control_press,
    on_control_release,
};
use super::input_fields::spawn_input_fields;
use super::materials::{
    AlphaSliderMaterial, CheckerboardMaterial, HsvRectMaterial, HueSliderMaterial,
};
use super::{
    AlphaHandle, AlphaHandleMaterial, AlphaMaterialNode, AlphaSlider, BORDER_RADIUS,
    CHECKERBOARD_SIZE, ColorInputRow, ColorPickerConfig, ColorPickerContent, ColorPickerPopover,
    ColorPickerState, ColorPickerTrigger, EditorColorPicker, HANDLE_BORDER, HANDLE_SIZE,
    HSV_RECT_HEIGHT, HsvRectHandle, HsvRectMaterialNode, HsvRectangle, HueHandle, HueSlider,
    POPOVER_WIDTH, PREVIEW_CHECKERBOARD_SIZE, PREVIEW_SWATCH_SIZE, PreviewSwatchMaterial,
    SLIDER_HEIGHT, SWATCH_SIZE, TriggerLabel, TriggerSwatch, TriggerSwatchConfig,
    TriggerSwatchMaterial,
};

use crate::button::{ButtonClickEvent, ButtonProps, ButtonVariant, button};
use crate::icons::{EditorFont, IconFont};
use crate::popover::{
    PopoverHeaderProps, PopoverPlacement, PopoverProps, PopoverTracker, activate_trigger,
    deactivate_trigger, popover, popover_content, popover_header,
};

pub(super) fn handle_style(left: f32, top: f32, color: Option<Srgba>, size: f32) -> impl Bundle {
    (
        Pickable::IGNORE,
        Node {
            position_type: PositionType::Absolute,
            width: px(size),
            height: px(size),
            left: px(left),
            top: px(top),
            border: UiRect::all(px(HANDLE_BORDER)),
            border_radius: BorderRadius::all(px(size / 2.0)),
            ..default()
        },
        BackgroundColor(color.unwrap_or(Srgba::NONE).into()),
        BorderColor::all(Srgba::WHITE),
        Outline {
            width: px(1.0),
            color: Srgba::BLACK.into(),
            ..default()
        },
    )
}

pub(super) fn slider_node() -> Node {
    Node {
        width: percent(100.0),
        height: px(SLIDER_HEIGHT),
        ..default()
    }
}

pub(super) fn fullsize_absolute_node() -> Node {
    Node {
        position_type: PositionType::Absolute,
        width: percent(100.0),
        height: percent(100.0),
        ..default()
    }
}

pub(super) fn setup_color_picker(
    mut commands: Commands,
    mut pickers: Query<(Entity, &ColorPickerConfig, &ColorPickerState), Added<EditorColorPicker>>,
) {
    for (entity, config, state) in &mut pickers {
        if config.inline {
            // Spawn the inline content through a liveness-checked world closure so
            // it cannot be parented to a despawned picker. A plain deferred
            // `with_child` races against rapid inspector rebuilds (duplicate /
            // undo): if the picker is despawned before the command flushes, the
            // content lands with a dangling `ChildOf`, gets orphaned to the UI
            // root, and renders full-width at the window origin.
            let picker = entity;
            commands.queue(move |world: &mut World| {
                if world.get_entity(picker).is_err() {
                    return;
                }
                let content = world
                    .spawn((
                        ColorPickerContent(picker),
                        Node {
                            flex_direction: FlexDirection::Column,
                            row_gap: px(12.0),
                            width: percent(100),
                            ..default()
                        },
                    ))
                    .id();
                world.entity_mut(picker).add_child(content);
            });
        } else {
            let rgba = state.to_rgba();
            let srgba = Srgba::new(rgba[0], rgba[1], rgba[2], rgba[3]);
            let hex = state.to_hex();

            let trigger_entity = commands
                .spawn((
                    ColorPickerTrigger(entity),
                    button(
                        ButtonProps::new(hex)
                            .with_variant(ButtonVariant::Default)
                            .align_left(),
                    ),
                ))
                .id();

            commands.entity(entity).add_child(trigger_entity);

            commands.entity(trigger_entity).insert(TriggerSwatchConfig {
                picker: entity,
                color: srgba,
            });
        }
    }
}

pub(super) fn setup_trigger_swatch(
    mut commands: Commands,
    mut checkerboard_materials: ResMut<Assets<CheckerboardMaterial>>,
    triggers: Query<(Entity, &TriggerSwatchConfig, &Children)>,
    texts: Query<Entity, With<Text>>,
) {
    for (trigger_entity, config, children) in &triggers {
        commands
            .entity(trigger_entity)
            .remove::<TriggerSwatchConfig>();

        let swatch_entity = commands
            .spawn((
                TriggerSwatch,
                Node {
                    position_type: PositionType::Absolute,
                    left: px(6.0),
                    width: px(SWATCH_SIZE),
                    height: px(SWATCH_SIZE),
                    border_radius: BorderRadius::all(px(BORDER_RADIUS)),
                    overflow: Overflow::clip(),
                    ..default()
                },
            ))
            .id();

        commands.entity(swatch_entity).with_children(|parent| {
            parent.spawn((
                TriggerSwatchMaterial(config.picker),
                MaterialNode(checkerboard_materials.add(CheckerboardMaterial {
                    color: Vec4::new(
                        config.color.red,
                        config.color.green,
                        config.color.blue,
                        config.color.alpha,
                    ),
                    size: CHECKERBOARD_SIZE,
                    border_radius: BORDER_RADIUS,
                })),
                Node {
                    position_type: PositionType::Absolute,
                    width: percent(100),
                    height: percent(100),
                    ..default()
                },
            ));
        });

        commands.entity(trigger_entity).add_child(swatch_entity);

        for child in children.iter() {
            if texts.get(child).is_ok() {
                commands.entity(child).insert((
                    TriggerLabel(config.picker),
                    Node {
                        margin: UiRect::left(px(SWATCH_SIZE + 6.0)),
                        ..default()
                    },
                ));
                break;
            }
        }
    }
}

pub(super) fn handle_trigger_click(
    trigger: On<ButtonClickEvent>,
    mut commands: Commands,
    editor_font: Res<EditorFont>,
    icon_font: Res<IconFont>,
    triggers: Query<&ColorPickerTrigger>,
    mut trackers: Query<&mut PopoverTracker>,
    existing_popovers: Query<(Entity, &ColorPickerPopover)>,
    mut button_styles: Query<(&mut BackgroundColor, &mut BorderColor, &mut ButtonVariant)>,
) {
    let Ok(picker_trigger) = triggers.get(trigger.entity) else {
        return;
    };

    let picker_entity = picker_trigger.0;
    let Ok(mut tracker) = trackers.get_mut(picker_entity) else {
        return;
    };

    for (popover_entity, popover_ref) in &existing_popovers {
        if popover_ref.0 == picker_entity {
            commands.entity(popover_entity).try_despawn();
            tracker.popover = None;
            deactivate_trigger(trigger.entity, &mut button_styles);
            return;
        }
    }

    activate_trigger(trigger.entity, &mut button_styles);

    let popover_entity = commands
        .spawn((
            ColorPickerPopover(picker_entity),
            popover(
                PopoverProps::new(trigger.entity)
                    .with_placement(PopoverPlacement::RightStart)
                    .with_padding(0.0)
                    .with_z_index(150)
                    .with_node(Node {
                        width: px(POPOVER_WIDTH),
                        ..default()
                    }),
            ),
        ))
        .id();

    tracker.open(popover_entity, trigger.entity);

    commands.entity(popover_entity).with_children(|parent| {
        parent.spawn(popover_header(
            PopoverHeaderProps::new("Color", popover_entity),
            &editor_font.0,
            &icon_font.0,
        ));

        parent.spawn((ColorPickerContent(picker_entity), popover_content()));
    });
}

pub(super) fn setup_color_picker_content(
    mut commands: Commands,
    mut hsv_rect_materials: ResMut<Assets<HsvRectMaterial>>,
    mut hue_materials: ResMut<Assets<HueSliderMaterial>>,
    mut alpha_materials: ResMut<Assets<AlphaSliderMaterial>>,
    mut checkerboard_materials: ResMut<Assets<CheckerboardMaterial>>,
    states: Query<&ColorPickerState>,
    contents: Query<(Entity, &ColorPickerContent), Added<ColorPickerContent>>,
) {
    for (content_entity, content) in &contents {
        let picker_entity = content.0;
        let Ok(state) = states.get(picker_entity) else {
            continue;
        };

        commands.entity(content_entity).with_children(|parent| {
            let current_color = state.to_srgba();
            let current_rgb = hsv_to_rgb(state.hue, state.saturation, state.brightness);

            // HSV Rectangle
            parent
                .spawn((
                    HsvRectangle(picker_entity),
                    Node {
                        width: percent(100.0),
                        height: px(HSV_RECT_HEIGHT),
                        ..default()
                    },
                ))
                .with_children(|hsv_rect_parent| {
                    hsv_rect_parent.spawn((
                        HsvRectMaterialNode(picker_entity),
                        Pickable::IGNORE,
                        MaterialNode(hsv_rect_materials.add(HsvRectMaterial {
                            hue: state.hue,
                            border_radius: BORDER_RADIUS,
                        })),
                        fullsize_absolute_node(),
                    ));

                    hsv_rect_parent.spawn((
                        HsvRectHandle(picker_entity),
                        handle_style(0.0, 0.0, Some(current_color.with_alpha(1.0)), HANDLE_SIZE),
                    ));
                })
                .observe(on_control_press::<HsvRectangle>)
                .observe(on_control_release::<HsvRectangle>)
                .observe(on_control_drag_start::<HsvRectangle>)
                .observe(on_control_drag::<HsvRectangle>)
                .observe(on_control_drag_end::<HsvRectangle>);

            // Sliders + preview swatch row
            parent
                .spawn((
                    ColorPickerContentRow,
                    Node {
                        column_gap: px(12.0),
                        align_items: AlignItems::Center,
                        ..default()
                    },
                ))
                .with_children(|slider_row| {
                    // Hue + Alpha sliders column
                    slider_row
                        .spawn(Node {
                            flex_direction: FlexDirection::Column,
                            row_gap: px(6.0),
                            flex_grow: 1.0,
                            ..default()
                        })
                        .with_children(|slider_col| {
                            // Hue slider
                            slider_col
                                .spawn((HueSlider(picker_entity), slider_node()))
                                .with_children(|hue_parent| {
                                    hue_parent.spawn((
                                        Pickable::IGNORE,
                                        MaterialNode(hue_materials.add(HueSliderMaterial {
                                            border_radius: BORDER_RADIUS,
                                        })),
                                        fullsize_absolute_node(),
                                    ));

                                    let hue_color = hsv_to_rgb(state.hue, 1.0, 1.0);
                                    hue_parent.spawn((
                                        HueHandle(picker_entity),
                                        handle_style(
                                            0.0,
                                            (SLIDER_HEIGHT - HANDLE_SIZE) / 2.0,
                                            Some(Srgba::new(
                                                hue_color.0,
                                                hue_color.1,
                                                hue_color.2,
                                                1.0,
                                            )),
                                            HANDLE_SIZE,
                                        ),
                                    ));
                                })
                                .observe(on_control_press::<HueSlider>)
                                .observe(on_control_release::<HueSlider>)
                                .observe(on_control_drag_start::<HueSlider>)
                                .observe(on_control_drag::<HueSlider>)
                                .observe(on_control_drag_end::<HueSlider>);

                            // Alpha slider
                            slider_col
                                .spawn((AlphaSlider(picker_entity), slider_node()))
                                .with_children(|alpha_parent| {
                                    alpha_parent.spawn((
                                        AlphaMaterialNode(picker_entity),
                                        Pickable::IGNORE,
                                        MaterialNode(alpha_materials.add(AlphaSliderMaterial {
                                            color: Vec4::new(
                                                current_rgb.0,
                                                current_rgb.1,
                                                current_rgb.2,
                                                1.0,
                                            ),
                                            checkerboard_size: CHECKERBOARD_SIZE,
                                            border_radius: BORDER_RADIUS,
                                        })),
                                        fullsize_absolute_node(),
                                    ));

                                    let inner_size = HANDLE_SIZE - HANDLE_BORDER * 2.0;
                                    let inner_radius = inner_size / 2.0;
                                    alpha_parent
                                        .spawn((
                                            AlphaHandle(picker_entity),
                                            handle_style(
                                                0.0,
                                                (SLIDER_HEIGHT - HANDLE_SIZE) / 2.0,
                                                None,
                                                HANDLE_SIZE,
                                            ),
                                        ))
                                        .with_children(|handle| {
                                            handle
                                                .spawn((
                                                    Pickable::IGNORE,
                                                    Node {
                                                        width: px(inner_size),
                                                        height: px(inner_size),
                                                        border_radius: BorderRadius::all(px(
                                                            inner_radius,
                                                        )),
                                                        overflow: Overflow::clip(),
                                                        ..default()
                                                    },
                                                ))
                                                .with_children(|swatch| {
                                                    swatch.spawn((
                                                        AlphaHandleMaterial(picker_entity),
                                                        Pickable::IGNORE,
                                                        MaterialNode(checkerboard_materials.add(
                                                            CheckerboardMaterial {
                                                                color: Vec4::new(
                                                                    current_color.red,
                                                                    current_color.green,
                                                                    current_color.blue,
                                                                    current_color.alpha,
                                                                ),
                                                                size: CHECKERBOARD_SIZE,
                                                                border_radius: inner_size,
                                                            },
                                                        )),
                                                        Node {
                                                            position_type: PositionType::Absolute,
                                                            width: percent(100.0),
                                                            height: percent(100.0),
                                                            ..default()
                                                        },
                                                    ));
                                                });
                                        });
                                })
                                .observe(on_control_press::<AlphaSlider>)
                                .observe(on_control_release::<AlphaSlider>)
                                .observe(on_control_drag_start::<AlphaSlider>)
                                .observe(on_control_drag::<AlphaSlider>)
                                .observe(on_control_drag_end::<AlphaSlider>);
                        });

                    // Preview swatch
                    slider_row
                        .spawn((
                            Pickable::IGNORE,
                            Node {
                                width: px(PREVIEW_SWATCH_SIZE),
                                height: px(PREVIEW_SWATCH_SIZE),
                                border_radius: BorderRadius::all(px(BORDER_RADIUS)),
                                overflow: Overflow::clip(),
                                ..default()
                            },
                        ))
                        .with_children(|swatch| {
                            swatch.spawn((
                                PreviewSwatchMaterial(picker_entity),
                                Pickable::IGNORE,
                                MaterialNode(checkerboard_materials.add(CheckerboardMaterial {
                                    color: Vec4::new(
                                        current_color.red,
                                        current_color.green,
                                        current_color.blue,
                                        current_color.alpha,
                                    ),
                                    size: PREVIEW_CHECKERBOARD_SIZE,
                                    border_radius: BORDER_RADIUS,
                                })),
                                Node {
                                    position_type: PositionType::Absolute,
                                    width: percent(100.0),
                                    height: percent(100.0),
                                    ..default()
                                },
                            ));
                        });
                });

            // Input fields row
            parent
                .spawn((
                    ColorInputRow(picker_entity),
                    Node {
                        width: percent(100),
                        column_gap: px(6.0),
                        ..default()
                    },
                ))
                .with_children(|row| {
                    spawn_input_fields(row, picker_entity, state.input_mode, state);
                });
        });
    }
}

/// Despawn a color-picker popover once its owning picker entity is gone.
///
/// The popover is a root overlay (high z-index), not a child of the picker, so
/// it does not cascade-despawn when the picker's host (e.g. an inspector card)
/// is rebuilt. Left alone it lingers with stale references: clicks on its
/// controls resolve a dead picker entity and do nothing. This reaps those
/// orphans so a rebuilt host starts clean.
pub(super) fn despawn_orphaned_color_picker_popovers(
    mut commands: Commands,
    popovers: Query<(Entity, &ColorPickerPopover)>,
    pickers: Query<(), With<EditorColorPicker>>,
) {
    for (popover_entity, popover) in &popovers {
        if pickers.get(popover.0).is_err() {
            commands.entity(popover_entity).try_despawn();
        }
    }
}

/// Marker on the picker content's slider/swatch row so it can be reaped if it
/// orphans (it is a plain layout node with no other identifying component).
#[derive(Component)]
pub(super) struct ColorPickerContentRow;

/// Despawn color-picker UI that has been orphaned to the UI root. The inline
/// content and its direct children (the HSV rectangle, the slider/swatch row,
/// and the input-fields row) are always parented to a card, the picker, or a
/// popover; they are never legitimately a root node. If a rapid host rebuild
/// despawns the content while these children are mid-spawn, Bevy strips their
/// dangling `ChildOf` and each renders full-width at the window origin. Reaping
/// any that reach the root (despawning their subtree) removes that artifact.
pub(super) fn despawn_orphaned_color_picker_roots(
    mut commands: Commands,
    orphans: Query<
        Entity,
        (
            Without<ChildOf>,
            Or<(
                With<ColorPickerContent>,
                With<HsvRectangle>,
                With<ColorInputRow>,
                With<ColorPickerContentRow>,
            )>,
        ),
    >,
) {
    for entity in &orphans {
        commands.entity(entity).try_despawn();
    }
}
