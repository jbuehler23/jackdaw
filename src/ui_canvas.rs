//! Dockable UI Canvas editor surface.

use std::collections::HashSet;

use bevy::{
    camera::RenderTarget,
    camera::visibility::VisibilitySystems,
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
    ui::{UiRect, UiSystems, widget::ViewportNode},
};
use jackdaw_feathers::{
    combobox::{
        ComboBoxChangeEvent, ComboBoxOptionData, ComboBoxSelectedIndex, combobox_with_selected,
    },
    tokens,
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

/// Selector order for [`UiCanvasMode`].
const MODE_ORDER: [UiCanvasMode; 3] = [
    UiCanvasMode::Ui,
    UiCanvasMode::Interact,
    UiCanvasMode::Scene,
];

impl UiCanvasMode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Scene => "Context",
            Self::Ui => "Select",
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
    /// Letterboxed frame holding the viewport and the editing overlays.
    pub stage: Entity,
    /// Area the stage is centered in.
    pub stage_area: Entity,
    /// Toolbar row holding this panel's controls.
    pub toolbar: Entity,
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

/// The canvas selector rebuilt whenever the document's canvas set changes.
#[derive(Component)]
struct CanvasSelector {
    host: Entity,
    /// Canvases the current options refer to, in option order.
    canvases: Vec<Entity>,
    /// Fixed-width toolbar slot owning this control.
    slot: Entity,
}

#[derive(Component)]
struct CanvasModeSelector {
    host: Entity,
}

#[derive(Component)]
struct CanvasEmptyState {
    host: Entity,
}

/// The letterboxed frame a panel renders its canvas into.
#[derive(Component, Clone, Copy)]
pub struct UiCanvasStage {
    pub host: Entity,
}

#[derive(Component, Clone, Copy)]
struct EditingProjection {
    host: Entity,
}

pub struct UiCanvasPlugin;

impl Plugin for UiCanvasPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(on_canvas_panel_despawn)
            .add_observer(on_projected_ui_click)
            .add_systems(Update, (sync_canvas_panels, sync_canvas_selectors).chain())
            .add_systems(
                PostUpdate,
                hide_authored_ui_sources
                    .after(VisibilitySystems::VisibilityPropagate)
                    .before(UiSystems::Stack),
            )
            .add_systems(Update, (update_stage_aspect, update_canvas_empty_states));
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
            BackgroundColor(tokens::PANEL_BG),
            ChildOf(host),
        ))
        .id();
    let toolbar = world
        .spawn((
            EditorEntity,
            Node {
                width: percent(100),
                height: px(tokens::TOOLBAR_HEIGHT),
                flex_shrink: 0.0,
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(tokens::TOOLBAR_GAP),
                padding: UiRect::horizontal(px(tokens::TOOLBAR_PADDING_LEFT)),
                border: UiRect::bottom(px(1)),
                ..default()
            },
            BackgroundColor(tokens::TOOLBAR_BG),
            BorderColor::all(tokens::TOOLBAR_BORDER),
            ChildOf(column),
        ))
        .id();

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
                width: px(160),
                height: percent(100),
                min_height: px(0),
                flex_shrink: 0.0,
                border: UiRect::right(px(1)),
                ..default()
            },
            BorderColor::all(tokens::PANEL_BORDER),
            ChildOf(body),
        ))
        .id();
    crate::ui_widgets_panel::build_ui_widgets_panel(world, palette_host);

    // The stage is letterboxed inside its area so the canvas reads as a device
    // frame rather than as whatever shape the dock happens to be.
    let stage_area = world
        .spawn((
            EditorEntity,
            Node {
                flex_grow: 1.0,
                min_width: px(0),
                min_height: px(0),
                align_items: AlignItems::Center,
                justify_content: JustifyContent::Center,
                padding: UiRect::all(px(12)),
                ..default()
            },
            BackgroundColor(tokens::WINDOW_BG),
            ChildOf(body),
        ))
        .id();
    let stage = world
        .spawn((
            UiCanvasStage { host },
            EditorEntity,
            Node {
                width: percent(100),
                height: percent(100),
                max_width: percent(100),
                max_height: percent(100),
                min_height: px(0),
                flex_shrink: 0.0,
                border: UiRect::all(px(1)),
                ..default()
            },
            BorderColor::all(tokens::BORDER_SUBTLE),
            BackgroundColor(Color::srgb(0.035, 0.038, 0.045)),
            Visibility::Hidden,
            ChildOf(stage_area),
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
            ViewportNode::new(camera),
            ChildOf(stage),
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
        Text::new("Add a widget to start a canvas"),
        TextFont {
            font_size: FontSize::Px(13.0),
            ..default()
        },
        TextColor(tokens::TAB_INACTIVE_TEXT),
        Pickable::IGNORE,
        ChildOf(stage_area),
    ));

    world.entity_mut(host).insert(UiCanvasPanelHost {
        camera,
        viewport,
        stage,
        stage_area,
        toolbar,
        canvas: None,
        projection: None,
        mode: UiCanvasMode::Ui,
    });
    spawn_mode_selector(world, toolbar, host);

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

/// A fixed-width toolbar cell. Comboboxes size to their parent, so the parent
/// is what sets a control's width.
fn spawn_control_slot(world: &mut World, toolbar: Entity, width: f32) -> Entity {
    world
        .spawn((
            EditorEntity,
            Node {
                width: px(width),
                flex_shrink: 0.0,
                ..default()
            },
            ChildOf(toolbar),
        ))
        .id()
}

/// Spawn the mode selector. Modes are fixed, so this is built once.
fn spawn_mode_selector(world: &mut World, toolbar: Entity, host: Entity) {
    let options = MODE_ORDER
        .iter()
        .map(|mode| ComboBoxOptionData::new(mode.label()))
        .collect::<Vec<_>>();
    let selected = MODE_ORDER
        .iter()
        .position(|mode| *mode == UiCanvasMode::Ui)
        .unwrap_or(0);
    let slot = spawn_control_slot(world, toolbar, 112.0);
    world
        .spawn((
            combobox_with_selected(options, selected),
            CanvasModeSelector { host },
            EditorEntity,
            ChildOf(slot),
        ))
        .observe(on_mode_selected);
}

fn on_mode_selected(
    event: On<ComboBoxChangeEvent>,
    selectors: Query<&CanvasModeSelector>,
    mut hosts: Query<&mut UiCanvasPanelHost>,
) {
    let Ok(selector) = selectors.get(event.entity) else {
        return;
    };
    let Some(mode) = MODE_ORDER.get(event.selected).copied() else {
        return;
    };
    if let Ok(mut host) = hosts.get_mut(selector.host) {
        host.mode = mode;
    }
}

/// Rebuild a panel's canvas selector when the document's canvas set or the
/// panel's choice changes. Comboboxes take their options at spawn, so the
/// control is reconciled rather than mutated.
fn sync_canvas_selectors(world: &mut World) {
    let canvases = authored_canvases(world);
    let labels = canvases
        .iter()
        .map(|canvas| {
            world
                .get::<Name>(*canvas)
                .map(|name| name.as_str().to_string())
                .unwrap_or_else(|| format!("Canvas {canvas}"))
        })
        .collect::<Vec<_>>();

    let mut host_query = world.query::<(Entity, &UiCanvasPanelHost)>();
    let panels = host_query
        .iter(world)
        .map(|(entity, host)| (entity, *host))
        .collect::<Vec<_>>();
    let mut selector_query = world.query::<(Entity, &CanvasSelector)>();
    let selectors = selector_query
        .iter(world)
        .map(|(entity, selector)| {
            (
                entity,
                selector.host,
                selector.canvases.clone(),
                selector.slot,
            )
        })
        .collect::<Vec<_>>();

    for (host_entity, panel) in panels {
        // A panel being torn down can still hold its host component for a tick
        // after its chrome is gone. Never parent onto a dead toolbar.
        if world.get_entity(panel.toolbar).is_err() {
            continue;
        }
        let existing = selectors
            .iter()
            .find(|(_, host, _, _)| *host == host_entity)
            .map(|(entity, _, current, slot)| (*entity, current.clone(), *slot));
        let selected = panel
            .canvas
            .and_then(|canvas| canvases.iter().position(|candidate| *candidate == canvas));
        if let Some((entity, current, slot)) = &existing {
            let index_matches = world
                .get::<ComboBoxSelectedIndex>(*entity)
                .map(|index| index.0)
                == selected;
            if *current == canvases && index_matches {
                continue;
            }
            if let Ok(slot) = world.get_entity_mut(*slot) {
                slot.despawn();
            }
        }

        let options = if labels.is_empty() {
            vec![ComboBoxOptionData::new("No canvas")]
        } else {
            labels
                .iter()
                .cloned()
                .map(ComboBoxOptionData::new)
                .collect()
        };
        let slot = spawn_control_slot(world, panel.toolbar, 150.0);
        world
            .spawn((
                combobox_with_selected(options, selected.unwrap_or(0)),
                CanvasSelector {
                    host: host_entity,
                    canvases: canvases.clone(),
                    slot,
                },
                EditorEntity,
                ChildOf(slot),
            ))
            .observe(on_canvas_selected);
    }
}

fn on_canvas_selected(
    event: On<ComboBoxChangeEvent>,
    selectors: Query<&CanvasSelector>,
    mut commands: Commands,
) {
    let Ok(selector) = selectors.get(event.entity) else {
        return;
    };
    let Some(canvas) = selector.canvases.get(event.selected).copied() else {
        return;
    };
    let host = selector.host;
    commands.queue(move |world: &mut World| {
        if let Err(error) = select_canvas(world, host, canvas) {
            warn!("failed to switch UI Canvas: {error}");
        }
    });
}

/// Letterbox each stage to its canvas reference aspect so authored layout is
/// previewed at the shape it ships in.
fn update_stage_aspect(
    hosts: Query<&UiCanvasPanelHost>,
    canvases: Query<&UiCanvas>,
    mut nodes: Query<&mut Node, With<UiCanvasStage>>,
) {
    for host in &hosts {
        let Ok(mut node) = nodes.get_mut(host.stage) else {
            continue;
        };
        let aspect = host
            .canvas
            .and_then(|canvas| canvases.get(canvas).ok())
            .map(|canvas| {
                let size = canvas.reference_size.as_vec2().max(Vec2::ONE);
                size.x / size.y
            });
        let desired = aspect.unwrap_or(16.0 / 9.0);
        if node.aspect_ratio != Some(desired) {
            node.aspect_ratio = Some(desired);
        }
    }
}

/// Authored canvases in stable document order. Raw `Entity` bits are recycled
/// across a reload, so they cannot order a user-visible list.
pub(crate) fn authored_canvases(world: &mut World) -> Vec<Entity> {
    let mut query = world
        .query_filtered::<(Entity, Option<&SceneNodeId>), (With<UiCanvas>, Without<ProjectedFrom>)>(
        );
    let mut canvases = query
        .iter(world)
        .map(|(entity, node)| (node.map(|node| node.0).unwrap_or(u64::MAX), entity))
        .collect::<Vec<_>>();
    canvases.sort();
    canvases.into_iter().map(|(_, entity)| entity).collect()
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
    let gesture_active = crate::ui_stage::manipulating(world);
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
        } else if !gesture_active
            && host
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

/// Keep the ECS-authored UI tree as scene data while rendering only its
/// per-view projections in the editor.
///
/// Mutating the authored [`Visibility`] would leak editor state into BSN and
/// would also be cloned into projections. [`InheritedVisibility`] is computed
/// state, so overriding it after hierarchy propagation suppresses the source
/// tree without changing the scene document.
fn hide_authored_ui_sources(world: &mut World) {
    let canvases = authored_canvases(world);
    let mut stack = canvases;
    while let Some(entity) = stack.pop() {
        let children = world
            .get::<Children>(entity)
            .map(|children| children.iter().collect::<Vec<_>>())
            .unwrap_or_default();
        if let Some(mut visibility) = world.get_mut::<InheritedVisibility>(entity) {
            *visibility = InheritedVisibility::HIDDEN;
        }
        stack.extend(children);
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
