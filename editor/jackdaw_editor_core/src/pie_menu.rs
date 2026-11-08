//! The Play menu, opened by the caret beside Play. It lists every launchable
//! instance, a row toggling each one, then the scene Play runs, Rebuild,
//! Restart and the separate-window choice. When the project has no
//! `jackdaw.toml`, a scaffold row stands in for the instances. Every row but
//! the scaffold one runs an operator. A per-frame system closes the popover
//! on Escape or an outside click.

use bevy::feathers::constants::size::CHECKBOX_SIZE;
use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use bevy::ui::ui_transform::UiGlobalTransform;
use bevy::ui_widgets::Activate;
use jackdaw_api::prelude::*;
use jackdaw_feathers::button::{ButtonOperatorCall, ButtonProps, ButtonVariant, button};
use jackdaw_feathers::icons::{EditorFont, Icon, IconFont};
use jackdaw_feathers::separator::{SeparatorProps, separator};
use jackdaw_feathers::tokens;

use crate::pie::{
    InstanceKey, PieRestartOp, PieSession, PieToggleInstanceOp, PieWindowMode,
    PieWindowModeToggleOp, ProjectBuildOp,
};
use crate::play_scene::PlayScene;
use crate::run_config::{CargoMeta, RunConfigs, read_run_configs, scaffold_manifest};

const MENU_MIN_WIDTH: f32 = 220.0;

pub struct PieMenuPlugin;

impl Plugin for PieMenuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PieMenuState>()
            .add_observer(on_menu_button_click)
            .add_observer(close_menu_on_item_activate)
            .add_observer(on_scaffold_row_click)
            .add_systems(Update, close_menu_on_outside_click);
    }
}

/// Marker on the caret button that opens the run-config dropdown. Sits
/// in the Play/Pause pill right after the Play button.
#[derive(Component)]
pub struct PieMenuButton;

/// Marker on the open dropdown root.
#[derive(Component)]
pub struct PieMenuPopover;

/// Marker on every menu row that runs an operator; activating one closes the
/// menu.
#[derive(Component)]
pub struct PieMenuItem;

/// Marker on the fallback row shown when the project has no
/// `jackdaw.toml`; clicking it scaffolds one.
#[derive(Component)]
pub struct PieScaffoldRow;

#[derive(Resource, Default)]
pub struct PieMenuState {
    pub popover_entity: Option<Entity>,
}

fn on_menu_button_click(
    mut click: On<PointerClick>,
    buttons: Query<(Entity, &ComputedNode, &UiGlobalTransform), With<PieMenuButton>>,
    parents: Query<&ChildOf>,
    mut state: ResMut<PieMenuState>,
    mut commands: Commands,
) {
    if click.event().button != PointerButton::Primary {
        return;
    }
    let Some(button) = find_ancestor_with(click.event_target(), &parents, |e| buttons.contains(e))
    else {
        return;
    };
    click.propagate(false);

    if let Some(popover) = state.popover_entity.take() {
        if let Ok(mut ec) = commands.get_entity(popover) {
            ec.despawn();
        }
        return;
    }

    let Ok((_, computed, global_tf)) = buttons.get(button) else {
        return;
    };
    let (_, _, pos) = global_tf.to_scale_angle_translation();
    let size = computed.size() * computed.inverse_scale_factor();
    let right = pos.x + size.x / 2.0;
    let top = pos.y + size.y / 2.0 + 4.0;

    commands.queue(move |world: &mut World| {
        let popover = spawn_menu(world, right, top);
        world.resource_mut::<PieMenuState>().popover_entity = Some(popover);
    });
}

/// Row data snapshotted before spawning so `spawn_menu` can take `&mut World`
/// without keeping the session and config borrows alive.
struct RowSpec {
    key: InstanceKey,
    label: String,
    running: bool,
}

fn spawn_menu(world: &mut World, right_x: f32, top_y: f32) -> Entity {
    let editor_font = world.get_resource::<EditorFont>().map(|f| f.0.clone());
    let icon_font = world.get_resource::<IconFont>().map(|f| f.0.clone());

    let rows = collect_rows(world);

    let popover = world
        .spawn((
            PieMenuPopover,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(top_y),
                left: Val::Px(right_x - MENU_MIN_WIDTH),
                min_width: Val::Px(MENU_MIN_WIDTH),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(tokens::SPACING_XS)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                row_gap: Val::Px(2.0),
                ..Default::default()
            },
            BackgroundColor(tokens::MENU_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            ZIndex(1000),
        ))
        .id();

    if rows.is_empty() {
        spawn_scaffold_row(world, popover, editor_font.clone(), icon_font);
    } else {
        for row in rows {
            spawn_instance_row(world, popover, row);
        }
    }

    world.spawn((separator(SeparatorProps::horizontal()), ChildOf(popover)));
    let scene = format!("Scene: {}", scene_played(world));
    spawn_note(world, popover, scene, editor_font);
    spawn_action_row(world, popover, "Rebuild", ProjectBuildOp::ID);
    spawn_action_row(world, popover, "Restart", PieRestartOp::ID);
    world.spawn((separator(SeparatorProps::horizontal()), ChildOf(popover)));
    let windowed = world.get_resource::<PieWindowMode>().copied() == Some(PieWindowMode::Windowed);
    spawn_check_row(
        world,
        popover,
        "Play in separate window",
        windowed,
        ButtonOperatorCall::new(PieWindowModeToggleOp::ID),
    );

    popover
}

/// The scene the first run configuration would play now, as the menu names it.
fn scene_played(world: &World) -> String {
    let launch = world
        .get_resource::<RunConfigs>()
        .and_then(|configs| configs.manifest.runs.first())
        .and_then(|run| crate::play_scene::launch_scene(world, run));
    match launch.map(|launch| launch.scene) {
        Some(PlayScene::Path(path) | PlayScene::Untitled(path)) => path,
        None => crate::pie::MAIN_SCENE.to_string(),
    }
}

/// A line of caption text in the menu, lined up with the rows' captions.
fn spawn_note(world: &mut World, popover: Entity, text: String, editor_font: Option<Handle<Font>>) {
    let mut font = TextFont {
        font_size: tokens::TEXT_SIZE_SM,
        ..Default::default()
    };
    if let Some(handle) = editor_font {
        font.font = handle.into();
    }
    let line = world
        .spawn((
            Node {
                padding: UiRect::axes(Val::Px(6.0), Val::Px(tokens::SPACING_XS)),
                column_gap: Val::Px(6.0),
                align_items: AlignItems::Center,
                ..Default::default()
            },
            ChildOf(popover),
        ))
        .id();
    world.spawn((
        Node {
            width: CHECKBOX_SIZE,
            flex_shrink: 0.0,
            ..Default::default()
        },
        ChildOf(line),
    ));
    world.spawn((
        Text::new(text),
        font,
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(line),
    ));
}

/// A row running `op_id`, its caption lined up with the checkbox rows'.
fn spawn_action_row(world: &mut World, popover: Entity, label: &str, op_id: &'static str) {
    world.spawn((
        PieMenuItem,
        button(
            ButtonProps::new(label)
                .with_variant(ButtonVariant::Ghost)
                .align_left()
                .reserving_left_icon()
                .call_operator(op_id),
        ),
        ChildOf(popover),
    ));
}

/// A row with a checkbox showing `checked`, running `call` when clicked.
fn spawn_check_row(
    world: &mut World,
    popover: Entity,
    label: &str,
    checked: bool,
    call: ButtonOperatorCall,
) {
    world.spawn((
        PieMenuItem,
        call,
        button(
            ButtonProps::new(label)
                .with_variant(ButtonVariant::Ghost)
                .align_left()
                .with_left_checkbox(checked),
        ),
        ChildOf(popover),
    ));
}

/// Collect one `RowSpec` per launchable instance. Single-instance configs
/// drop the `#1` suffix in the label; the key always carries the index.
fn collect_rows(world: &mut World) -> Vec<RowSpec> {
    let runs = world.resource::<RunConfigs>().manifest.runs.clone();
    let session = world.non_send::<PieSession>();

    let mut rows = Vec::new();
    for run in &runs {
        let label = run.label().to_string();
        for instance in 1..=run.instances {
            let key = InstanceKey {
                config: label.clone(),
                instance,
            };
            let display = if run.instances == 1 {
                label.clone()
            } else {
                key.to_string()
            };
            let running = session.is_running(&key);
            rows.push(RowSpec {
                key,
                label: display,
                running,
            });
        }
    }
    rows
}

fn spawn_instance_row(world: &mut World, popover: Entity, spec: RowSpec) {
    let call = ButtonOperatorCall::new(PieToggleInstanceOp::ID)
        .with_param("config", spec.key.config.clone())
        .with_param("instance", i64::from(spec.key.instance));
    spawn_check_row(world, popover, &spec.label, spec.running, call);
}

fn spawn_scaffold_row(
    world: &mut World,
    popover: Entity,
    editor_font: Option<Handle<Font>>,
    icon_font: Option<Handle<Font>>,
) {
    let row = world
        .spawn((
            PieScaffoldRow,
            button(
                ButtonProps::new("")
                    .with_variant(ButtonVariant::Ghost)
                    .align_left(),
            ),
            ChildOf(popover),
        ))
        .id();

    if let Some(handle) = icon_font {
        world.spawn((
            Text::new(String::from(Icon::FilePlus.unicode())),
            TextFont {
                font: handle.into(),
                font_size: tokens::TEXT_SIZE,
                ..Default::default()
            },
            TextColor(tokens::DOC_TAB_INACTIVE_LABEL),
            Pickable::IGNORE,
            ChildOf(row),
        ));
    }

    let mut label_font = TextFont {
        font_size: tokens::TEXT_SIZE_SM,
        ..Default::default()
    };
    if let Some(handle) = editor_font {
        label_font.font = handle.into();
    }
    world.spawn((
        Text::new("Generate jackdaw.toml".to_string()),
        label_font,
        TextColor(tokens::DOC_TAB_INACTIVE_LABEL),
        Pickable::IGNORE,
        ChildOf(row),
    ));
}

/// Close the menu once one of its rows has run its operator.
fn close_menu_on_item_activate(
    activate: On<Activate>,
    items: Query<(), With<PieMenuItem>>,
    mut commands: Commands,
) {
    if items.contains(activate.entity) {
        commands.queue(close_menu);
    }
}

fn on_scaffold_row_click(
    mut click: On<PointerClick>,
    rows: Query<&PieScaffoldRow>,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) {
    if click.event().button != PointerButton::Primary {
        return;
    }
    if find_ancestor_with(click.event_target(), &parents, |e| rows.contains(e)).is_none() {
        return;
    }
    click.propagate(false);

    commands.queue(|world: &mut World| {
        write_scaffold(world);
        read_run_configs(world);
        close_menu(world);
    });
}

/// Write a starter `jackdaw.toml` to the open project's root. Logs and
/// bails on a missing project or an io error rather than failing the
/// frame. Never overwrites: an existing file holds the user's plugin,
/// package, pins, and run configurations.
fn write_scaffold(world: &World) {
    let Some(root) = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(|p| p.root.clone())
    else {
        warn!("PIE: scaffold requested but no project is open");
        return;
    };
    let path = root.join("jackdaw.toml");
    if path.is_file() {
        warn!("PIE: {} already exists; leaving it alone", path.display());
        return;
    }
    let Some(meta) = CargoMeta::load(&root) else {
        warn!("PIE: cargo metadata failed for {}", root.display());
        return;
    };
    let body = scaffold_manifest(&meta);
    if let Err(err) = std::fs::write(&path, body) {
        warn!("PIE: failed to write {}: {err}", path.display());
    }
}

fn close_menu(world: &mut World) {
    if let Some(popover) = world.resource_mut::<PieMenuState>().popover_entity.take() {
        world.despawn(popover);
    }
}

fn close_menu_on_outside_click(
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<PieMenuState>,
    popovers: Query<&ComputedNode, With<PieMenuPopover>>,
    popover_transforms: Query<&UiGlobalTransform, With<PieMenuPopover>>,
    windows: Query<&Window>,
    focus: crate::keybind_focus::KeybindFocus,
    mut commands: Commands,
) {
    let Some(popover_entity) = state.popover_entity else {
        return;
    };
    if focus.keyboard_is_spoken_for() {
        return;
    }
    if !mouse.just_pressed(MouseButton::Left) && !keyboard.just_pressed(KeyCode::Escape) {
        return;
    }
    if keyboard.just_pressed(KeyCode::Escape) {
        if let Ok(mut ec) = commands.get_entity(popover_entity) {
            ec.despawn();
        }
        state.popover_entity = None;
        return;
    }

    let cursor = windows
        .single()
        .ok()
        .and_then(bevy::prelude::Window::cursor_position);
    if let (Some(cursor), Ok(computed), Ok(global_tf)) = (
        cursor,
        popovers.get(popover_entity),
        popover_transforms.get(popover_entity),
    ) {
        let (_, _, pos) = global_tf.to_scale_angle_translation();
        let size = computed.size() * computed.inverse_scale_factor();
        let min = pos - size * 0.5;
        let max = pos + size * 0.5;
        if cursor.x >= min.x && cursor.x <= max.x && cursor.y >= min.y && cursor.y <= max.y {
            return;
        }
    }

    if let Ok(mut ec) = commands.get_entity(popover_entity) {
        ec.despawn();
    }
    state.popover_entity = None;
}

fn find_ancestor_with<F>(start: Entity, parents: &Query<&ChildOf>, predicate: F) -> Option<Entity>
where
    F: Fn(Entity) -> bool,
{
    let mut entity = start;
    for _ in 0..8 {
        if predicate(entity) {
            return Some(entity);
        }
        if let Ok(co) = parents.get(entity) {
            entity = co.parent();
        } else {
            return None;
        }
    }
    None
}
