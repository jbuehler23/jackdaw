//! Play-button run-config dropdown. The caret opens a list of every
//! launchable instance; clicking a row toggles launch/stop. When the
//! project has no `jackdaw.toml`, a single scaffold row is shown instead.
//! A per-frame system closes the popover on Escape or an outside click.

use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use bevy::ui::ui_transform::UiGlobalTransform;
use jackdaw_feathers::icons::{EditorFont, Icon, IconFont};
use jackdaw_feathers::tokens;

use crate::pie::{InstanceKey, PieSession, launch_instance, stop_instance};
use crate::run_config::{CargoMeta, RunConfigs, read_run_configs, scaffold_manifest};

const MENU_MIN_WIDTH: f32 = 220.0;
const ROW_HEIGHT: f32 = 26.0;
const CHECK_SLOT_WIDTH: f32 = 16.0;

pub struct PieMenuPlugin;

impl Plugin for PieMenuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PieMenuState>()
            .add_observer(on_menu_button_click)
            .add_observer(on_instance_row_click)
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

/// Marker on one instance row, carrying the instance it launches or
/// stops when clicked.
#[derive(Component)]
pub struct PieInstanceRow(pub InstanceKey);

/// Marker on the fallback row shown when the project has no
/// `jackdaw.toml`; clicking it scaffolds one.
#[derive(Component)]
pub struct PieScaffoldRow;

#[derive(Resource, Default)]
pub struct PieMenuState {
    pub popover_entity: Option<Entity>,
}

fn on_menu_button_click(
    mut click: On<Pointer<Click>>,
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
        spawn_scaffold_row(world, popover, editor_font, icon_font);
    } else {
        for row in rows {
            spawn_instance_row(world, popover, row, editor_font.clone(), icon_font.clone());
        }
    }

    popover
}

/// Collect one `RowSpec` per launchable instance. Single-instance configs
/// drop the `#1` suffix in the label; the key always carries the index.
fn collect_rows(world: &mut World) -> Vec<RowSpec> {
    let runs = world.resource::<RunConfigs>().manifest.runs.clone();
    let session = world.non_send_resource::<PieSession>();

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

fn spawn_instance_row(
    world: &mut World,
    popover: Entity,
    spec: RowSpec,
    editor_font: Option<Handle<Font>>,
    icon_font: Option<Handle<Font>>,
) {
    let label_color = if spec.running {
        tokens::DOC_TAB_ACTIVE_LABEL
    } else {
        tokens::DOC_TAB_INACTIVE_LABEL
    };

    let row = world
        .spawn((
            PieInstanceRow(spec.key),
            Interaction::default(),
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(6.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(5.0)),
                height: Val::Px(ROW_HEIGHT),
                border_radius: BorderRadius::all(Val::Px(3.0)),
                ..Default::default()
            },
            BackgroundColor(Color::NONE),
            ChildOf(popover),
        ))
        .id();

    spawn_check_slot(world, row, spec.running, icon_font);

    let mut label_font = TextFont {
        font_size: tokens::FONT_SM,
        ..Default::default()
    };
    if let Some(handle) = editor_font {
        label_font.font = handle;
    }
    world.spawn((
        Text::new(spec.label),
        label_font,
        TextColor(label_color),
        Pickable::IGNORE,
        ChildOf(row),
    ));
}

/// Fixed-width slot showing a check glyph when running, or an empty
/// spacer so labels stay aligned across rows.
fn spawn_check_slot(
    world: &mut World,
    row: Entity,
    running: bool,
    icon_font: Option<Handle<Font>>,
) {
    let slot = world
        .spawn((
            Node {
                width: Val::Px(CHECK_SLOT_WIDTH),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..Default::default()
            },
            Pickable::IGNORE,
            ChildOf(row),
        ))
        .id();

    if let (true, Some(handle)) = (running, icon_font) {
        world.spawn((
            Text::new(String::from(Icon::Check.unicode())),
            TextFont {
                font: handle,
                font_size: 12.0,
                ..Default::default()
            },
            TextColor(tokens::DOC_TAB_ACTIVE_LABEL),
            Pickable::IGNORE,
            ChildOf(slot),
        ));
    }
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
            Interaction::default(),
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(8.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(5.0)),
                height: Val::Px(ROW_HEIGHT),
                border_radius: BorderRadius::all(Val::Px(3.0)),
                ..Default::default()
            },
            BackgroundColor(Color::NONE),
            ChildOf(popover),
        ))
        .id();

    if let Some(handle) = icon_font {
        world.spawn((
            Text::new(String::from(Icon::FilePlus.unicode())),
            TextFont {
                font: handle,
                font_size: 12.0,
                ..Default::default()
            },
            TextColor(tokens::DOC_TAB_INACTIVE_LABEL),
            Pickable::IGNORE,
            ChildOf(row),
        ));
    }

    let mut label_font = TextFont {
        font_size: tokens::FONT_SM,
        ..Default::default()
    };
    if let Some(handle) = editor_font {
        label_font.font = handle;
    }
    world.spawn((
        Text::new("Generate jackdaw.toml".to_string()),
        label_font,
        TextColor(tokens::DOC_TAB_INACTIVE_LABEL),
        Pickable::IGNORE,
        ChildOf(row),
    ));
}

fn on_instance_row_click(
    mut click: On<Pointer<Click>>,
    rows: Query<&PieInstanceRow>,
    parents: Query<&ChildOf>,
    mut commands: Commands,
) -> Result {
    if click.event().button != PointerButton::Primary {
        return Ok(());
    }
    let Some(row) = find_ancestor_with(click.event_target(), &parents, |e| rows.contains(e)) else {
        return Ok(());
    };
    click.propagate(false);

    let key = rows.get(row)?.0.clone();
    commands.queue(move |world: &mut World| {
        if world.non_send_resource::<PieSession>().is_running(&key) {
            stop_instance(world, &key);
        } else if let Some(run) = world
            .resource::<RunConfigs>()
            .manifest
            .run_by_name(&key.config)
            .cloned()
        {
            launch_instance(world, key.clone(), run);
        }
        close_menu(world);
    });
    Ok(())
}

fn on_scaffold_row_click(
    mut click: On<Pointer<Click>>,
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
/// frame.
fn write_scaffold(world: &World) {
    let Some(root) = world
        .get_resource::<crate::project::ProjectRoot>()
        .map(|p| p.root.clone())
    else {
        warn!("PIE: scaffold requested but no project is open");
        return;
    };
    let Some(meta) = CargoMeta::load(&root) else {
        warn!("PIE: cargo metadata failed for {}", root.display());
        return;
    };
    let body = scaffold_manifest(&meta);
    let path = root.join("jackdaw.toml");
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
    mut commands: Commands,
) {
    let Some(popover_entity) = state.popover_entity else {
        return;
    };
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
