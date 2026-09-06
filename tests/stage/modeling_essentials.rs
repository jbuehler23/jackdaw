//! Modeling essentials: live `MeshMirror` evaluation feeding `BrushMeshCache`,
//! the modifier-stack ops, the x-ray material swap, and reference images.
//!
//! The brush mesh systems are gated to `AppState::Editor` in the editor's
//! plugin schedule, so these tests drive them directly in the production
//! order: removal marker, then regenerate.

use crate::util;

use bevy::prelude::*;
use jackdaw::brush::{
    Brush, BrushMaterialPalette, BrushMeshCache, BrushMeshChunk, MeshMirror,
    ensure_brush_chunk_materials, mark_brushes_changed_on_modifier_removal,
    regenerate_brush_meshes, setup_default_materials,
};
use jackdaw::view_modes::ViewModeSettings;
use jackdaw_api::prelude::*;
use jackdaw_geometry::{Modifier, ModifierEntry, ModifierStack};

/// Wrap a `MeshMirror` as a single-entry editor modifier stack.
fn mirror_stack(mirror: MeshMirror) -> ModifierStack {
    ModifierStack {
        modifiers: vec![ModifierEntry::new(Modifier::Mirror(mirror))],
    }
}

/// Spawn a half-cube occupying x in [0, 1]: a unit cube shifted so its
/// -X face lies exactly on the X = 0 mirror plane.
fn spawn_half_cube(app: &mut App) -> Entity {
    let mut brush = Brush::cuboid(0.5, 0.5, 0.5);
    for vert in &mut brush.topology.vertices {
        vert.position.x += 0.5;
    }
    app.world_mut()
        .spawn((
            Name::new("HalfCube"),
            brush,
            Transform::default(),
            Visibility::default(),
        ))
        .id()
}

/// Run the mirror-removal marker and the mesh rebuild once, in the
/// order the editor schedules them.
fn tick_brush_meshes(app: &mut App) {
    app.world_mut()
        .run_system_cached(mark_brushes_changed_on_modifier_removal)
        .expect("modifier removal marker ran");
    app.world_mut()
        .run_system_cached(regenerate_brush_meshes)
        .expect("regenerate_brush_meshes ran");
}

fn cache_of(app: &App, entity: Entity) -> &BrushMeshCache {
    app.world()
        .entity(entity)
        .get::<BrushMeshCache>()
        .expect("brush has a BrushMeshCache after regenerate")
}

/// Spawn a unit cube centered on the origin (x in [-0.5, 0.5]) so it
/// straddles the X = 0 mirror plane; a bisecting mirror must then cut it
/// and introduce cap/split geometry.
fn spawn_straddle_cube(app: &mut App) -> Entity {
    app.world_mut()
        .spawn((
            Name::new("StraddleCube"),
            Brush::cuboid(0.5, 0.5, 0.5),
            Transform::default(),
            Visibility::default(),
        ))
        .id()
}

#[test]
fn mirrored_brush_cache_holds_both_halves_with_source_maps() {
    let mut app = util::headless_app();
    app.finish();
    app.update();

    let entity = spawn_half_cube(&mut app);
    app.world_mut()
        .entity_mut(entity)
        .insert(mirror_stack(MeshMirror::default()));
    tick_brush_meshes(&mut app);

    let cache = cache_of(&app, entity);
    // 6 authored faces + 5 mirrored: the -X face lies in the mirror
    // plane (all four verts weld) so it produces no mirrored copy.
    assert_eq!(cache.face_polygons.len(), 11);
    // 8 authored verts + 4 mirrored: the 4 on-plane verts weld.
    assert_eq!(cache.vertices.len(), 12);
    assert!(!cache.face_source.is_empty());
    assert!(!cache.vert_source.is_empty());
    let last = cache.face_polygons.len() - 1;
    assert!(
        cache.face_to_authored(last) < 6,
        "mirrored face must resolve to an authored face index"
    );
    // Authored elements keep their indices (identity prefix).
    assert_eq!(cache.face_to_authored(0), 0);
}

#[test]
fn picking_remaps_to_authored_space() {
    let mut app = util::headless_app();
    app.finish();
    app.update();

    let entity = spawn_half_cube(&mut app);
    app.world_mut()
        .entity_mut(entity)
        .insert(mirror_stack(MeshMirror::default()));
    tick_brush_meshes(&mut app);

    let cache = cache_of(&app, entity);
    for f in 0..cache.face_polygons.len() {
        assert!(
            cache.face_to_authored(f) < 6,
            "evaluated face {f} must resolve to an authored face"
        );
    }
    for v in 0..cache.vertices.len() {
        assert!(
            cache.vert_to_authored(v) < 8,
            "evaluated vertex {v} must resolve to an authored vertex"
        );
    }
    // The authored prefix is exactly the un-mirrored element set; it is
    // the baseline the legacy hull rebuild is allowed to see.
    assert_eq!(cache.authored_face_count(), 6);
    assert_eq!(cache.authored_vertex_count(), 8);
    assert_eq!(cache.authored_vertices().len(), 8);
    assert_eq!(cache.authored_face_polygons().len(), 6);
}

#[test]
fn removing_mirror_restores_authored_only_cache() {
    let mut app = util::headless_app();
    app.finish();
    app.update();

    let entity = spawn_half_cube(&mut app);
    app.world_mut()
        .entity_mut(entity)
        .insert(mirror_stack(MeshMirror::default()));
    tick_brush_meshes(&mut app);
    assert_eq!(cache_of(&app, entity).face_polygons.len(), 11);

    // Removal alone must trigger the rebuild within one marker + regenerate pass.
    app.world_mut().entity_mut(entity).remove::<ModifierStack>();
    tick_brush_meshes(&mut app);

    let cache = cache_of(&app, entity);
    assert_eq!(cache.face_polygons.len(), 6);
    assert!(cache.face_source.is_empty());
    assert!(cache.vert_source.is_empty());
}

#[test]
fn bisect_cut_geometry_is_not_editable() {
    let mut app = util::headless_app();
    app.finish();
    app.update();

    // A cube straddling the mirror plane with a bisecting X mirror: the
    // clip caps the cut and splits verts onto x=0, all carrying NO_SOURCE.
    let entity = spawn_straddle_cube(&mut app);
    app.world_mut()
        .entity_mut(entity)
        .insert(mirror_stack(MeshMirror {
            mirror_x: true,
            bisect: [true, false, false],
            ..MeshMirror::default()
        }));
    tick_brush_meshes(&mut app);

    let cache = cache_of(&app, entity);

    assert!(
        cache.vert_source.contains(&jackdaw_geometry::NO_SOURCE),
        "bisect must introduce NO_SOURCE cut verts"
    );

    // Index 0 lies in the authored identity prefix.
    assert_eq!(cache.authored_vert(0), Some(0));
    let cut_index = cache
        .vert_source
        .iter()
        .position(|&s| s == jackdaw_geometry::NO_SOURCE)
        .expect("a NO_SOURCE vert exists");
    assert_eq!(
        cache.authored_vert(cut_index),
        None,
        "cut geometry must not map to an authored vertex"
    );

    // Cut geometry is excluded from editing.
    let editable = (0..cache.vertices.len())
        .filter(|&v| cache.authored_vert(v).is_some())
        .count();
    assert!(
        editable < cache.vertices.len(),
        "editable verts ({editable}) must exclude the {} cut verts",
        cache.vertices.len() - editable
    );

    // A NO_SOURCE face index maps to no authored face, so the face picker skips it.
    let cap_face = cache
        .face_source
        .iter()
        .position(|&s| s == jackdaw_geometry::NO_SOURCE)
        .expect("a NO_SOURCE cap face exists");
    assert_eq!(
        cache.authored_face(cap_face),
        None,
        "the cut cap face must not map to an authored face"
    );
}

#[test]
fn non_bisecting_mirror_leaves_every_vert_editable() {
    let mut app = util::headless_app();
    app.finish();
    app.update();

    // Bisect disabled: a plain reflect adds mirrored copies but no NO_SOURCE
    // cut geometry, which pins the cut-only behaviour to the bisect flag.
    let entity = spawn_straddle_cube(&mut app);
    app.world_mut()
        .entity_mut(entity)
        .insert(mirror_stack(MeshMirror {
            mirror_x: true,
            bisect: [false; 3],
            ..MeshMirror::default()
        }));
    tick_brush_meshes(&mut app);

    let cache = cache_of(&app, entity);
    assert!(
        cache
            .vert_source
            .iter()
            .all(|&s| s != jackdaw_geometry::NO_SOURCE),
        "a non-bisecting mirror introduces no cut geometry"
    );
    assert!(
        (0..cache.vertices.len()).all(|v| cache.authored_vert(v).is_some()),
        "every vert is editable without a bisect"
    );
}

/// Select `entity` and clear the headless placeholder `InputFocus`, which the
/// mirror ops' availability checks read as a text field owning the keyboard.
fn select_for_operators(app: &mut App, entity: Entity) {
    use bevy::input_focus::InputFocus;
    app.world_mut().resource_mut::<InputFocus>().clear();
    app.world_mut()
        .resource_mut::<jackdaw::selection::Selection>()
        .entities = vec![entity];
    app.update();
}

#[track_caller]
fn assert_available(app: &mut App, id: &'static str, expected: bool) {
    let available = app.world_mut().operator(id).is_available().unwrap();
    assert_eq!(available, expected, "{id} availability");
}

#[test]
fn mirror_ops_gate_on_selection_and_component() {
    let mut app = util::headless_app();
    app.finish();
    app.update();
    app.world_mut()
        .resource_mut::<bevy::input_focus::InputFocus>()
        .clear();
    app.update();

    assert_available(&mut app, "mesh.mirror.add", false);
    assert_available(&mut app, "mesh.mirror.apply", false);
    assert_available(&mut app, "mesh.mirror.bisect", false);
    assert_available(&mut app, "mesh.symmetrize", false);

    let entity = spawn_half_cube(&mut app);
    select_for_operators(&mut app, entity);
    assert_available(&mut app, "mesh.mirror.add", true);
    assert_available(&mut app, "mesh.mirror.apply", false);
    assert_available(&mut app, "mesh.mirror.bisect", true);
    assert_available(&mut app, "mesh.symmetrize", true);

    app.world_mut()
        .entity_mut(entity)
        .insert(mirror_stack(MeshMirror::default()));
    app.update();
    assert_available(&mut app, "mesh.mirror.add", false);
    assert_available(&mut app, "mesh.mirror.apply", true);
    assert_available(&mut app, "mesh.mirror.bisect", false);
    assert_available(&mut app, "mesh.symmetrize", true);
}

#[test]
fn apply_bakes_mirror_into_authored_topology() {
    let mut app = util::headless_app();
    app.finish();
    app.update();

    let entity = spawn_half_cube(&mut app);
    app.world_mut()
        .entity_mut(entity)
        .insert(mirror_stack(MeshMirror::default()));
    select_for_operators(&mut app, entity);

    let result = app
        .world_mut()
        .operator("mesh.mirror.apply")
        .call()
        .expect("mesh.mirror.apply dispatched");
    assert_eq!(result, OperatorResult::Finished);

    // The component is gone and the authored topology now holds both
    // halves: 6 authored faces + 5 mirrored (the on-plane face welds
    // away), 8 authored verts + 4 mirrored.
    assert!(app.world().entity(entity).get::<ModifierStack>().is_none());
    let brush = app
        .world()
        .entity(entity)
        .get::<Brush>()
        .expect("brush survives apply");
    assert_eq!(brush.topology.polygons.len(), 11);
    assert_eq!(brush.topology.vertices.len(), 12);
    assert_eq!(brush.faces.len(), 11);

    // Nothing is left to re-mirror.
    tick_brush_meshes(&mut app);
    let cache = cache_of(&app, entity);
    assert_eq!(cache.face_polygons.len(), 11);
    assert!(cache.face_source.is_empty());
    assert!(cache.vert_source.is_empty());
}

#[test]
fn symmetrize_x_bakes_authored_topology() {
    let mut app = util::headless_app();
    app.finish();
    app.update();

    let entity = spawn_half_cube(&mut app);
    select_for_operators(&mut app, entity);

    let result = app
        .world_mut()
        .operator("mesh.symmetrize")
        .call()
        .expect("mesh.symmetrize dispatched");
    assert_eq!(result, OperatorResult::Finished);

    // Half cube occupies x in [0, 1], the default bisect keeps all of it, and
    // the X mirror welds the 4 on-plane verts: 6 + 5 faces, 8 + 4 verts.
    assert!(app.world().entity(entity).get::<ModifierStack>().is_none());
    let brush = app
        .world()
        .entity(entity)
        .get::<Brush>()
        .expect("brush survives symmetrize");
    assert_eq!(brush.topology.polygons.len(), 11);
    assert_eq!(brush.faces.len(), 11);
}

/// Collect each chunk's (`uses_default_material`, current material
/// handle) for the given brush.
fn chunk_materials(app: &mut App, entity: Entity) -> Vec<(bool, Handle<StandardMaterial>)> {
    let chunk_entities = app
        .world()
        .entity(entity)
        .get::<BrushMeshCache>()
        .expect("brush has a BrushMeshCache after regenerate")
        .chunk_entities
        .clone();
    chunk_entities
        .iter()
        .map(|&chunk_entity| {
            let chunk = app
                .world()
                .entity(chunk_entity)
                .get::<BrushMeshChunk>()
                .expect("chunk entity has BrushMeshChunk");
            let mat = app
                .world()
                .entity(chunk_entity)
                .get::<MeshMaterial3d<StandardMaterial>>()
                .expect("chunk entity has MeshMaterial3d");
            (chunk.uses_default_material, mat.0.clone())
        })
        .collect()
}

#[test]
fn xray_overrides_every_chunk_and_restores_on_toggle_off() {
    let mut app = util::headless_app();
    app.finish();
    app.update();

    // The palette is normally built on entering `AppState::Editor`, so run the
    // setup system directly.
    app.world_mut()
        .run_system_cached(setup_default_materials)
        .expect("setup_default_materials ran");

    // A brush with one explicit-material face and five default faces,
    // so both chunk kinds are covered.
    let entity = spawn_half_cube(&mut app);
    let red = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    app.world_mut()
        .entity_mut(entity)
        .get_mut::<Brush>()
        .expect("brush spawned")
        .faces[0]
        .material = red.clone();
    tick_brush_meshes(&mut app);

    let palette_default = app
        .world()
        .resource::<BrushMaterialPalette>()
        .default_material
        .clone();
    let palette_x_ray = app
        .world()
        .resource::<BrushMaterialPalette>()
        .x_ray_material
        .clone();

    let chunks = chunk_materials(&mut app, entity);
    assert_eq!(chunks.len(), 2, "default + explicit material chunks");

    // Every chunk gets the x-ray material; the brush is unselected and not
    // previewed, so never the selected variant.
    app.world_mut().resource_mut::<ViewModeSettings>().x_ray = true;
    app.world_mut()
        .run_system_cached(ensure_brush_chunk_materials)
        .expect("ensure_brush_chunk_materials ran");
    for (_, mat) in chunk_materials(&mut app, entity) {
        assert_eq!(mat, palette_x_ray, "x-ray overrides every chunk");
    }

    let palette_x_ray_selected = app
        .world()
        .resource::<BrushMaterialPalette>()
        .x_ray_selected_material
        .clone();
    app.world_mut()
        .entity_mut(entity)
        .insert(jackdaw::selection::Selected);
    app.world_mut()
        .run_system_cached(ensure_brush_chunk_materials)
        .expect("ensure_brush_chunk_materials ran");
    for (_, mat) in chunk_materials(&mut app, entity) {
        assert_eq!(
            mat, palette_x_ray_selected,
            "selected brush gets the selected x-ray variant"
        );
    }
    app.world_mut()
        .entity_mut(entity)
        .remove::<jackdaw::selection::Selected>();

    // X-ray off: default chunks resume the palette, the explicit chunk
    // restores its recorded build-time material.
    app.world_mut().resource_mut::<ViewModeSettings>().x_ray = false;
    app.world_mut()
        .run_system_cached(ensure_brush_chunk_materials)
        .expect("ensure_brush_chunk_materials ran");
    for (uses_default, mat) in chunk_materials(&mut app, entity) {
        if uses_default {
            assert_eq!(mat, palette_default, "default chunk resumes the palette");
        } else {
            assert_eq!(mat, red, "explicit chunk restores its face material");
        }
    }
}

#[test]
fn symmetrize_x_bakes_prior_live_y_mirror() {
    // Symmetrize on a brush carrying a live mirror on a different axis bakes
    // that mirror first, keeping its geometry.
    let mut app = util::headless_app();
    app.finish();
    app.update();

    let entity = spawn_half_cube(&mut app);
    app.world_mut()
        .entity_mut(entity)
        .insert(mirror_stack(MeshMirror {
            mirror_x: false,
            mirror_y: true,
            mirror_z: false,
            ..MeshMirror::default()
        }));
    select_for_operators(&mut app, entity);

    let result = app
        .world_mut()
        .operator("mesh.symmetrize")
        .call()
        .expect("mesh.symmetrize dispatched");
    assert_eq!(result, OperatorResult::Finished);

    assert!(app.world().entity(entity).get::<ModifierStack>().is_none());
    let brush = app
        .world()
        .entity(entity)
        .get::<Brush>()
        .expect("brush survives symmetrize");

    // 10 faces survive the bisect + cap = 11, then the X mirror skips the
    // all-on-plane cap and doubles the remaining 10: 21.
    assert!(
        brush.topology.polygons.len() > 11,
        "Y-baked geometry must survive the X symmetrize"
    );
    assert_eq!(brush.topology.polygons.len(), 21);
    assert_eq!(brush.faces.len(), 21);
}

/// Read the entity's modifier stack, panicking if absent.
fn stack_of(app: &App, entity: Entity) -> &ModifierStack {
    app.world()
        .entity(entity)
        .get::<ModifierStack>()
        .expect("brush has a ModifierStack")
}

#[test]
fn modifier_add_mirror_matches_standalone_cache() {
    let mut app = util::headless_app();
    app.finish();
    app.update();

    let entity = spawn_half_cube(&mut app);
    select_for_operators(&mut app, entity);

    let result = app
        .world_mut()
        .operator("modifier.add")
        .param("kind", "mirror")
        .call()
        .expect("modifier.add dispatched");
    assert_eq!(result, OperatorResult::Finished);
    tick_brush_meshes(&mut app);

    // The evaluated cache matches the standalone-mirror result: 6 authored
    // faces + 5 mirrored, 8 authored verts + 4 mirrored, non-empty maps.
    let cache = cache_of(&app, entity);
    assert_eq!(cache.face_polygons.len(), 11);
    assert_eq!(cache.vertices.len(), 12);
    assert!(!cache.face_source.is_empty());
    assert!(!cache.vert_source.is_empty());
    assert_eq!(cache.face_to_authored(0), 0);
}

#[test]
fn modifier_toggle_flips_flag() {
    let mut app = util::headless_app();
    app.finish();
    app.update();

    let entity = spawn_half_cube(&mut app);
    app.world_mut()
        .entity_mut(entity)
        .insert(mirror_stack(MeshMirror::default()));
    select_for_operators(&mut app, entity);
    assert!(stack_of(&app, entity).modifiers[0].in_game);

    let result = app
        .world_mut()
        .operator("modifier.toggle")
        .param("index", 0_i64)
        .param("flag", "in_game")
        .call()
        .expect("modifier.toggle dispatched");
    assert_eq!(result, OperatorResult::Finished);

    assert!(!stack_of(&app, entity).modifiers[0].in_game);
}

#[test]
fn modifier_move_reorders() {
    let mut app = util::headless_app();
    app.finish();
    app.update();

    let entity = spawn_half_cube(&mut app);
    app.world_mut().entity_mut(entity).insert(ModifierStack {
        modifiers: vec![
            ModifierEntry::new(Modifier::Mirror(MeshMirror {
                mirror_x: true,
                mirror_y: false,
                mirror_z: false,
                ..MeshMirror::default()
            })),
            ModifierEntry::new(Modifier::Mirror(MeshMirror {
                mirror_x: false,
                mirror_y: true,
                mirror_z: false,
                ..MeshMirror::default()
            })),
        ],
    });
    select_for_operators(&mut app, entity);

    let result = app
        .world_mut()
        .operator("modifier.move_down")
        .param("index", 0_i64)
        .call()
        .expect("modifier.move_down dispatched");
    assert_eq!(result, OperatorResult::Finished);

    let stack = stack_of(&app, entity);
    let Modifier::Mirror(first) = &stack.modifiers[0].modifier;
    let Modifier::Mirror(second) = &stack.modifiers[1].modifier;
    assert!(
        first.mirror_y && !first.mirror_x,
        "Y mirror moved to the top"
    );
    assert!(second.mirror_x && !second.mirror_y, "X mirror moved down");
}

#[test]
fn modifier_apply_bakes_and_removes_entry() {
    let mut app = util::headless_app();
    app.finish();
    app.update();

    let entity = spawn_half_cube(&mut app);
    app.world_mut()
        .entity_mut(entity)
        .insert(mirror_stack(MeshMirror::default()));
    select_for_operators(&mut app, entity);

    let result = app
        .world_mut()
        .operator("modifier.apply")
        .param("index", 0_i64)
        .call()
        .expect("modifier.apply dispatched");
    assert_eq!(result, OperatorResult::Finished);

    let brush = app
        .world()
        .entity(entity)
        .get::<Brush>()
        .expect("brush survives apply");
    assert_eq!(brush.topology.polygons.len(), 11);
    assert_eq!(brush.topology.vertices.len(), 12);
    assert_eq!(brush.faces.len(), 11);
    assert!(app.world().entity(entity).get::<ModifierStack>().is_none());
}

#[test]
fn modifier_remove_restores_base_cache() {
    let mut app = util::headless_app();
    app.finish();
    app.update();

    let entity = spawn_half_cube(&mut app);
    app.world_mut()
        .entity_mut(entity)
        .insert(mirror_stack(MeshMirror::default()));
    select_for_operators(&mut app, entity);
    tick_brush_meshes(&mut app);
    assert_eq!(cache_of(&app, entity).face_polygons.len(), 11);

    let result = app
        .world_mut()
        .operator("modifier.remove")
        .param("index", 0_i64)
        .call()
        .expect("modifier.remove dispatched");
    assert_eq!(result, OperatorResult::Finished);
    tick_brush_meshes(&mut app);

    assert!(app.world().entity(entity).get::<ModifierStack>().is_none());
    let cache = cache_of(&app, entity);
    assert_eq!(cache.face_polygons.len(), 6);
    assert!(cache.face_source.is_empty());
    assert!(cache.vert_source.is_empty());
}

const REFERENCE_IMAGE_TYPE_PATH: &str = "jackdaw::reference_image::ReferenceImage";

/// A `ReferenceImage` component must survive the scene save path
/// (always-save override for the `jackdaw::` skip prefix) and
/// round-trip its fields through reflection.
#[test]
fn reference_image_round_trips_through_scene_document() {
    use jackdaw::reference_image::ReferenceImage;

    let mut app = util::headless_app();
    app.finish();
    app.update();

    let original = ReferenceImage {
        path: "ref.png".to_string(),
        opacity: 0.5,
        locked: true,
    };
    let entity = app
        .world_mut()
        .spawn((
            Name::new("Front Board"),
            original.clone(),
            Transform::default(),
            Visibility::default(),
        ))
        .id();
    jackdaw::scene_io::register_entity_in_ast(app.world_mut(), entity);

    // The document must carry the component patch; convert it back through
    // the same reflection registry the loader uses and compare every field.
    let registry = app.world().resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let ast = app.world().resource::<jackdaw_bsn::SceneBsnAst>();
    let node = ast.ast_for(entity).expect("registered in the document");
    let value = jackdaw_bsn::get_bsn_field(ast, node, REFERENCE_IMAGE_TYPE_PATH, "")
        .expect("ReferenceImage captured into the document");
    let reflected = jackdaw_bsn::bsn_value_to_reflect(
        &value,
        std::any::TypeId::of::<ReferenceImage>(),
        &registry,
        None,
    )
    .expect("convert ReferenceImage back from the document");
    let back = ReferenceImage::from_reflect(reflected.as_ref()).expect("from_reflect");
    assert_eq!(back.path, original.path);
    assert_eq!(back.opacity, original.opacity);
    assert_eq!(back.locked, original.locked);
}

/// An empty-path reference image must get the flat placeholder
/// material and the shared quad mesh from the maintenance system
/// without panicking.
#[test]
fn reference_image_maintenance_installs_placeholder_for_empty_path() {
    use jackdaw::reference_image::{ReferenceImage, maintain_reference_images};

    let mut app = util::headless_app();
    app.finish();
    app.update();

    let entity = app
        .world_mut()
        .spawn((
            Name::new("Reference Image"),
            ReferenceImage::default(),
            Transform::default(),
            Visibility::default(),
        ))
        .id();

    app.world_mut()
        .run_system_cached(maintain_reference_images)
        .expect("maintain_reference_images ran");

    let mesh = app.world().entity(entity).get::<Mesh3d>();
    assert!(mesh.is_some(), "maintenance attaches the shared quad mesh");

    let material_handle = app
        .world()
        .entity(entity)
        .get::<MeshMaterial3d<StandardMaterial>>()
        .expect("maintenance attaches a material")
        .0
        .clone();
    let material = app
        .world()
        .resource::<Assets<StandardMaterial>>()
        .get(&material_handle)
        .expect("placeholder material exists");
    assert!(
        material.base_color_texture.is_none(),
        "empty path renders the flat placeholder, not a texture"
    );
    assert_eq!(material.base_color.alpha(), 0.7, "default opacity applied");
}

/// The maintenance system refreshes the material in-place when the path is
/// already current, leaving `ReferenceImageRuntime` untouched, so a manually
/// set non-uniform scale survives an opacity change.
#[test]
fn reference_image_opacity_change_updates_alpha_without_clobbering_scale() {
    use jackdaw::reference_image::{ReferenceImage, maintain_reference_images};

    let mut app = util::headless_app();
    app.finish();
    app.update();

    let entity = app
        .world_mut()
        .spawn((
            Name::new("Reference Image"),
            ReferenceImage::default(),
            Transform::default(),
            Visibility::default(),
        ))
        .id();

    app.world_mut()
        .run_system_cached(maintain_reference_images)
        .expect("first maintain_reference_images ran");

    // Manually stretch the board along X.
    app.world_mut()
        .entity_mut(entity)
        .get_mut::<Transform>()
        .expect("transform exists")
        .scale
        .x = 3.0;

    // Mutate only opacity; path stays empty (current).
    app.world_mut()
        .entity_mut(entity)
        .get_mut::<ReferenceImage>()
        .expect("reference image component")
        .opacity = 0.4;

    // Two maintenance passes: change detection fires on the first,
    // the decode-poll path runs on the second.
    app.world_mut()
        .run_system_cached(maintain_reference_images)
        .expect("second maintain_reference_images ran");
    app.world_mut()
        .run_system_cached(maintain_reference_images)
        .expect("third maintain_reference_images ran");

    let scale = app
        .world()
        .entity(entity)
        .get::<Transform>()
        .expect("transform still present")
        .scale;
    assert_eq!(
        scale.x, 3.0,
        "manual X scale must not be clobbered by opacity change"
    );

    let material_handle = app
        .world()
        .entity(entity)
        .get::<MeshMaterial3d<StandardMaterial>>()
        .expect("material still attached")
        .0
        .clone();
    let material = app
        .world()
        .resource::<Assets<StandardMaterial>>()
        .get(&material_handle)
        .expect("material exists in Assets");
    assert_eq!(
        material.base_color.alpha(),
        0.4,
        "material alpha must reflect updated opacity"
    );
}
