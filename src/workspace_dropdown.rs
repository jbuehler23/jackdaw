//! Workspace switcher dropdown. The trigger sits in the right side
//! of the window title bar next to the Play/Pause pill; it shows the
//! current workspace name and a chevron. Clicking it opens a popover
//! listing every workspace plus a "+ New Workspace" item.
//!
//! Each popover row is itself a [`WorkspaceTab`] entity, so
//! `handle_workspace_tab_double_click` (double-click renames) in
//! `jackdaw_panels` keeps working without duplication. The rows are
//! feathers buttons, so switching and adding run off their click event
//! rather than the dock strip's `Interaction` systems.
//!
//! Workspace deletion lives in `jackdaw_panels` but is intentionally
//! NOT surfaced here; per user request, workspaces are views you
//! switch between, not files you close.

use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use bevy::ui::ui_transform::UiGlobalTransform;
use jackdaw_feathers::button::{ButtonClickEvent, ButtonProps, ButtonVariant, button};
use jackdaw_feathers::icons::{EditorFont, Icon, IconFont};
use jackdaw_feathers::tokens;
use jackdaw_localization::LocalizedText;
use jackdaw_panels::tree::DockTree;
use jackdaw_panels::workspace::{WorkspaceChanged, WorkspaceRegistry, WorkspaceTab};
use jackdaw_panels::workspace_tabs::{AddWorkspaceButton, WorkspaceTabLabel, add_workspace};

const POPOVER_MIN_WIDTH: f32 = 200.0;

pub struct WorkspaceDropdownPlugin;

impl Plugin for WorkspaceDropdownPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorkspaceDropdownState>()
            .add_observer(on_trigger_click)
            .add_observer(on_workspace_row_click)
            .add_observer(on_add_workspace_row_click)
            .add_observer(on_workspace_changed_close_popover)
            .add_systems(
                Update,
                (update_trigger_label, close_popover_on_outside_click),
            );
    }
}

/// Marker on the trigger button (sits in the window title bar).
#[derive(Component)]
pub struct WorkspaceDropdownTrigger;

/// Marker on the trigger's inner label text, so the per-frame
/// `update_trigger_label` system can find and rewrite it when the
/// active workspace changes.
#[derive(Component)]
pub struct WorkspaceDropdownTriggerLabel;

/// Marker on the spawned popover root.
#[derive(Component)]
pub struct WorkspaceDropdownPopover;

#[derive(Resource, Default)]
pub struct WorkspaceDropdownState {
    pub popover_entity: Option<Entity>,
}

/// Title bar trigger bundle. Mount inside the right-hand group of
/// `window_title_bar` next to the Play/Pause pill.
pub fn workspace_dropdown_trigger(
    editor_font: Handle<Font>,
    icon_font: Handle<Font>,
) -> impl Bundle {
    // The trigger sits in a slot that holds the title bar's minimum
    // width for it; the button itself fills the slot.
    (
        Node {
            min_width: Val::Px(120.0),
            ..Default::default()
        },
        children![(
            WorkspaceDropdownTrigger,
            button(ButtonProps::new("").align_left()),
            children![
                (
                    WorkspaceDropdownTriggerLabel,
                    Text::default(),
                    TextFont {
                        font: editor_font.into(),
                        font_size: tokens::TEXT_SIZE_SM,
                        ..Default::default()
                    },
                    TextColor(tokens::TEXT_PRIMARY),
                    Pickable::IGNORE,
                ),
                (
                    Node {
                        flex_grow: 1.0,
                        ..Default::default()
                    },
                    Pickable::IGNORE,
                ),
                (
                    Text::new(String::from(Icon::ChevronDown.unicode())),
                    TextFont {
                        font: icon_font.into(),
                        font_size: tokens::TEXT_SIZE_XS,
                        ..Default::default()
                    },
                    TextColor(tokens::TEXT_SECONDARY),
                    Pickable::IGNORE,
                ),
            ],
        )],
    )
}

fn update_trigger_label(
    registry: Res<WorkspaceRegistry>,
    triggers: Query<&Children, With<WorkspaceDropdownTrigger>>,
    mut labels: Query<&mut Text, With<WorkspaceDropdownTriggerLabel>>,
) {
    if !registry.is_changed() {
        return;
    }
    let name = registry
        .active_workspace()
        .map(|w| w.name.clone())
        .unwrap_or_default();
    for children in triggers.iter() {
        for child in children.iter() {
            if let Ok(mut text) = labels.get_mut(child)
                && text.0 != name
            {
                text.0 = name.clone();
            }
        }
    }
}

fn on_trigger_click(
    mut click: On<Pointer<Click>>,
    triggers: Query<(Entity, &ComputedNode, &UiGlobalTransform), With<WorkspaceDropdownTrigger>>,
    parents: Query<&ChildOf>,
    mut state: ResMut<WorkspaceDropdownState>,
    mut commands: Commands,
) {
    if click.event().button != PointerButton::Primary {
        return;
    }
    let Some(trigger) =
        find_ancestor_with(click.event_target(), &parents, |e| triggers.contains(e))
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

    let Ok((_, computed, global_tf)) = triggers.get(trigger) else {
        return;
    };
    let (_, _, pos) = global_tf.to_scale_angle_translation();
    let size = computed.size() * computed.inverse_scale_factor();
    let right = pos.x + size.x / 2.0;
    let top = pos.y + size.y / 2.0 + 4.0;

    commands.queue(move |world: &mut World| {
        let popover = spawn_popover(world, right, top);
        world
            .resource_mut::<WorkspaceDropdownState>()
            .popover_entity = Some(popover);
    });
}

fn spawn_popover(world: &mut World, right_x: f32, top_y: f32) -> Entity {
    let editor_font = world.get_resource::<EditorFont>().map(|f| f.0.clone());
    let icon_font = world.get_resource::<IconFont>().map(|f| f.0.clone());

    let registry_snapshot: Vec<(String, String, Color, Option<String>)> = world
        .resource::<WorkspaceRegistry>()
        .iter()
        .map(|w| (w.id.clone(), w.name.clone(), w.accent_color, w.icon.clone()))
        .collect();
    let active_id = world.resource::<WorkspaceRegistry>().active.clone();

    let popover = world
        .spawn((
            WorkspaceDropdownPopover,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(top_y),
                left: Val::Px(right_x - POPOVER_MIN_WIDTH),
                min_width: Val::Px(POPOVER_MIN_WIDTH),
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

    for (id, name, accent, icon) in &registry_snapshot {
        spawn_popover_row(
            world,
            popover,
            id,
            name,
            *accent,
            icon.as_deref(),
            active_id.as_deref() == Some(id),
            editor_font.clone(),
            icon_font.clone(),
        );
    }

    spawn_popover_separator(world, popover);
    spawn_popover_add_row(world, popover, editor_font, icon_font);

    popover
}

fn spawn_popover_row(
    world: &mut World,
    popover: Entity,
    workspace_id: &str,
    name: &str,
    accent: Color,
    icon_glyph: Option<&str>,
    is_active: bool,
    editor_font: Option<Handle<Font>>,
    icon_font: Option<Handle<Font>>,
) {
    let row_variant = if is_active {
        ButtonVariant::Active
    } else {
        ButtonVariant::Ghost
    };
    let label_color = if is_active {
        tokens::DOC_TAB_ACTIVE_LABEL
    } else {
        tokens::DOC_TAB_INACTIVE_LABEL
    };

    let row = world
        .spawn((
            WorkspaceTab {
                workspace_id: workspace_id.to_string(),
            },
            button(ButtonProps::new("").with_variant(row_variant).align_left()),
            ChildOf(popover),
        ))
        .id();

    // Accent stripe.
    world.spawn((
        Node {
            width: Val::Px(2.5),
            height: Val::Px(12.0),
            border_radius: BorderRadius::all(Val::Px(5.0)),
            ..Default::default()
        },
        BackgroundColor(accent),
        Pickable::IGNORE,
        ChildOf(row),
    ));

    // Workspace icon (optional).
    if let (Some(glyph), Some(handle)) = (icon_glyph, icon_font) {
        world.spawn((
            Text::new(glyph.to_string()),
            TextFont {
                font: handle.into(),
                font_size: tokens::TEXT_SIZE,
                ..Default::default()
            },
            TextColor(label_color),
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
        WorkspaceTabLabel {
            workspace_id: workspace_id.to_string(),
        },
        Text::new(name.to_string()),
        label_font,
        TextColor(label_color),
        Pickable::IGNORE,
        ChildOf(row),
    ));
}

fn spawn_popover_separator(world: &mut World, popover: Entity) {
    world.spawn((
        Node {
            height: Val::Px(1.0),
            margin: UiRect::vertical(Val::Px(4.0)),
            ..Default::default()
        },
        BackgroundColor(tokens::BORDER_SUBTLE),
        Pickable::IGNORE,
        ChildOf(popover),
    ));
}

fn spawn_popover_add_row(
    world: &mut World,
    popover: Entity,
    editor_font: Option<Handle<Font>>,
    icon_font: Option<Handle<Font>>,
) {
    let row = world
        .spawn((
            AddWorkspaceButton,
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
            Text::new(String::from(Icon::Plus.unicode())),
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
        LocalizedText::new("new-workspace"),
        label_font,
        TextColor(tokens::DOC_TAB_INACTIVE_LABEL),
        Pickable::IGNORE,
        ChildOf(row),
    ));
}

fn on_workspace_changed_close_popover(
    _: On<WorkspaceChanged>,
    mut state: ResMut<WorkspaceDropdownState>,
    mut commands: Commands,
) {
    if let Some(popover) = state.popover_entity.take()
        && let Ok(mut ec) = commands.get_entity(popover)
    {
        ec.despawn();
    }
}

fn close_popover_on_outside_click(
    mouse: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mut state: ResMut<WorkspaceDropdownState>,
    popovers: Query<&ComputedNode, With<WorkspaceDropdownPopover>>,
    popover_transforms: Query<&UiGlobalTransform, With<WorkspaceDropdownPopover>>,
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

    // Left-click: if the click happened inside the popover's bounding box,
    // ignore so item observers / rename activation can run.
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

/// Switch to the workspace a popover row names.
///
/// `WorkspaceChanged`'s observer is the sole owner of
/// `registry.active`; setting it here would race with auto-save.
fn on_workspace_row_click(
    click: On<ButtonClickEvent>,
    rows: Query<&WorkspaceTab>,
    registry: Res<WorkspaceRegistry>,
    mut commands: Commands,
) {
    let Ok(row) = rows.get(click.entity) else {
        return;
    };
    if registry.active.as_ref() == Some(&row.workspace_id) {
        return;
    }
    commands.trigger(WorkspaceChanged {
        old: registry.active.clone(),
        new: row.workspace_id.clone(),
    });
}

/// Add a workspace from the popover's last row.
fn on_add_workspace_row_click(
    click: On<ButtonClickEvent>,
    rows: Query<(), With<AddWorkspaceButton>>,
    mut registry: ResMut<WorkspaceRegistry>,
    tree: Res<DockTree>,
    mut commands: Commands,
) {
    if rows.contains(click.entity) {
        add_workspace(&mut registry, &tree, &mut commands);
    }
}
