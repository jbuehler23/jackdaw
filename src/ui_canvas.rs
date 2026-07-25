//! Dockable UI Canvas editor surface.

use std::collections::HashSet;

use bevy::{
    camera::RenderTarget,
    ecs::{
        component::Component,
        hierarchy::ChildOf,
        lifecycle::Despawn,
        observer::On,
        query::{Changed, Or, With, Without},
        system::{Commands, Query, ResMut},
        world::World,
    },
    image::ImageSampler,
    picking::events::{Click, Pointer},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat, TextureUsages},
    ui::{UiRect, widget::ViewportNode},
    ui_widgets::Button,
};
use jackdaw_scene_types::SceneNodeId;
use jackdaw_ui::{
    UiButton, UiCanvas, UiCheckbox, UiSlider, UiStyleOverride, UiTextInput, UiThemeScope, UiToggle,
};

use crate::{
    EditorEntity,
    selection::Selection,
    ui_projection::{
        ProjectedFrom, UiProjection, UiProjectionHandle, UiProjectionRoot, UiProjectionSpec,
    },
};

/// Registered dock-window id for the dedicated UI editor.
pub const UI_CANVAS_WINDOW_ID: &str = "jackdaw.ui_canvas";

const DEFAULT_CANVAS_WIDTH: u32 = 1280;
const DEFAULT_CANVAS_HEIGHT: u32 = 720;

/// Editing behavior for one Canvas/Viewport adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UiCanvasMode {
    /// Render as scene context without editor selection interception.
    Scene,
    /// Pick authored UI nodes and show editor affordances.
    #[default]
    Ui,
    /// Forward interaction to the projected runtime widgets.
    Interact,
}

impl UiCanvasMode {
    fn next(self) -> Self {
        match self {
            Self::Scene => Self::Ui,
            Self::Ui => Self::Interact,
            Self::Interact => Self::Scene,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Scene => "Scene",
            Self::Ui => "UI",
            Self::Interact => "Interact",
        }
    }
}

/// Per-instance state stored on the dock content entity.
#[derive(Component, Clone, Copy, Debug)]
pub struct UiCanvasPanelHost {
    /// Private Camera2d for this panel.
    pub camera: Entity,
    /// UI node containing the camera's [`ViewportNode`].
    pub viewport: Entity,
    /// Currently authored canvas, if the document contains one.
    pub canvas: Option<Entity>,
    /// View-local projection of `canvas`.
    pub projection: Option<UiProjectionHandle>,
    /// Current editing/interaction mode.
    pub mode: UiCanvasMode,
}

#[derive(Component)]
pub struct UiCanvasCamera;

#[derive(Component)]
pub struct UiCanvasViewport;

#[derive(Component)]
struct CanvasSelector {
    host: Entity,
}

#[derive(Component)]
struct CanvasSelectorLabel {
    host: Entity,
}

#[derive(Component)]
struct CanvasModeButton {
    host: Entity,
}

#[derive(Component)]
struct CanvasModeLabel {
    host: Entity,
}

#[derive(Component)]
struct CanvasEmptyState {
    host: Entity,
}

#[derive(Component, Clone, Copy)]
struct EditingProjection {
    host: Entity,
}

pub struct UiCanvasPlugin;

impl Plugin for UiCanvasPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_canvas_panel_despawn)
            .add_observer(on_canvas_selector_click)
            .add_observer(on_canvas_mode_click)
            .add_observer(on_projected_ui_click)
            .add_systems(Update, sync_canvas_panels)
            .add_systems(
                Update,
                (update_canvas_toolbar_labels, update_canvas_empty_states),
            );
    }
}

/// Build one independent docked UI Canvas panel.
pub fn build_ui_canvas_panel(world: &mut World, host: Entity) {
    let target = create_render_target(world);
    let camera = world
        .spawn((
            UiCanvasCamera,
            EditorEntity,
            Camera2d,
            Camera {
                order: -2,
                ..default()
            },
            RenderTarget::Image(target.into()),
        ))
        .id();

    let column = world
        .spawn((
            EditorEntity,
            Node {
                width: percent(100),
                height: percent(100),
                min_height: px(0),
                flex_direction: FlexDirection::Column,
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(Color::srgb(0.075, 0.08, 0.095)),
            ChildOf(host),
        ))
        .id();
    let toolbar = world
        .spawn((
            EditorEntity,
            Node {
                width: percent(100),
                height: px(30),
                flex_shrink: 0.0,
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(6),
                padding: UiRect::horizontal(px(6)),
                ..default()
            },
            BackgroundColor(Color::srgb(0.11, 0.12, 0.14)),
            ChildOf(column),
        ))
        .id();
    spawn_toolbar_button(
        world,
        toolbar,
        CanvasSelector { host },
        CanvasSelectorLabel { host },
        "Canvas: None",
    );
    spawn_toolbar_button(
        world,
        toolbar,
        CanvasModeButton { host },
        CanvasModeLabel { host },
        "Mode: UI",
    );

    let body = world
        .spawn((
            EditorEntity,
            Node {
                width: percent(100),
                flex_grow: 1.0,
                min_height: px(0),
                flex_direction: FlexDirection::Row,
                ..default()
            },
            ChildOf(column),
        ))
        .id();
    let palette_host = world
        .spawn((
            EditorEntity,
            Node {
                width: px(220),
                height: percent(100),
                min_height: px(0),
                flex_shrink: 0.0,
                ..default()
            },
            ChildOf(body),
        ))
        .id();
    crate::ui_widgets_panel::build_ui_widgets_panel(world, palette_host);
    let canvas_area = world
        .spawn((
            EditorEntity,
            Node {
                flex_grow: 1.0,
                min_width: px(0),
                min_height: px(0),
                ..default()
            },
            ChildOf(body),
        ))
        .id();
    let viewport = world
        .spawn((
            UiCanvasViewport,
            EditorEntity,
            Node {
                width: percent(100),
                height: percent(100),
                min_height: px(0),
                ..default()
            },
            BackgroundColor(Color::srgb(0.035, 0.038, 0.045)),
            ViewportNode::new(camera),
            Visibility::Hidden,
            ChildOf(canvas_area),
        ))
        .id();
    world.spawn((
        CanvasEmptyState { host },
        EditorEntity,
        Node {
            position_type: PositionType::Absolute,
            width: percent(100),
            height: percent(100),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            ..default()
        },
        Text::new("Choose Canvas, or add a widget to create one"),
        TextFont {
            font_size: FontSize::Px(14.0),
            ..default()
        },
        TextColor(Color::srgb(0.62, 0.64, 0.69)),
        Pickable::IGNORE,
        ChildOf(canvas_area),
    ));

    world.entity_mut(host).insert(UiCanvasPanelHost {
        camera,
        viewport,
        canvas: None,
        projection: None,
        mode: UiCanvasMode::Ui,
    });

    if let Some(canvas) = authored_canvases(world).first().copied()
        && let Err(error) = select_canvas(world, host, canvas)
    {
        warn!("failed to bind UI Canvas panel: {error}");
    }
}

/// Select which authored canvas one panel edits.
pub fn select_canvas(
    world: &mut World,
    host: Entity,
    canvas: Entity,
) -> Result<(), crate::ui_projection::UiProjectionError> {
    if world.get::<UiCanvas>(canvas).is_none() {
        return Err(crate::ui_projection::UiProjectionError::NotCanvas(canvas));
    }
    let Some(panel) = world.get::<UiCanvasPanelHost>(host).copied() else {
        return Err(crate::ui_projection::UiProjectionError::MissingCanvas(
            canvas,
        ));
    };
    if panel.canvas == Some(canvas) && panel.projection.is_some() {
        return Ok(());
    }
    if let Some(old) = panel.projection {
        UiProjection::close(world, old);
    }
    let projection = UiProjection::open(
        world,
        UiProjectionSpec {
            canvas,
            target_camera: panel.camera,
        },
    )?;
    mark_editing_projection(world, projection, host);
    if let Some(mut panel) = world.get_mut::<UiCanvasPanelHost>(host) {
        panel.canvas = Some(canvas);
        panel.projection = Some(projection);
    }
    Ok(())
}

fn clear_canvas(world: &mut World, host: Entity) {
    let projection = world
        .get::<UiCanvasPanelHost>(host)
        .and_then(|panel| panel.projection);
    if let Some(projection) = projection {
        UiProjection::close(world, projection);
    }
    if let Some(mut panel) = world.get_mut::<UiCanvasPanelHost>(host) {
        panel.canvas = None;
        panel.projection = None;
    }
}

fn refresh_panel_projection(world: &mut World, host: Entity) {
    let projection = world
        .get::<UiCanvasPanelHost>(host)
        .and_then(|panel| panel.projection);
    let Some(projection) = projection else {
        return;
    };
    if UiProjection::refresh(world, projection).is_ok() {
        mark_editing_projection(world, projection, host);
    }
}

fn mark_editing_projection(world: &mut World, projection: UiProjectionHandle, host: Entity) {
    if let Some(root) = UiProjection::root(world, projection) {
        world.entity_mut(root).insert(EditingProjection { host });
    }
}

fn create_render_target(world: &mut World) -> Handle<Image> {
    let size = Extent3d {
        width: DEFAULT_CANVAS_WIDTH,
        height: DEFAULT_CANVAS_HEIGHT,
        depth_or_array_layers: 1,
    };
    let mut image = Image::new_fill(
        size,
        TextureDimension::D2,
        &[9, 10, 12, 255],
        TextureFormat::Bgra8UnormSrgb,
        default(),
    );
    image.texture_descriptor.usage =
        TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST | TextureUsages::RENDER_ATTACHMENT;
    image.sampler = ImageSampler::linear();
    world.resource_mut::<Assets<Image>>().add(image)
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
                min_height: px(22),
                padding: UiRect::horizontal(px(7)),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                ..default()
            },
            BackgroundColor(Color::srgb(0.18, 0.19, 0.22)),
            ChildOf(toolbar),
        ))
        .id();
    world.spawn((
        label_marker,
        EditorEntity,
        Text::new(label),
        TextFont {
            font_size: FontSize::Px(12.0),
            ..default()
        },
        TextColor(Color::srgb(0.86, 0.87, 0.90)),
        Pickable::IGNORE,
        ChildOf(button),
    ));
}

fn authored_canvases(world: &mut World) -> Vec<Entity> {
    let mut query = world.query_filtered::<Entity, (With<UiCanvas>, Without<ProjectedFrom>)>();
    let mut canvases = query.iter(world).collect::<Vec<_>>();
    canvases.sort_by_key(|entity| entity.to_bits());
    canvases
}

fn on_canvas_panel_despawn(
    event: On<Despawn, UiCanvasPanelHost>,
    hosts: Query<&UiCanvasPanelHost>,
    mut commands: Commands,
) {
    let Ok(host) = hosts.get(event.event_target()).copied() else {
        return;
    };
    commands.queue(move |world: &mut World| {
        if let Some(projection) = host.projection {
            UiProjection::close(world, projection);
        }
        if world.get_entity(host.camera).is_ok() {
            world.entity_mut(host.camera).despawn();
        }
    });
}

fn on_canvas_selector_click(
    event: On<Pointer<Click>>,
    selectors: Query<&CanvasSelector>,
    mut commands: Commands,
) {
    let Ok(selector) = selectors.get(event.event_target()) else {
        return;
    };
    let host = selector.host;
    commands.queue(move |world: &mut World| {
        let canvases = authored_canvases(world);
        if canvases.is_empty() {
            clear_canvas(world, host);
            return;
        }
        let current = world
            .get::<UiCanvasPanelHost>(host)
            .and_then(|panel| panel.canvas);
        let next = current
            .and_then(|current| canvases.iter().position(|canvas| *canvas == current))
            .map(|index| canvases[(index + 1) % canvases.len()])
            .unwrap_or(canvases[0]);
        if let Err(error) = select_canvas(world, host, next) {
            warn!("failed to switch UI Canvas: {error}");
        }
    });
}

fn on_canvas_mode_click(
    event: On<Pointer<Click>>,
    buttons: Query<&CanvasModeButton>,
    mut hosts: Query<&mut UiCanvasPanelHost>,
) {
    let Ok(button) = buttons.get(event.event_target()) else {
        return;
    };
    if let Ok(mut host) = hosts.get_mut(button.host) {
        host.mode = host.mode.next();
    }
}

fn on_projected_ui_click(
    mut event: On<Pointer<Click>>,
    projected: Query<&ProjectedFrom>,
    projection_roots: Query<&EditingProjection, With<UiProjectionRoot>>,
    parents: Query<&ChildOf>,
    hosts: Query<&UiCanvasPanelHost>,
    authored: Query<(Entity, &SceneNodeId), Without<ProjectedFrom>>,
    mut selection: ResMut<Selection>,
    mut commands: Commands,
) {
    let mut current = Some(event.event_target());
    let mut node = None;
    let mut host = None;
    while let Some(entity) = current {
        node = node.or_else(|| projected.get(entity).ok().map(|source| source.0));
        host = host.or_else(|| projection_roots.get(entity).ok().map(|root| root.host));
        current = parents.get(entity).ok().map(ChildOf::parent);
    }
    let (Some(node), Some(host)) = (node, host) else {
        return;
    };
    let Ok(panel) = hosts.get(host) else {
        return;
    };
    if panel.mode != UiCanvasMode::Ui {
        return;
    }
    if let Some(entity) = authored
        .iter()
        .find_map(|(entity, candidate)| (*candidate == node).then_some(entity))
    {
        selection.select_single(&mut commands, entity);
        event.propagate(false);
    }
}

fn sync_canvas_panels(world: &mut World) {
    let available = authored_canvases(world);
    let available_set = available.iter().copied().collect::<HashSet<_>>();
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
    let mut host_query = world.query::<(Entity, &UiCanvasPanelHost)>();
    let hosts = host_query
        .iter(world)
        .map(|(entity, host)| (entity, *host))
        .collect::<Vec<_>>();

    for (host_entity, host) in hosts {
        let current_is_valid = host
            .canvas
            .is_some_and(|canvas| available_set.contains(&canvas));
        if !current_is_valid {
            if let Some(first) = available.first().copied() {
                if let Err(error) = select_canvas(world, host_entity, first) {
                    warn!("failed to bind UI Canvas panel: {error}");
                }
            } else {
                clear_canvas(world, host_entity);
            }
            continue;
        }
        if host.projection.is_none() {
            if let Some(canvas) = host.canvas
                && let Err(error) = select_canvas(world, host_entity, canvas)
            {
                warn!("failed to restore UI Canvas projection: {error}");
            }
        } else if host
            .canvas
            .is_some_and(|canvas| changed_canvases.contains(&canvas))
        {
            refresh_panel_projection(world, host_entity);
        }
    }
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

fn update_canvas_toolbar_labels(
    hosts: Query<&UiCanvasPanelHost>,
    names: Query<&Name>,
    mut selector_labels: Query<(&CanvasSelectorLabel, &mut Text), Without<CanvasModeLabel>>,
    mut mode_labels: Query<(&CanvasModeLabel, &mut Text), Without<CanvasSelectorLabel>>,
) {
    for (label, mut text) in &mut selector_labels {
        let Ok(host) = hosts.get(label.host) else {
            continue;
        };
        let canvas_name = host
            .canvas
            .and_then(|canvas| names.get(canvas).ok())
            .map(Name::as_str)
            .unwrap_or("None");
        **text = format!("Canvas: {canvas_name}");
    }
    for (label, mut text) in &mut mode_labels {
        let Ok(host) = hosts.get(label.host) else {
            continue;
        };
        **text = format!("Mode: {}", host.mode.label());
    }
}

fn update_canvas_empty_states(
    hosts: Query<&UiCanvasPanelHost>,
    empty_states: Query<(Entity, &CanvasEmptyState)>,
    mut visibility: Query<&mut Visibility>,
) {
    for (empty_entity, empty) in &empty_states {
        let Ok(host) = hosts.get(empty.host) else {
            continue;
        };
        let has_canvas = host.canvas.is_some();
        if let Ok(mut viewport_visibility) = visibility.get_mut(host.viewport) {
            *viewport_visibility = if has_canvas {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            };
        }
        if let Ok(mut empty_visibility) = visibility.get_mut(empty_entity) {
            *empty_visibility = if has_canvas {
                Visibility::Hidden
            } else {
                Visibility::Inherited
            };
        }
    }
}
