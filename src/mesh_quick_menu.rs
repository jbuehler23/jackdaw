//! Mesh quick-menu configuration: the operators offered in the radial
//! menu for each brush edit sub-mode, and the mapping to widget items.

use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ActiveModalQuery;
use jackdaw_feathers::icons::Icon;
use jackdaw_widgets::{
    RadialMenuItem, RadialMenuSelect, RadialMenuState, confirm_radial_menu, open_radial_menu,
};

use crate::brush::{BrushEditMode, EditMode};
use crate::keybind_focus::KeybindFocus;

/// One configured quick-menu entry: the operator dispatched on selection
/// plus how it presents in the ring. `operator_id` is dispatched verbatim.
#[derive(Clone)]
pub struct MeshMenuItem {
    pub operator_id: String,
    pub label: String,
    pub icon: Icon,
}

impl MeshMenuItem {
    fn new(operator_id: &str, label: &str, icon: Icon) -> Self {
        Self {
            operator_id: operator_id.to_string(),
            label: label.to_string(),
            icon,
        }
    }
}

/// Per-sub-mode quick-menu configuration. Each list is the ordered ring
/// shown when the quick-menu opens in that sub-mode; edit these lists to
/// change the menu. Curated to stay within a single readable ring.
#[derive(Resource)]
pub struct MeshQuickMenu {
    pub vertex: Vec<MeshMenuItem>,
    pub edge: Vec<MeshMenuItem>,
    pub face: Vec<MeshMenuItem>,
}

impl Default for MeshQuickMenu {
    fn default() -> Self {
        Self {
            vertex: vec![
                MeshMenuItem::new("brush.mesh.connect_verts", "Connect Verts", Icon::Waypoints),
                MeshMenuItem::new("brush.mesh.weld_selected", "Weld", Icon::Merge),
                MeshMenuItem::new("brush.mesh.dissolve_verts", "Dissolve Verts", Icon::Eraser),
                MeshMenuItem::new("brush.mesh.vertex_bevel", "Vertex Bevel", Icon::Shrink),
            ],
            edge: vec![
                MeshMenuItem::new("brush.mesh.edge_bevel", "Edge Bevel", Icon::Slice),
                MeshMenuItem::new("brush.mesh.loop_cut", "Loop Cut", Icon::Scissors),
                MeshMenuItem::new(
                    "brush.mesh.bridge_edge_loops",
                    "Bridge Edge Loops",
                    Icon::Combine,
                ),
                MeshMenuItem::new("brush.mesh.dissolve_edges", "Dissolve Edges", Icon::Eraser),
            ],
            face: vec![
                MeshMenuItem::new("brush.mesh.extrude_region", "Extrude", Icon::Expand),
                MeshMenuItem::new("brush.mesh.inset", "Inset", Icon::Frame),
                MeshMenuItem::new("brush.mesh.dissolve_faces", "Dissolve Faces", Icon::Eraser),
                MeshMenuItem::new("brush.mesh.subdivide", "Subdivide", Icon::Grid3x3),
            ],
        }
    }
}

/// Map the configured items for `mode` to widget items, with each item's
/// `action` set to its operator id. The `Clip` and `Knife` sub-modes have
/// no quick-menu and yield an empty list.
pub fn items_for_submode(menu: &MeshQuickMenu, mode: BrushEditMode) -> Vec<RadialMenuItem> {
    let items = match mode {
        BrushEditMode::Vertex => &menu.vertex,
        BrushEditMode::Edge => &menu.edge,
        BrushEditMode::Face => &menu.face,
        BrushEditMode::Clip | BrushEditMode::Knife => return Vec::new(),
    };
    items
        .iter()
        .map(|item| RadialMenuItem {
            label: item.label.clone(),
            icon: Some(item.icon),
            action: item.operator_id.clone(),
        })
        .collect()
}

/// Open the mesh quick-menu when the quick-menu key is held in a brush edit
/// sub-mode, and confirm the highlighted wedge when it releases. Gated like the
/// tool keys: nothing happens while text is being typed or a modal operator is
/// running. Bound to C, which is context-aware: in a brush edit sub-mode it
/// opens this menu, while in object mode it stays the cut-brush gesture (the
/// cut handler yields in edit mode). It avoids the axis-constraint keys (X/Y/Z)
/// so the hold never arms a constrained transform.
fn mesh_quick_menu_input(
    keys: Res<ButtonInput<KeyCode>>,
    edit_mode: Res<EditMode>,
    keybind_focus: KeybindFocus,
    active: ActiveModalQuery,
    menu: Res<MeshQuickMenu>,
    windows: Query<&Window, With<PrimaryWindow>>,
    radial: Res<RadialMenuState>,
    mut commands: Commands,
) {
    if keys.just_pressed(KeyCode::KeyC)
        && let EditMode::BrushEdit(mode) = *edit_mode
        && !keybind_focus.is_typing()
        && !active.is_modal_running()
    {
        let items = items_for_submode(&menu, mode);
        if !items.is_empty()
            && let Some(cursor) = windows.iter().next().and_then(Window::cursor_position)
        {
            open_radial_menu(&mut commands, cursor, items);
        }
    }

    if keys.just_released(KeyCode::KeyC) && radial.open.is_some() {
        commands.queue(|world: &mut World| confirm_radial_menu(world));
    }
}

/// Dispatch the operator named by a confirmed radial selection. An operator
/// that is unavailable in the current state is skipped by the dispatch gate, so
/// such a selection simply does nothing.
fn dispatch_quick_menu_selection(select: On<RadialMenuSelect>, mut commands: Commands) {
    commands.operator(select.event().action.clone()).call();
}

/// Registers the quick-menu config, the held-key open/confirm system, and the
/// selection-dispatch observer.
pub struct MeshQuickMenuPlugin;

impl Plugin for MeshQuickMenuPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MeshQuickMenu>()
            .add_systems(
                Update,
                mesh_quick_menu_input.run_if(in_state(crate::AppState::Editor)),
            )
            .add_observer(dispatch_quick_menu_selection);
    }
}
