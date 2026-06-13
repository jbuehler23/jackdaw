//! Headless coverage for modeling essentials: live `MeshMirror`
//! evaluation feeding `BrushMeshCache`, and the x-ray view-mode
//! material swap.
//!
//! The brush mesh systems are gated to `AppState::Editor` in the
//! editor's plugin schedule, so these tests drive them directly with
//! `run_system_cached` (same pattern as the toolbar-highlight tests),
//! preserving the production order: removal marker, then regenerate.

use bevy::prelude::*;
use jackdaw::brush::{
    Brush, BrushMaterialPalette, BrushMeshCache, BrushMeshChunk, MeshMirror,
    ensure_brush_chunk_materials, mark_brushes_changed_on_mirror_removal, regenerate_brush_meshes,
    setup_default_materials,
};
use jackdaw::view_modes::ViewModeSettings;
use jackdaw_api::prelude::*;

mod util;

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
        .run_system_cached(mark_brushes_changed_on_mirror_removal)
        .expect("mirror removal marker ran");
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

#[test]
fn mirrored_brush_cache_holds_both_halves_with_source_maps() {
    let mut app = util::headless_app();
    app.finish();
    app.update();

    let entity = spawn_half_cube(&mut app);
    app.world_mut()
        .entity_mut(entity)
        .insert(MeshMirror::default());
    tick_brush_meshes(&mut app);

    let cache = cache_of(&app, entity);
    // 6 authored faces + 5 mirrored: the -X face lies in the mirror
    // plane (all four verts weld) so it produces no mirrored copy.
    assert_eq!(cache.face_polygons.len(), 11);
    // 8 authored verts + 4 mirrored: the 4 on-plane verts weld.
    assert_eq!(cache.vertices.len(), 12);
    assert!(!cache.face_source.is_empty());
    assert!(!cache.vert_source.is_empty());
    // Mirrored faces map back to an authored face.
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
        .insert(MeshMirror::default());
    tick_brush_meshes(&mut app);

    let cache = cache_of(&app, entity);
    // Every evaluated face maps into the authored range.
    for f in 0..cache.face_polygons.len() {
        assert!(
            cache.face_to_authored(f) < 6,
            "evaluated face {f} must resolve to an authored face"
        );
    }
    // Every evaluated vertex maps into the authored range.
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
        .insert(MeshMirror::default());
    tick_brush_meshes(&mut app);
    assert_eq!(cache_of(&app, entity).face_polygons.len(), 11);

    // Removal alone must trigger the rebuild, within a single
    // marker + regenerate pass; no manual Brush touch.
    app.world_mut().entity_mut(entity).remove::<MeshMirror>();
    tick_brush_meshes(&mut app);

    let cache = cache_of(&app, entity);
    assert_eq!(cache.face_polygons.len(), 6);
    assert!(cache.face_source.is_empty());
    assert!(cache.vert_source.is_empty());
}

/// Select `entity` and clear the headless placeholder `InputFocus`,
/// which the mirror ops' availability checks read as "a text field
/// owns the keyboard" (same setup as the `brush_ops` tests).
fn select_for_operators(app: &mut App, entity: Entity) {
    use bevy::input_focus::InputFocus;
    app.world_mut().resource_mut::<InputFocus>().0 = None;
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
        .0 = None;
    app.update();

    // Empty selection: nothing is available.
    assert_available(&mut app, "mesh.mirror.add", false);
    assert_available(&mut app, "mesh.mirror.apply", false);
    assert_available(&mut app, "mesh.mirror.bisect", false);
    assert_available(&mut app, "mesh.symmetrize", false);

    // A selected brush without a mirror: add / bisect / symmetrize only.
    let entity = spawn_half_cube(&mut app);
    select_for_operators(&mut app, entity);
    assert_available(&mut app, "mesh.mirror.add", true);
    assert_available(&mut app, "mesh.mirror.apply", false);
    assert_available(&mut app, "mesh.mirror.bisect", true);
    assert_available(&mut app, "mesh.symmetrize", true);

    // With a mirror present: apply replaces add / bisect.
    app.world_mut()
        .entity_mut(entity)
        .insert(MeshMirror::default());
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
        .insert(MeshMirror::default());
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
    assert!(app.world().entity(entity).get::<MeshMirror>().is_none());
    let brush = app
        .world()
        .entity(entity)
        .get::<Brush>()
        .expect("brush survives apply");
    assert_eq!(brush.topology.polygons.len(), 11);
    assert_eq!(brush.topology.vertices.len(), 12);
    assert_eq!(brush.faces.len(), 11);

    // The next rebuild renders the baked geometry with no source maps;
    // nothing is left to re-mirror.
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

    // No live mirror remains; the authored topology holds both halves.
    // Half cube occupies x in [0, 1]; the default bisect keeps all of it
    // (positive side). X-mirror welds the 4 on-plane verts (x=0) so the
    // -X face produces no mirrored copy: 6 + 5 = 11 faces, 8 + 4 = 12 verts.
    assert!(app.world().entity(entity).get::<MeshMirror>().is_none());
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

    // The palette is normally built on entering `AppState::Editor`;
    // these tests never leave `ProjectSelect`, so run the setup
    // system directly like the mesh systems below.
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

    // X-ray on: every chunk gets the x-ray material, explicit or not.
    // The brush is unselected and not previewed, so never the
    // selected variant.
    app.world_mut().resource_mut::<ViewModeSettings>().x_ray = true;
    app.world_mut()
        .run_system_cached(ensure_brush_chunk_materials)
        .expect("ensure_brush_chunk_materials ran");
    for (_, mat) in chunk_materials(&mut app, entity) {
        assert_eq!(mat, palette_x_ray, "x-ray overrides every chunk");
    }

    // Selecting the brush switches every chunk to the selected variant.
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
    // Regression for the bug where symmetrize on a brush carrying a
    // live mirror on a different axis silently discarded the mirrored
    // geometry. The fix bakes the existing live mirror first, so the
    // authored topology reflects both the Y-baked half and the X mirror.
    let mut app = util::headless_app();
    app.finish();
    app.update();

    let entity = spawn_half_cube(&mut app);
    app.world_mut().entity_mut(entity).insert(MeshMirror {
        mirror_x: false,
        mirror_y: true,
        mirror_z: false,
        ..MeshMirror::default()
    });
    select_for_operators(&mut app, entity);

    let result = app
        .world_mut()
        .operator("mesh.symmetrize")
        .call()
        .expect("mesh.symmetrize dispatched");
    assert_eq!(result, OperatorResult::Finished);

    assert!(app.world().entity(entity).get::<MeshMirror>().is_none());
    let brush = app
        .world()
        .entity(entity)
        .get::<Brush>()
        .expect("brush survives symmetrize");

    // The Y-baked geometry doubles the face count before the X bisect,
    // so the result holds strictly more faces than the no-live-mirror
    // case (11). The exact count is 21: 10 faces survive the bisect +
    // cap = 11, then X-mirror skips the all-on-plane cap and doubles the
    // remaining 10 faces (11 + 10 = 21).
    assert!(
        brush.topology.polygons.len() > 11,
        "Y-baked geometry must survive the X symmetrize"
    );
    assert_eq!(brush.topology.polygons.len(), 21);
    assert_eq!(brush.faces.len(), 21);
}

const REFERENCE_IMAGE_TYPE_PATH: &str = "jackdaw::reference_image::ReferenceImage";

/// A `ReferenceImage` component must survive the scene save path
/// (always-save override for the `jackdaw::` skip prefix) and
/// round-trip its fields through reflection.
#[test]
fn reference_image_round_trips_through_jsn_scene() {
    use bevy::reflect::serde::{TypedReflectDeserializer, TypedReflectSerializer};
    use jackdaw::reference_image::ReferenceImage;
    use serde::de::DeserializeSeed;

    let mut app = util::headless_app();
    app.finish();
    app.update();

    let original = ReferenceImage {
        path: "ref.png".to_string(),
        opacity: 0.5,
        locked: true,
    };
    app.world_mut().spawn((
        Name::new("Front Board"),
        original.clone(),
        Transform::default(),
        Visibility::default(),
    ));

    let jsn = jackdaw::scene_io::serialize_world_to_jsn_scene(app.world_mut());
    let component_json = jsn
        .scene
        .iter()
        .find_map(|entity| entity.components.get(REFERENCE_IMAGE_TYPE_PATH))
        .expect("ReferenceImage serialized into the scene")
        .clone();

    // Deserialize back through the same reflection registry the scene
    // loader uses and compare every field.
    let registry = app.world().resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    let registration = registry
        .get(std::any::TypeId::of::<ReferenceImage>())
        .expect("ReferenceImage registered");
    let reflected = TypedReflectDeserializer::new(registration, &registry)
        .deserialize(component_json.clone())
        .expect("deserialize ReferenceImage from scene JSON");
    let back = ReferenceImage::from_reflect(reflected.as_partial_reflect()).expect("from_reflect");
    assert_eq!(back.path, original.path);
    assert_eq!(back.opacity, original.opacity);
    assert_eq!(back.locked, original.locked);

    // The serializer output matches its own round-trip input.
    let serializer = TypedReflectSerializer::new(&original, &registry);
    let direct = serde_json::to_value(&serializer).expect("serialize ReferenceImage");
    assert_eq!(direct, component_json);
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

/// Mutating opacity must update the material alpha but must NOT clobber
/// a manually set non-uniform scale. The maintenance system should
/// refresh the material in-place when the path is already current,
/// leaving `ReferenceImageRuntime` (and therefore `aspect_applied`)
/// untouched.
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

    // First maintenance pass: installs the placeholder material and the runtime.
    app.world_mut()
        .run_system_cached(maintain_reference_images)
        .expect("first maintain_reference_images ran");

    // Manually stretch the board along X (simulates an artist dragging the scale gizmo).
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

    // Scale must survive the opacity tweak.
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

    // Material alpha must reflect the new opacity.
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
