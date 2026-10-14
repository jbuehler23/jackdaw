//! The scene viewports: which one the cursor is over, its camera, and the
//! ray under the cursor.
//!
//! Every 3D viewport panel has a camera carrying [`MainViewportCamera`] and a
//! UI node carrying [`SceneViewport`] that shows what the camera renders.
//! [`ActiveViewport`] names the pair under the cursor. Tools read
//! [`ViewportCursor`] to turn the cursor into a world-space ray, the way the
//! editor's own selection and gizmos do.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::ui::{UiGlobalTransform, widget::ViewportNode};

/// Marker for a 3D viewport camera. Once multi-viewport support
/// lands every viewport panel will spawn its own camera carrying this
/// marker, so queries that need *all* viewport cameras (or a specific
/// one selected via [`ActiveViewport`]) iterate them rather than
/// using `Single<>`.
#[derive(Component, Default, Clone, Copy, Debug)]
pub struct MainViewportCamera;

/// Marker on a UI node that hosts a 3D viewport (the leaf inside
/// which `ViewportNode` projects a camera's render target). With
/// multi-viewport, each registered `jackdaw.viewport` panel spawns
/// one of these.
#[derive(Component, Default, Clone, Copy, Debug)]
pub struct SceneViewport;

/// A viewport overlay an extension draws, listed with the editor's own in
/// the viewport settings menu.
///
/// Spawned by [`crate::ExtensionContext::register_viewport_overlay`]. The
/// operator that toggles the overlay keeps [`Self::shown`] current, which is
/// what the menu's check mark reads.
#[derive(Component, Clone, Debug)]
pub struct ViewportOverlay {
    /// The row's text.
    pub label: std::borrow::Cow<'static, str>,
    /// The operator the row calls.
    pub operator_id: &'static str,
    /// Whether the overlay is drawn.
    pub shown: bool,
}

/// What a viewport panel is showing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewportMode {
    /// The 3D world: perspective camera, grid, world-space tools.
    #[default]
    ThreeD,
    /// The 2D canvas: an orthographic stage at the authored reference size.
    TwoD,
}

impl ViewportMode {
    /// The mode a `mode` operator parameter names, or `None` when it names
    /// neither. Case and surrounding space are ignored.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "3d" => Some(Self::ThreeD),
            "2d" => Some(Self::TwoD),
            _ => None,
        }
    }
}

/// Window-to-camera-space mapping for a scene viewport node.
///
/// `top_left` is where the viewport node starts in window logical pixels,
/// `vp_size` is its logical size, and `remap` scales window-local pixels
/// onto the camera's render target (which may differ from the UI node
/// size on `HiDPI` / fractional scaling).
pub struct ViewportRemap {
    pub top_left: Vec2,
    pub vp_size: Vec2,
    pub remap: Vec2,
}

impl ViewportRemap {
    /// Compute remap parameters from the camera and the scene viewport's
    /// `ComputedNode` + `UiGlobalTransform`.
    pub fn new(camera: &Camera, computed: &ComputedNode, vp_transform: &UiGlobalTransform) -> Self {
        let scale = computed.inverse_scale_factor();
        let vp_pos = vp_transform.translation * scale;
        let vp_size = computed.size() * scale;
        let top_left = vp_pos - vp_size / 2.0;
        let target_size = camera.logical_viewport_size().unwrap_or(vp_size);
        Self {
            top_left,
            vp_size,
            remap: target_size / vp_size,
        }
    }
}

/// Tracks which viewport panel currently has the mouse over it.
///
/// Hover-routed viewport input (camera fly mode, click handling,
/// gizmo hover, etc.) reads this to decide which viewport to act on.
/// Updated each frame by `update_active_viewport`.
///
/// During a right-click fly session the resource keeps pointing at
/// the camera that started the session, even if the cursor strays
/// outside the viewport's bounds, so fly input stays attached to the
/// right viewport until the user releases the mouse.
///
/// A panel showing its 2D canvas reports its [`Self::host`] and [`Self::mode`]
/// but leaves [`Self::camera`] and [`Self::ui_node`] empty, which is what makes
/// every world-space tool stand down over a canvas.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct ActiveViewport {
    /// Camera entity of the currently-hovered viewport, when it is showing
    /// the 3D world.
    pub camera: Option<Entity>,
    /// UI-node entity of the currently-hovered viewport's
    /// `SceneViewport`, when it is showing the 3D world.
    pub ui_node: Option<Entity>,
    /// Panel entity of the currently-hovered viewport, in either mode.
    pub host: Option<Entity>,
    /// What the currently-hovered viewport is showing, or `None` when the
    /// cursor is over no viewport at all.
    pub mode: Option<ViewportMode>,
}

/// Bundled queries for converting screen position to a viewport ray.
/// Used by selection, gizmos, modal transforms, and drawing systems.
///
/// Multi-viewport-aware: routes via [`ActiveViewport`] (the hovered
/// viewport). Modal operators that need a stable target across frames
/// should capture an entity at start and pass it to [`Self::camera_for`].
#[derive(SystemParam)]
pub struct ViewportCursor<'w, 's> {
    pub windows: Query<'w, 's, &'static Window>,
    cameras: Query<'w, 's, (&'static Camera, &'static GlobalTransform), With<MainViewportCamera>>,
    viewports: Query<
        'w,
        's,
        (
            &'static ComputedNode,
            &'static UiGlobalTransform,
            &'static ViewportNode,
        ),
        With<SceneViewport>,
    >,
    active: Res<'w, ActiveViewport>,
    ui_scale: Res<'w, UiScale>,
    hover_map: Res<'w, bevy::picking::hover::HoverMap>,
}

impl ViewportCursor<'_, '_> {
    /// True when UI drawn on top of the hovered viewport, such as the terrain
    /// tool palette, is under the cursor rather than the viewport's own
    /// `SceneViewport` node.
    pub fn blocked_by_overlay(&self) -> bool {
        hover_blocks_click(&self.hover_map, self.active.ui_node)
    }

    pub fn viewport_pointer(&self) -> Option<Vec2> {
        let viewport_entity = self.active.ui_node?;
        if hover_blocks_click(&self.hover_map, Some(viewport_entity)) {
            return None;
        }
        self.cursor()
    }

    /// The hovered viewport's camera + global transform.
    pub fn camera(&self) -> Option<(&Camera, &GlobalTransform)> {
        let camera_entity = self.active.camera?;
        self.cameras.get(camera_entity).ok()
    }

    /// The hovered viewport's UI-node geometry (for cursor remapping).
    pub fn viewport(&self) -> Option<(&ComputedNode, &UiGlobalTransform)> {
        let ui_entity = self.active.ui_node?;
        self.viewports.get(ui_entity).ok().map(|(c, t, _)| (c, t))
    }

    /// Camera entity of the hovered viewport (for modal capture).
    pub fn camera_entity(&self) -> Option<Entity> {
        self.active.camera
    }

    /// UI-node entity of the hovered viewport (for modal capture).
    pub fn viewport_entity(&self) -> Option<Entity> {
        self.active.ui_node
    }

    /// Look up a specific camera by entity. Used by modal operators
    /// that captured the active viewport at drag-start and want to
    /// keep referring to it across frames regardless of where the
    /// cursor wanders.
    pub fn camera_for(&self, entity: Entity) -> Option<(&Camera, &GlobalTransform)> {
        self.cameras.get(entity).ok()
    }

    /// Look up a specific viewport UI node by entity (companion to
    /// [`Self::camera_for`]).
    pub fn viewport_for(&self, entity: Entity) -> Option<(&ComputedNode, &UiGlobalTransform)> {
        self.viewports.get(entity).ok().map(|(c, t, _)| (c, t))
    }

    /// Convert a window-space cursor position into the camera-space
    /// coordinates of a specific viewport. Positions outside the pane
    /// are still remapped, so a captured-viewport drag can keep updating
    /// over adjacent UI. Returns `None` only if `viewport_entity` is no
    /// longer a `SceneViewport`.
    pub fn viewport_cursor_for(
        &self,
        camera: &Camera,
        viewport_entity: Entity,
        cursor: Vec2,
    ) -> Option<Vec2> {
        let (computed, vp_tf, _) = self.viewports.get(viewport_entity).ok()?;
        let map = ViewportRemap::new(camera, computed, vp_tf);
        Some((cursor - map.top_left) * map.remap)
    }

    /// The world-space ray under the cursor in the hovered viewport, or
    /// `None` when the cursor is over no 3D viewport or over UI drawn on
    /// top of one.
    pub fn ray(&self) -> Option<Ray3d> {
        let camera = self.active.camera?;
        let viewport = self.active.ui_node?;
        let cursor = self.viewport_pointer()?;
        self.ray_for(camera, viewport, cursor)
    }

    /// The world-space ray through `cursor`, a window position in
    /// ui-logical pixels, as seen by `camera` in the viewport node
    /// `viewport`. For a drag that captured its viewport when it began and
    /// follows it wherever the cursor goes.
    pub fn ray_for(&self, camera: Entity, viewport: Entity, cursor: Vec2) -> Option<Ray3d> {
        let (camera, transform) = self.camera_for(camera)?;
        let local = self.viewport_cursor_for(camera, viewport, cursor)?;
        camera.viewport_to_world(transform, local).ok()
    }

    /// Cursor position in ui-logical pixels
    pub fn cursor(&self) -> Option<Vec2> {
        let window = self.windows.single().ok()?;
        let raw = window.cursor_position()?;
        Some(raw / self.ui_scale.0)
    }
}

/// True when the hover map contains an entity other than `viewport_entity`.
/// With no viewport to compare against, nothing can be over one.
pub fn hover_blocks_click(
    hover_map: &bevy::picking::hover::HoverMap,
    viewport_entity: Option<Entity>,
) -> bool {
    let Some(viewport_entity) = viewport_entity else {
        return false;
    };
    hover_map
        .values()
        .flat_map(|hits| hits.keys())
        .any(|&entity| entity != viewport_entity)
}

/// Cursor position in ui-logical pixels, the space `Val::Px` and
/// [`ViewportRemap`] operate in. Use this anywhere the
/// cursor will be compared against UI nodes or fed into viewport-coordinate
/// math, instead of calling `Window::cursor_position()` directly.
///
/// Systems that already take [`ViewportCursor`] can use
/// [`ViewportCursor::cursor`] instead and drop this parameter.
#[derive(SystemParam)]
pub struct UiCursorPos<'w, 's> {
    windows: Query<'w, 's, &'static Window>,
    ui_scale: Res<'w, UiScale>,
}

impl UiCursorPos<'_, '_> {
    /// Ui-logical cursor position, or `None` if the cursor is outside
    /// the window / there's no primary window.
    pub fn get(&self) -> Option<Vec2> {
        let window = self.windows.single().ok()?;
        let raw = window.cursor_position()?;
        Some(raw / self.ui_scale.0)
    }
}

#[cfg(test)]
mod tests {
    use bevy::picking::{backend::HitData, hover::HoverMap, pointer::PointerId};

    use super::*;

    fn hover_map_with(entity: Entity) -> HoverMap {
        let hit = HitData {
            camera: Entity::PLACEHOLDER,
            depth: 0.0,
            position: None,
            normal: None,
            extra: None,
        };
        let mut hits = bevy::ecs::entity::EntityHashMap::default();
        hits.insert(entity, hit);
        let mut map = HoverMap::default();
        map.insert(PointerId::Mouse, hits);
        map
    }

    #[test]
    fn viewport_own_node_does_not_block_the_click() {
        let viewport = Entity::from_raw_u32(1).unwrap();
        let hover_map = hover_map_with(viewport);
        assert!(!hover_blocks_click(&hover_map, Some(viewport)));
    }

    #[test]
    fn an_overlay_entity_blocks_the_click() {
        let viewport = Entity::from_raw_u32(1).unwrap();
        let palette_button = Entity::from_raw_u32(2).unwrap();
        let hover_map = hover_map_with(palette_button);
        assert!(hover_blocks_click(&hover_map, Some(viewport)));
    }

    #[test]
    fn an_empty_hover_map_does_not_block_the_click() {
        let viewport = Entity::from_raw_u32(1).unwrap();
        assert!(!hover_blocks_click(&HoverMap::default(), Some(viewport)));
    }

    #[test]
    fn no_active_viewport_does_not_block_the_click() {
        let hover_map = hover_map_with(Entity::from_raw_u32(2).unwrap());
        assert!(!hover_blocks_click(&hover_map, None));
    }
}
