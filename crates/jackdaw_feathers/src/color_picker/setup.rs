use bevy::feathers::controls::{
    ColorChannel, ColorPlaneValue, ColorSwatchValue, FeathersColorPlane, FeathersColorSlider,
    FeathersColorSwatch, SliderBaseColor,
};
use bevy::prelude::*;

use super::input_fields::spawn_input_fields;
use super::{
    COLOR_PLANE_HEIGHT, ColorInputRow, ColorPart, ColorPickerConfig, ColorPickerContent,
    ColorPickerPopover, ColorPickerState, ColorPickerTrigger, ColorSubWidget, EditorColorPicker,
    POPOVER_WIDTH, PREVIEW_SWATCH_SIZE, SWATCH_SIZE, TriggerLabel, TriggerSwatch,
    TriggerSwatchConfig,
};

use crate::button::{
    ButtonClickEvent, ButtonContentText, ButtonProps, ButtonVariant, button, button_caption,
};
use crate::icons::{EditorFont, IconFont};
use crate::popover::{
    PopoverHeaderProps, PopoverPlacement, PopoverProps, PopoverTracker, activate_trigger,
    deactivate_trigger, popover, popover_content, popover_header,
};

pub(super) fn setup_color_picker(
    add: On<Add<EditorColorPicker>>,
    mut commands: Commands,
    pickers: Query<(&ColorPickerConfig, &ColorPickerState)>,
) {
    let entity = add.entity;
    if let Ok((config, state)) = pickers.get(entity) {
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

/// Put the swatch on the trigger and mark its caption as the hex readout. The
/// config is inserted after the trigger is spawned, by which time the button
/// has built its caption.
pub(super) fn setup_trigger_swatch(
    add: On<Add<TriggerSwatchConfig>>,
    mut commands: Commands,
    triggers: Query<&TriggerSwatchConfig>,
    children: Query<&Children>,
    captions: Query<(), With<ButtonContentText>>,
) {
    let trigger_entity = add.entity;
    if let Ok(config) = triggers.get(trigger_entity) {
        let Some(caption) = button_caption(trigger_entity, &children, &captions) else {
            return;
        };

        commands
            .entity(trigger_entity)
            .remove::<TriggerSwatchConfig>();

        commands.spawn_scene(bsn! { @FeathersColorSwatch }).insert((
            TriggerSwatch,
            ColorSwatchValue(Color::Srgba(config.color)),
            ColorSubWidget {
                picker: config.picker,
                part: ColorPart::Whole,
            },
            Node {
                position_type: PositionType::Absolute,
                left: px(6.0),
                width: px(SWATCH_SIZE),
                height: px(SWATCH_SIZE),
                ..default()
            },
            ChildOf(trigger_entity),
        ));

        commands.entity(caption).insert((
            TriggerLabel(config.picker),
            Node {
                margin: UiRect::left(px(SWATCH_SIZE + 6.0)),
                ..default()
            },
        ));
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
    mut button_styles: Query<&mut ButtonVariant>,
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
    add: On<Add<ColorPickerContent>>,
    mut commands: Commands,
    states: Query<&ColorPickerState>,
    contents: Query<&ColorPickerContent>,
) {
    let content_entity = add.entity;
    if let Ok(content) = contents.get(content_entity) {
        let picker = content.0;
        let Ok(state) = states.get(picker) else {
            return;
        };
        let rgba = state.to_rgba();
        let color = Color::Srgba(state.to_srgba());

        // The plane carries red on its x axis and blue on its y, with
        // green as the fixed channel behind the gradient.
        commands
            .spawn_scene(bsn! { @FeathersColorPlane::RedBlue })
            .insert((
                ColorPlaneValue(Vec3::new(rgba[0], rgba[2], rgba[1])),
                ColorSubWidget {
                    picker,
                    part: ColorPart::Whole,
                },
                Node {
                    width: percent(100.0),
                    height: px(COLOR_PLANE_HEIGHT),
                    ..default()
                },
                ChildOf(content_entity),
            ));

        let row = commands
            .spawn((
                ColorPickerContentRow,
                Node {
                    column_gap: px(12.0),
                    align_items: AlignItems::Center,
                    ..default()
                },
                ChildOf(content_entity),
            ))
            .id();

        let slider_column = commands
            .spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: px(6.0),
                    flex_grow: 1.0,
                    ..default()
                },
                ChildOf(row),
            ))
            .id();

        for (part, channel) in [
            (ColorPart::Red, ColorChannel::Red),
            (ColorPart::Green, ColorChannel::Green),
            (ColorPart::Blue, ColorChannel::Blue),
            (ColorPart::Alpha, ColorChannel::Alpha),
        ] {
            let value = rgba[part.channel().expect("a channel part has an index")];
            commands
                .spawn_scene(bsn! {
                    @FeathersColorSlider {
                        @value: {value},
                        @channel: {channel},
                    }
                })
                .insert((
                    SliderBaseColor(color),
                    ColorSubWidget { picker, part },
                    ChildOf(slider_column),
                ));
        }

        commands.spawn_scene(bsn! { @FeathersColorSwatch }).insert((
            ColorSwatchValue(color),
            ColorSubWidget {
                picker,
                part: ColorPart::Whole,
            },
            Node {
                width: px(PREVIEW_SWATCH_SIZE),
                height: px(PREVIEW_SWATCH_SIZE),
                ..default()
            },
            ChildOf(row),
        ));

        let input_row = commands
            .spawn((
                ColorInputRow(picker),
                Node {
                    width: percent(100),
                    column_gap: px(6.0),
                    ..default()
                },
                ChildOf(content_entity),
            ))
            .id();
        let input_mode = state.input_mode;
        commands.entity(input_row).with_children(|row| {
            spawn_input_fields(row, picker, input_mode, state);
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
/// content and its direct children (the colour plane, the slider/swatch row,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color_picker::visuals::update_trigger_display;
    use crate::color_picker::{ColorPickerProps, color_picker};
    use crate::icons::{EditorFont, IconFont};

    /// An app with the picker's setup passes. The render-side material
    /// plugins the widget's own plugin adds are not needed to read a
    /// label, so the passes under test are registered by hand.
    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
            crate::button::plugin,
        ));
        app.init_asset::<bevy::text::Font>();
        app.init_resource::<bevy::input_focus::InputFocus>();
        app.insert_resource(IconFont(Handle::default()));
        app.insert_resource(EditorFont(Handle::default()));
        app.add_observer(setup_color_picker)
            .add_observer(setup_trigger_swatch)
            .add_observer(setup_color_picker_content)
            .add_systems(Update, update_trigger_display);
        app
    }

    /// A picker with its trigger built.
    fn app_with_picker(color: [f32; 4]) -> (App, Entity) {
        let mut app = app();
        let entity = app
            .world_mut()
            .spawn(color_picker(ColorPickerProps::new().with_color(color)))
            .id();
        app.world_mut().flush();
        (app, entity)
    }

    /// The hex the trigger is drawing, found by the label marker the setup
    /// pass is supposed to have put on the button's caption.
    fn readout(app: &App) -> String {
        app.world()
            .iter_entities()
            .find(EntityRef::contains::<TriggerLabel>)
            .expect("the trigger's caption carries the label marker")
            .get::<Text>()
            .expect("the label is the caption text")
            .0
            .clone()
    }

    /// The readout follows the colour. It only does so if the setup pass
    /// finds the caption inside its clipping slot rather than among the
    /// button's own children; without the marker the hex stays at the
    /// colour it was spawned with.
    #[test]
    fn the_trigger_reads_the_colour_it_is_recoloured_to() {
        let (mut app, picker) = app_with_picker([1.0, 0.0, 0.0, 1.0]);
        assert_eq!(readout(&app), "FF0000");

        app.world_mut()
            .get_mut::<ColorPickerState>(picker)
            .expect("the picker holds its state")
            .set_from_rgba([0.0, 0.0, 1.0, 1.0]);
        app.update();

        assert_eq!(readout(&app), "0000FF");
    }

    /// The trigger has its swatch and readout on the frame the picker is
    /// spawned, before any update has run.
    #[test]
    fn a_picker_trigger_is_complete_on_the_frame_it_spawns() {
        let (app, picker) = app_with_picker([0.0, 1.0, 0.0, 1.0]);
        assert_eq!(readout(&app), "00FF00");
        let swatch = app
            .world()
            .iter_entities()
            .find(EntityRef::contains::<TriggerSwatch>)
            .expect("the trigger has its swatch");
        let trigger = swatch.get::<ChildOf>().expect("on the trigger").parent();
        assert_eq!(
            app.world().get::<ChildOf>(trigger).map(ChildOf::parent),
            Some(picker),
        );
    }

    /// An inline picker shows its plane, sliders and fields on the frame it
    /// is spawned.
    #[test]
    fn an_inline_picker_is_complete_on_the_frame_it_spawns() {
        let mut app = app();
        let picker = app
            .world_mut()
            .spawn(color_picker(ColorPickerProps::new().inline()))
            .id();
        app.world_mut().flush();

        let world = app.world_mut();
        let mut parts = world.query::<&ColorSubWidget>();
        let parts: Vec<Option<usize>> = parts
            .iter(world)
            .filter(|part| part.picker == picker)
            .map(|part| part.part.channel())
            .collect();
        assert!(parts.contains(&None), "the plane and swatch are there");
        for channel in 0..4 {
            assert!(parts.contains(&Some(channel)), "slider {channel} is there");
        }
        let mut rows = world.query::<&ColorInputRow>();
        assert_eq!(
            rows.iter(world).filter(|row| row.0 == picker).count(),
            1,
            "and so is the row of fields",
        );
    }

    /// The readout keeps clear of the swatch drawn over the button's left
    /// edge.
    #[test]
    fn the_readout_is_moved_clear_of_the_swatch() {
        let (app, _) = app_with_picker([1.0, 0.0, 0.0, 1.0]);
        let labelled = app
            .world()
            .iter_entities()
            .find(EntityRef::contains::<TriggerLabel>)
            .expect("the trigger's caption carries the label marker");
        let node = labelled.get::<Node>().expect("the caption is laid out");
        assert_eq!(node.margin.left, px(SWATCH_SIZE + 6.0));
    }
}
