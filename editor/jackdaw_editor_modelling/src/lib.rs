//! Brush and mesh modelling tools for the Jackdaw scene editor: topology
//! operators, knife mode, element delete and nudge, the mesh quick menu, the
//! measure tool, the modifier and mirror plane operators and the brush
//! inspector card. The brush data, its meshes and its CSG stay in the editor
//! core.
//!
//! Add [`ModellingEditorPlugin`] after the editor's own plugins.

pub mod brush_card;
pub mod brush_element_ops;
pub mod knife_mode;
pub mod measure_tool;
pub mod mesh_quick_menu;
pub mod mirror_plane_ops;
pub mod modifier_ops;
pub mod topology_ops;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::ExtensionAppExt as _;
use jackdaw_editor_core::MenuBarSlots;
use jackdaw_editor_core::inspector::{ComponentCardRegistry, InspectorSync};
use jackdaw_editor_core::scene_io::hooks::DragResets;

pub use jackdaw_editor_core::modelling_mode::MODELLING_EDITOR_EXTENSION_ID;
use topology_ops::edge_bevel::EdgeBevelModalState;
use topology_ops::edge_slide_modal::EdgeSlideModalState;
use topology_ops::extrude::ExtrudeModalState;
use topology_ops::inset::InsetModalState;
use topology_ops::loop_cut::{LoopCutModalState, LoopCutPreviewLines};
use topology_ops::vertex_bevel::VertexBevelModalState;
use topology_ops::vertex_slide_modal::VertexSlideModalState;

/// The modelling tools' state and systems, their hooks into the editor, and
/// [`ModellingEditorExtension`].
pub struct ModellingEditorPlugin;

impl Plugin for ModellingEditorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<InsetModalState>()
            .init_resource::<LoopCutModalState>()
            .init_resource::<LoopCutPreviewLines>()
            .init_resource::<ExtrudeModalState>()
            .init_resource::<EdgeSlideModalState>()
            .init_resource::<VertexSlideModalState>()
            .init_resource::<EdgeBevelModalState>()
            .init_resource::<VertexBevelModalState>()
            .add_plugins((
                measure_tool::MeasureToolPlugin,
                mesh_quick_menu::MeshQuickMenuPlugin,
            ))
            .add_systems(
                Update,
                (
                    // A hovered plane handle grabs the press ahead of the
                    // sub-element drag triggers, which bail while it is hovered.
                    mirror_plane_ops::mirror_plane_drag_invoke_trigger
                        .after(jackdaw_editor_core::brush::interaction::brush_vertex_edge_hover)
                        .before(jackdaw_editor_core::brush_drag_ops::face_drag_invoke_trigger),
                    knife_mode::handle_knife_mode
                        .after(jackdaw_editor_core::brush::interaction::handle_clip_mode),
                )
                    .in_set(jackdaw_editor_core::EditorInteractionSystems),
            )
            .add_systems(
                PostUpdate,
                (
                    topology_ops::loop_cut::draw_loop_cut_preview,
                    topology_ops::loop_cut::update_loop_cut_mid_label,
                    knife_mode::draw_knife_overlay,
                )
                    .chain()
                    .in_set(jackdaw_editor_core::JackdawDrawSystems),
            )
            .add_systems(
                Update,
                topology_ops::select_elements::flag_menu_on_element_editing
                    .run_if(in_state(jackdaw_editor_core::AppState::Editor)),
            )
            .add_systems(
                Update,
                brush_card::update_brush_face_properties
                    .after(InspectorSync)
                    .run_if(in_state(jackdaw_editor_core::AppState::Editor)),
            )
            .add_observer(brush_card::on_brush_face_text_commit);
        let world = app.world_mut();
        {
            let mut resets = world.get_resource_or_init::<DragResets>();
            resets.register::<ExtrudeModalState>();
            resets.register::<InsetModalState>();
            resets.register::<EdgeBevelModalState>();
            resets.register::<VertexBevelModalState>();
            resets.register::<EdgeSlideModalState>();
            resets.register::<VertexSlideModalState>();
        }
        world
            .get_resource_or_init::<ComponentCardRegistry>()
            .register::<jackdaw_scene_types::Brush>(brush_card::fill_brush_card_body);
        world
            .get_resource_or_init::<MenuBarSlots>()
            .register(topology_ops::select_elements::select_menu);
        app.register_extension::<ModellingEditorExtension>();
    }
}

/// The modelling operators.
#[derive(Default)]
pub struct ModellingEditorExtension;

impl JackdawExtension for ModellingEditorExtension {
    fn id(&self) -> String {
        MODELLING_EDITOR_EXTENSION_ID.to_string()
    }

    fn label(&self) -> String {
        "Modelling".to_string()
    }

    fn description(&self) -> String {
        "Brush topology and mesh editing, knife, modifiers, mirror planes and the measure tool."
            .to_string()
    }

    fn kind(&self) -> ExtensionKind {
        ExtensionKind::Builtin
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        topology_ops::bridge_edge_loops::add_to_extension(ctx);
        topology_ops::connect_verts::add_to_extension(ctx);
        topology_ops::dissolve_edges::add_to_extension(ctx);
        topology_ops::dissolve_faces::add_to_extension(ctx);
        topology_ops::dissolve_verts::add_to_extension(ctx);
        topology_ops::edge_bevel::add_to_extension(ctx);
        topology_ops::vertex_bevel::add_to_extension(ctx);
        topology_ops::edge_slide::add_to_extension(ctx);
        topology_ops::edge_slide_modal::add_to_extension(ctx);
        topology_ops::select_elements::add_to_extension(ctx);
        topology_ops::select_invert::add_to_extension(ctx);
        topology_ops::select_less::add_to_extension(ctx);
        topology_ops::select_linked::add_to_extension(ctx);
        topology_ops::select_loop::add_to_extension(ctx);
        topology_ops::select_more::add_to_extension(ctx);
        topology_ops::select_ring::add_to_extension(ctx);
        topology_ops::extrude::add_to_extension(ctx);
        topology_ops::inset::add_to_extension(ctx);
        topology_ops::loop_cut::add_to_extension(ctx);
        topology_ops::make_edge_face::add_to_extension(ctx);
        topology_ops::merge_by_distance::add_to_extension(ctx);
        topology_ops::mirror_ops::add_to_extension(ctx);
        topology_ops::subdivide::add_to_extension(ctx);
        topology_ops::vertex_slide::add_to_extension(ctx);
        topology_ops::element_transform::add_to_extension(ctx);
        topology_ops::randomize::add_to_extension(ctx);
        topology_ops::vertex_slide_modal::add_to_extension(ctx);
        topology_ops::weld_selected::add_to_extension(ctx);
        topology_ops::uv_reset_axes::add_to_extension(ctx);
        topology_ops::uv_world_aligned::add_to_extension(ctx);
        topology_ops::uv_rotate_90::add_to_extension(ctx);
        topology_ops::uv_fit_to_face::add_to_extension(ctx);
        topology_ops::uv_texel_density::add_to_extension(ctx);
        topology_ops::uv_align_to_edge::add_to_extension(ctx);
        topology_ops::reconvexify::add_to_extension(ctx);
        measure_tool::add_to_extension(ctx);
        brush_card::add_to_extension(ctx);
        brush_element_ops::add_to_extension(ctx);
        mirror_plane_ops::add_to_extension(ctx);
        modifier_ops::add_to_extension(ctx);
        knife_mode::add_to_extension(ctx);
    }
}
