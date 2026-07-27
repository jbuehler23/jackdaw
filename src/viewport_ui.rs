//! UI overlay adapter for ordinary 3D viewport windows.
//!
//! Authored canvases remain a single ECS source tree. Each viewport owns
//! derived projections targeting its private camera, so any number of 3D
//! viewports can preview, select, or interact with the same game UI.

use std::collections::HashSet;

use bevy::{
    ecs::{
        hierarchy::Children,
        lifecycle::Despawn,
        query::{Changed, Or, With, Without},
    },
    picking::events::{Click, Pointer},
    prelude::*,
    ui_widgets::Button,
};
use jackdaw_scene_types::SceneNodeId;
use jackdaw_ui::{
    UiButton, UiCanvas, UiCheckbox, UiSlider, UiStyleOverride, UiTextInput, UiThemeScope, UiToggle,
};

use crate::{
    EditorEntity,
    selection::Selection,
    ui_canvas::UiCanvasMode,
    ui_projection::{
        ProjectedFrom, UiProjection, UiProjectionHandle, UiProjectionRoot, UiProjectionSpec,
    },
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewportUiFilter {
    /// Preview only canvases enabled for the running game.
    #[default]
    RuntimeVisible,
    /// Preview disabled/authoring-only canvases too.
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewportCanvasProjection {
    pub canvas: Entity,
    pub handle: UiProjectionHandle,
}

/// Per-window UI overlay state stored on a 3D viewport's dock host.
#[derive(Component, Clone, Debug)]
pub struct ViewportUiHost {
    pub camera: Entity,
    pub mode: UiCanvasMode,
    pub filter: ViewportUiFilter,
    pub projections: Vec<ViewportCanvasProjection>,
}

#[derive(Component, Clone, Copy)]
struct ViewportEditingProjection {
    host: Entity,
}

#[derive(Component)]
struct ViewportUiModeButton {
    host: Entity,
}

#[derive(Component)]
struct ViewportUiFilterButton {
    host: Entity,
}

#[derive(Component)]
struct ViewportUiModeLabel {
    host: Entity,
}

#[derive(Component)]
struct ViewportUiFilterLabel {
    host: Entity,
}

pub struct ViewportUiPlugin;

impl Plugin for ViewportUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_viewport_ui_host_despawn)
            .add_observer(on_viewport_ui_mode_click)
            .add_observer(on_viewport_ui_filter_click)
            .add_observer(on_viewport_projected_click)
            .add_systems(Update, sync_viewport_ui)
            .add_systems(Update, update_toolbar_labels);
    }
}

/// Attach UI preview/editing to an existing viewport host and camera.
pub fn attach_viewport_ui(world: &mut World, host: Entity, camera: Entity) {
    world.entity_mut(host).insert(ViewportUiHost {
        camera,
        mode: UiCanvasMode::Scene,
        filter: ViewportUiFilter::RuntimeVisible,
        projections: Vec::new(),
    });
}

/// Spawn compact controls for one viewport UI adapter.
pub fn spawn_viewport_ui_toolbar(world: &mut World, parent: Entity, host: Entity) -> Entity {
    let toolbar = world
        .spawn((
            EditorEntity,
            Node {
                width: percent(100),
                height: px(26),
                flex_shrink: 0.0,
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(5),
                padding: UiRect::horizontal(px(6)),
                ..default()
            },
            BackgroundColor(Color::srgb(0.10, 0.11, 0.13)),
            ChildOf(parent),
        ))
        .id();
    spawn_toolbar_button(
        world,
        toolbar,
        ViewportUiModeButton { host },
        ViewportUiModeLabel { host },
        "UI: Scene",
    );
    spawn_toolbar_button(
        world,
        toolbar,
        ViewportUiFilterButton { host },
        ViewportUiFilterLabel { host },
        "Canvases: Runtime",
    );
    toolbar
}

fn spawn_toolbar_button<M: Component, L: Component>(
    world: &mut World,
    toolbar: Entity,
    marker: M,
    label_marker: L,
    label: &'static str,
) {
    let button = world
        .spawn((
            marker,
            EditorEntity,
            Button,
            Node {
                min_height: px(20),
                padding: UiRect::horizontal(px(6)),
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgb(0.17, 0.18, 0.21)),
            ChildOf(toolbar),
        ))
        .id();
    world.spawn((
        label_marker,
        EditorEntity,
        Text::new(label),
        TextFont {
            font_size: FontSize::Px(11.0),
            ..default()
        },
        TextColor(Color::srgb(0.84, 0.85, 0.89)),
        Pickable::IGNORE,
        ChildOf(button),
    ));
}

fn on_viewport_ui_host_despawn(
    event: On<Despawn, ViewportUiHost>,
    hosts: Query<&ViewportUiHost>,
    mut commands: Commands,
) {
    let Ok(host) = hosts.get(event.event_target()) else {
        return;
    };
    let handles = host
        .projections
        .iter()
        .map(|projection| projection.handle)
        .collect::<Vec<_>>();
    commands.queue(move |world: &mut World| {
        for handle in handles {
            UiProjection::close(world, handle);
        }
    });
}

fn on_viewport_ui_mode_click(
    event: On<Pointer<Click>>,
    buttons: Query<&ViewportUiModeButton>,
    mut hosts: Query<&mut ViewportUiHost>,
) {
    let Ok(button) = buttons.get(event.event_target()) else {
        return;
    };
    if let Ok(mut host) = hosts.get_mut(button.host) {
        host.mode = match host.mode {
            UiCanvasMode::Scene => UiCanvasMode::Ui,
            UiCanvasMode::Ui => UiCanvasMode::Interact,
            UiCanvasMode::Interact => UiCanvasMode::Scene,
        };
    }
}

fn on_viewport_ui_filter_click(
    event: On<Pointer<Click>>,
    buttons: Query<&ViewportUiFilterButton>,
    mut hosts: Query<&mut ViewportUiHost>,
) {
    let Ok(button) = buttons.get(event.event_target()) else {
        return;
    };
    if let Ok(mut host) = hosts.get_mut(button.host) {
        host.filter = match host.filter {
            ViewportUiFilter::RuntimeVisible => ViewportUiFilter::All,
            ViewportUiFilter::All => ViewportUiFilter::RuntimeVisible,
        };
    }
}

fn on_viewport_projected_click(
    mut event: On<Pointer<Click>>,
    projected: Query<&ProjectedFrom>,
    roots: Query<&ViewportEditingProjection, With<UiProjectionRoot>>,
    parents: Query<&ChildOf>,
    hosts: Query<&ViewportUiHost>,
    authored: Query<(Entity, &SceneNodeId), Without<ProjectedFrom>>,
    mut selection: ResMut<Selection>,
    mut commands: Commands,
) {
    let mut current = Some(event.event_target());
    let mut node = None;
    let mut host = None;
    while let Some(entity) = current {
        node = node.or_else(|| projected.get(entity).ok().map(|source| source.0));
        host = host.or_else(|| roots.get(entity).ok().map(|root| root.host));
        current = parents.get(entity).ok().map(ChildOf::parent);
    }
    let (Some(node), Some(host)) = (node, host) else {
        return;
    };
    if hosts
        .get(host)
        .is_ok_and(|host| host.mode == UiCanvasMode::Ui)
        && let Some(entity) = authored
            .iter()
            .find_map(|(entity, candidate)| (*candidate == node).then_some(entity))
    {
        selection.select_single(&mut commands, entity);
        event.propagate(false);
    }
}

fn sync_viewport_ui(world: &mut World) {
    let mut changed_query = world.query_filtered::<Entity, (
        Or<(
            Changed<UiCanvas>,
            Changed<UiButton>,
            Changed<UiCheckbox>,
            Changed<UiToggle>,
            Changed<UiSlider>,
            Changed<UiTextInput>,
            Changed<Node>,
            Changed<Children>,
            Changed<UiThemeScope>,
            Changed<UiStyleOverride>,
        )>,
        Without<ProjectedFrom>,
    )>();
    let changed_entities = changed_query.iter(world).collect::<Vec<_>>();
    let changed_canvases = changed_entities
        .into_iter()
        .filter_map(|entity| authored_canvas_ancestor(world, entity))
        .collect::<HashSet<_>>();

    let gesture_active = crate::ui_stage::manipulating(world);
    let canvases = authored_canvases(world);
    let mut query = world.query::<(Entity, &ViewportUiHost)>();
    let hosts = query
        .iter(world)
        .map(|(entity, host)| (entity, host.clone()))
        .collect::<Vec<_>>();

    for (host_entity, mut state) in hosts {
        let desired = canvases
            .iter()
            .filter(|(_, canvas)| state.filter == ViewportUiFilter::All || canvas.enabled)
            .map(|(entity, _)| *entity)
            .collect::<HashSet<_>>();

        state.projections.retain(|projection| {
            if desired.contains(&projection.canvas) {
                true
            } else {
                UiProjection::close(world, projection.handle);
                false
            }
        });

        for canvas in &desired {
            if let Some(projection) = state
                .projections
                .iter_mut()
                .find(|projection| projection.canvas == *canvas)
            {
                if !gesture_active
                    && changed_canvases.contains(canvas)
                    && UiProjection::refresh(world, projection.handle).is_err()
                {
                    continue;
                }
                mark_projection(world, projection.handle, host_entity);
            } else if let Ok(handle) = UiProjection::open(
                world,
                UiProjectionSpec {
                    canvas: *canvas,
                    target_camera: state.camera,
                },
            ) {
                mark_projection(world, handle, host_entity);
                state.projections.push(ViewportCanvasProjection {
                    canvas: *canvas,
                    handle,
                });
            }
        }
        apply_projection_pickability(world, &state);
        if let Some(mut current) = world.get_mut::<ViewportUiHost>(host_entity) {
            *current = state;
        }
    }
}

fn mark_projection(world: &mut World, handle: UiProjectionHandle, host: Entity) {
    if let Some(root) = UiProjection::root(world, handle) {
        world
            .entity_mut(root)
            .insert(ViewportEditingProjection { host });
    }
}

fn apply_projection_pickability(world: &mut World, state: &ViewportUiHost) {
    for projection in &state.projections {
        let Some(root) = UiProjection::root(world, projection.handle) else {
            continue;
        };
        let mut stack = vec![root];
        while let Some(entity) = stack.pop() {
            if let Some(children) = world.get::<Children>(entity) {
                stack.extend(children.iter());
            }
            if state.mode == UiCanvasMode::Scene {
                world.entity_mut(entity).insert(Pickable::IGNORE);
            } else {
                world.entity_mut(entity).remove::<Pickable>();
            }
        }
    }
}

fn authored_canvases(world: &mut World) -> Vec<(Entity, UiCanvas)> {
    let mut query = world.query_filtered::<(Entity, &UiCanvas), Without<ProjectedFrom>>();
    let mut canvases = query
        .iter(world)
        .map(|(entity, canvas)| (entity, canvas.clone()))
        .collect::<Vec<_>>();
    canvases.sort_by_key(|(entity, _)| entity.to_bits());
    canvases
}

fn authored_canvas_ancestor(world: &World, entity: Entity) -> Option<Entity> {
    let mut current = Some(entity);
    while let Some(candidate) = current {
        if world.get::<ProjectedFrom>(candidate).is_some() {
            return None;
        }
        if world.get::<UiCanvas>(candidate).is_some() {
            return Some(candidate);
        }
        current = world.get::<ChildOf>(candidate).map(ChildOf::parent);
    }
    None
}

fn update_toolbar_labels(
    hosts: Query<&ViewportUiHost>,
    mut mode_labels: Query<(&ViewportUiModeLabel, &mut Text), Without<ViewportUiFilterLabel>>,
    mut filter_labels: Query<(&ViewportUiFilterLabel, &mut Text), Without<ViewportUiModeLabel>>,
) {
    for (label, mut text) in &mut mode_labels {
        let Ok(host) = hosts.get(label.host) else {
            continue;
        };
        **text = match host.mode {
            UiCanvasMode::Scene => "UI: Scene",
            UiCanvasMode::Ui => "UI: Edit",
            UiCanvasMode::Interact => "UI: Interact",
        }
        .to_string();
    }
    for (label, mut text) in &mut filter_labels {
        let Ok(host) = hosts.get(label.host) else {
            continue;
        };
        **text = match host.filter {
            ViewportUiFilter::RuntimeVisible => "Canvases: Runtime",
            ViewportUiFilter::All => "Canvases: All",
        }
        .to_string();
    }
}
