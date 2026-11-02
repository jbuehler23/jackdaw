use crate::commands::{
    CommandGroup, CommandHistory, DespawnEntity, EditorCommand, despawn_scene_entity,
};
use crate::draw_brush::{
    ActiveDraw, DrawBrushState, MIN_FRAGMENT_SIZE, cut_brush_from_active, topology_aabbs_overlap,
};
use crate::keybind_focus::KeybindFocus;
use crate::prelude::*;
use crate::selection::{Selected, Selection};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use jackdaw_api_internal::operator::{AvailabilityParams, report_replaced};
use jackdaw_geometry::{Modifier, ModifierStack};
use jackdaw_geometry::{
    clean_degenerate_faces, compute_brush_geometry_from_planes, compute_brush_topology,
};
use jackdaw_scene_types::{
    Brush, BrushFaceData, BrushPlane, BrushTopology, PropertyValue, unsorted_global,
};

struct SubtractionResult {
    original_entity: Entity,
    fragments: Vec<(Brush, Transform)>,
}

/// Spawn each result's fragments beside its original, under the same parent,
/// at the world placement the kernel gave them.
fn spawn_subtract_fragments(world: &mut World, results: &[SubtractionResult]) -> Vec<Entity> {
    let mut created = Vec::new();
    for result in results {
        let parent = world
            .get::<ChildOf>(result.original_entity)
            .and_then(|child_of| unsorted_global(world, child_of.parent()));
        for (brush, transform) in &result.fragments {
            let local = parent.map_or(*transform, |parent| {
                GlobalTransform::from(*transform).reparented_to(&parent)
            });
            match crate::entity_ops::spawn_cloned_brush_with_geometry(
                world,
                result.original_entity,
                brush.clone(),
                local,
            ) {
                Some(entity) => created.push(entity),
                None => warn!(
                    "CSG fragment spawn failed: source {:?} has no document node",
                    result.original_entity
                ),
            }
        }
    }
    created
}

/// Perform CSG subtraction: subtract the drawn solid from all intersecting brushes.
/// Routes through the mesh-CSG kernel so concave targets are handled correctly.
pub(crate) fn subtract_drawn_brush(active: &ActiveDraw, commands: &mut Commands) {
    let Some((cutter_brush, cutter_transform)) = cut_brush_from_active(active) else {
        return;
    };
    let (world_cutter_faces, world_cutter_topo) = jackdaw_csg::brush_to_world(
        &cutter_brush.faces,
        &cutter_brush.topology,
        cutter_transform.compute_affine(),
    );

    // Diagnostic logging for CSG subtract: log cutter geometry so a buggy
    // op can be reconstructed from the log output. Remove this block once
    // the box-cutter bugs are pinned down.
    {
        let bbox_min = world_cutter_topo
            .vertices
            .iter()
            .map(|v| v.position)
            .fold(Vec3::MAX, Vec3::min);
        let bbox_max = world_cutter_topo
            .vertices
            .iter()
            .map(|v| v.position)
            .fold(Vec3::MIN, Vec3::max);
        info!(
            "csg-subtract: cutter faces={} verts={} bbox=({:.4},{:.4},{:.4})..({:.4},{:.4},{:.4})",
            world_cutter_faces.len(),
            world_cutter_topo.vertices.len(),
            bbox_min.x,
            bbox_min.y,
            bbox_min.z,
            bbox_max.x,
            bbox_max.y,
            bbox_max.z,
        );
    }

    commands.queue(move |world: &mut World| {
        // First, collect all brush entities and their data.
        let mut query = world.query::<(Entity, &Brush, &GlobalTransform)>();
        let targets: Vec<(Entity, Brush, GlobalTransform)> = query
            .iter(world)
            .map(|(e, b, gt)| (e, b.clone(), *gt))
            .collect();

        // Second, compute subtractions (pure computation).
        let mut results: Vec<SubtractionResult> = Vec::new();
        let cutter_input = jackdaw_csg::CsgInput::new(&world_cutter_faces, &world_cutter_topo);

        for (entity, brush, global_transform) in &targets {
            // Transform target faces + topology to world space.
            let (world_target_faces, world_target_topo) = jackdaw_csg::brush_to_world(
                &brush.faces,
                &brush.topology,
                global_transform.affine(),
            );

            // Cheap AABB rejection before invoking the kernel. See
            // `topology_aabbs_overlap` above for why we don't use the plane
            // separating-axis test on concave brushes.
            if !topology_aabbs_overlap(&world_target_topo, &world_cutter_topo) {
                continue;
            }

            // Diagnostic: this target survives the cheap rejection and is
            // about to go through the kernel.
            {
                let bbox_min = world_target_topo
                    .vertices
                    .iter()
                    .map(|v| v.position)
                    .fold(Vec3::MAX, Vec3::min);
                let bbox_max = world_target_topo
                    .vertices
                    .iter()
                    .map(|v| v.position)
                    .fold(Vec3::MIN, Vec3::max);
                info!(
                    "csg-subtract: target {:?} faces={} verts={} bbox=({:.4},{:.4},{:.4})..({:.4},{:.4},{:.4})",
                    entity,
                    world_target_faces.len(),
                    world_target_topo.vertices.len(),
                    bbox_min.x,
                    bbox_min.y,
                    bbox_min.z,
                    bbox_max.x,
                    bbox_max.y,
                    bbox_max.z,
                );
            }

            let target_input = jackdaw_csg::CsgInput::new(&world_target_faces, &world_target_topo);
            // `EmptyResult` here means the cutter fully consumed the target;
            // we treat that the same as "no fragments survive" and let the
            // downstream code despawn the original. A kernel error is
            // different: leave the original untouched.
            let raw_fragments =
                match jackdaw_csg::brush_difference_split(&target_input, &cutter_input) {
                    Ok(pieces) => pieces,
                    Err(jackdaw_csg::CsgError::EmptyResult) => {
                        info!("csg-subtract: target {entity:?} fully consumed");
                        Vec::new()
                    }
                    Err(e) => {
                        warn!("box cutter CSG kernel error: {e}");
                        continue;
                    }
                };

            // Diagnostic: how many pieces did the kernel produce, and what's
            // each fragment's bounding box / face count.
            info!(
                "csg-subtract: target {:?} produced {} raw fragment(s)",
                entity,
                raw_fragments.len()
            );
            for (i, frag) in raw_fragments.iter().enumerate() {
                let bbox_min = frag
                    .topology
                    .vertices
                    .iter()
                    .map(|v| v.position)
                    .fold(Vec3::MAX, Vec3::min);
                let bbox_max = frag
                    .topology
                    .vertices
                    .iter()
                    .map(|v| v.position)
                    .fold(Vec3::MIN, Vec3::max);
                info!(
                    "csg-subtract:   fragment {} faces={} verts={} bbox=({:.4},{:.4},{:.4})..({:.4},{:.4},{:.4})",
                    i,
                    frag.faces.len(),
                    frag.topology.vertices.len(),
                    bbox_min.x,
                    bbox_min.y,
                    bbox_min.z,
                    bbox_max.x,
                    bbox_max.y,
                    bbox_max.z,
                );
            }

            let mut fragment_data: Vec<(Brush, Transform)> = Vec::new();
            for fragment in &raw_fragments {
                if fragment.topology.vertices.len() < 4 || fragment.faces.len() < 4 {
                    continue;
                }
                let world_verts: Vec<Vec3> = fragment
                    .topology
                    .vertices
                    .iter()
                    .map(|v| v.position)
                    .collect();
                let bbox_min = world_verts.iter().fold(Vec3::MAX, |a, &b| a.min(b));
                let bbox_max = world_verts.iter().fold(Vec3::MIN, |a, &b| a.max(b));
                let bbox_size = bbox_max - bbox_min;
                if bbox_size.x < MIN_FRAGMENT_SIZE
                    || bbox_size.y < MIN_FRAGMENT_SIZE
                    || bbox_size.z < MIN_FRAGMENT_SIZE
                {
                    continue;
                }
                let centroid: Vec3 = world_verts.iter().sum::<Vec3>() / world_verts.len() as f32;

                // Recentre faces around the centroid.
                let local_faces: Vec<BrushFaceData> = fragment
                    .faces
                    .iter()
                    .map(|f| BrushFaceData {
                        plane: BrushPlane {
                            normal: f.plane.normal,
                            distance: f.plane.distance - f.plane.normal.dot(centroid),
                        },
                        ..f.clone()
                    })
                    .collect();

                if local_faces.len() < 4 {
                    continue;
                }

                // Recentre the topology to match. The mesh-CSG kernel
                // already produces clean, manifold geometry with a
                // matching faces/polygons parallel-array invariant; we
                // intentionally do NOT route through
                // `clean_degenerate_faces` + `compute_brush_topology`
                // here. That convex-era fallback rebuilds topology via
                // triple-plane intersection, which collapses concave
                // brushes (e.g., an L-shape fragment) to a convex hull
                // or empty topology and makes the result vanish.
                let mut local_topo = fragment.topology.clone();
                for v in &mut local_topo.vertices {
                    v.position -= centroid;
                }

                fragment_data.push((
                    Brush {
                        faces: local_faces,
                        topology: local_topo,
                    },
                    Transform::from_translation(centroid),
                ));
            }

            results.push(SubtractionResult {
                original_entity: *entity,
                fragments: fragment_data,
            });
        }

        if results.is_empty() {
            return;
        }

        {
            let despawning: Vec<Entity> = results.iter().map(|r| r.original_entity).collect();
            let mut selection = world.resource_mut::<Selection>();
            selection.entities.retain(|e| !despawning.contains(e));
        }
        for result in &results {
            if let Ok(mut e) = world.get_entity_mut(result.original_entity) {
                e.remove::<Selected>();
            }
        }

        spawn_subtract_fragments(world, &results);

        for result in &results {
            despawn_scene_entity(world, result.original_entity);
        }
    });
}

/// Core logic for Join (convex merge). Callable from both keyboard shortcut and menu.
pub(crate) fn join_selected_brushes_impl(world: &mut World) {
    let candidates: Vec<Entity> = world.resource::<Selection>().entities.clone();
    let mut brush_query = world.query::<&Brush>();
    let selected_brushes: Vec<Entity> = candidates
        .into_iter()
        .filter(|&e| brush_query.get(world, e).is_ok())
        .collect();
    if selected_brushes.len() < 2 {
        return;
    }

    // Bake live mirrors into the authored topology before any vertex is
    // read: the hull must include the mirrored halves, and the baked
    // brushes must not keep a `MeshMirror` that would re-mirror the result.
    for &entity in &selected_brushes {
        crate::brush::topology_ops::mirror_ops::bake_mirror(world, entity);
    }

    // Join (Convex Merge) wraps all selected brushes' vertices in a single
    // convex hull. This is well-defined for both convex and concave inputs:
    // gather every vertex from each brush's topology (rather than
    // re-deriving them from face planes, which was the convex-paradigm path
    // and is undefined for non-convex shapes), then call parry's convex_hull
    // on the combined set.

    let primary_entity = selected_brushes[0];
    let others: Vec<Entity> = selected_brushes[1..].to_vec();

    {
        // Read primary brush data
        let Some(primary_brush) = world.get::<Brush>(primary_entity) else {
            return;
        };
        let old_primary_brush = primary_brush.clone();

        let Some(primary_gtf) = world.get::<GlobalTransform>(primary_entity) else {
            return;
        };
        let (_, rotation, translation) = primary_gtf.to_scale_rotation_translation();
        let inv_rotation = rotation.inverse();

        // Gather every topology vertex from the primary brush, then every
        // topology vertex from each other brush mapped into primary's local
        // space. Using topology vertices (not plane-derived ones) keeps the
        // operation correct for concave inputs.
        let existing_verts: Vec<Vec3> = old_primary_brush
            .topology
            .vertices
            .iter()
            .map(|v| v.position)
            .collect();
        let existing_count = existing_verts.len();
        let mut all_local_verts: Vec<Vec3> = existing_verts;

        for &other in &others {
            let Some(other_brush) = world.get::<Brush>(other) else {
                continue;
            };
            let Some(other_gtf) = world.get::<GlobalTransform>(other) else {
                continue;
            };
            for v in &other_brush.topology.vertices {
                let world_pos = other_gtf.transform_point(v.position);
                all_local_verts.push(inv_rotation * (world_pos - translation));
            }
        }

        if all_local_verts.len() < 4 {
            return;
        }

        let old_face_polygons = compute_brush_geometry_from_planes(&old_primary_brush.faces).1;
        let last_mat = world
            .resource::<crate::brush::LastUsedMaterial>()
            .material
            .clone();
        let Some(new_faces) = jackdaw_hull::build_hull_faces_matching(
            &all_local_verts,
            existing_count,
            &old_primary_brush.faces,
            &old_face_polygons,
            last_mat.unwrap_or_default(),
        ) else {
            return;
        };

        let topology = compute_brush_topology(&new_faces);
        let new_brush = Brush {
            faces: new_faces,
            topology,
        };

        // Snapshot others before despawning (for undo)
        let mut undo_commands: Vec<Box<dyn EditorCommand>> = Vec::new();

        // SetBrush for primary
        undo_commands.push(Box::new(crate::brush::SetBrush {
            entity: primary_entity,
            old: old_primary_brush,
            new: new_brush.clone(),
            label: "Join brushes".to_string(),
        }));

        // Snapshot and despawn each other brush
        for &other in &others {
            undo_commands.push(Box::new(DespawnEntity::from_world(world, other)));
        }

        // Apply: update primary brush (ECS + AST)
        crate::brush::sync_brush_to_ast(world, primary_entity, &new_brush);
        if let Some(mut brush) = world.get_mut::<Brush>(primary_entity) {
            *brush = new_brush;
        }

        // Drop Selected and Selection before despawn so neither holds ids
        // of entities that are about to disappear.
        for &other in &others {
            if let Ok(mut ec) = world.get_entity_mut(other) {
                ec.remove::<Selected>();
            }
        }
        {
            let mut selection = world.resource_mut::<Selection>();
            selection.entities.retain(|e| !others.contains(e));
        }

        for &other in &others {
            despawn_scene_entity(world, other);
        }

        // Push grouped undo command
        let mut history = world.resource_mut::<CommandHistory>();
        history.push_executed(Box::new(CommandGroup {
            commands: undo_commands,
            label: "Join brushes".to_string(),
        }));
    }
}

/// Core logic for CSG Subtract. Selected brushes are cutters, unselected
/// brushes they overlap are targets. `keep_cutters = Some(false)` removes the
/// cutters once something is cut.
pub(crate) fn csg_subtract_selected_impl(world: &mut World, keep_cutters: Option<bool>) {
    let selection = world.resource::<Selection>();
    let selected_set: Vec<Entity> = selection.entities.clone();

    // Bake live mirrors into the authored topology before any geometry is
    // read: cutters unconditionally, mirrored targets only when their
    // evaluated bounds reach a cutter. Baking removes the `MeshMirror`, so
    // the fragments below cannot get re-mirrored by the mesh rebuild.
    crate::brush::topology_ops::mirror_ops::bake_engaged_mirrors(world, &selected_set);

    let mut brush_query = world.query_filtered::<Entity, With<Brush>>();
    let all_brushes: Vec<Entity> = brush_query.iter(world).collect();
    let cutters: Vec<Entity> = all_brushes
        .iter()
        .copied()
        .filter(|e| selected_set.contains(e))
        .collect();
    let targets: Vec<Entity> = all_brushes
        .iter()
        .copied()
        .filter(|e| !selected_set.contains(e))
        .collect();
    if cutters.is_empty() || targets.is_empty() {
        return;
    }

    // Subtract via mesh-CSG (manifold kernel). Works on both convex and
    // concave inputs; a cut that splits a target yields several fragments.
    let cutter_world: Vec<WorldBrush> = cutters
        .iter()
        .filter_map(|&cutter| world_operand(world, cutter))
        .collect();

    let mut results: Vec<SubtractionResult> = Vec::new();
    for &entity in &targets {
        let Some(target) = world_operand(world, entity) else {
            continue;
        };
        // Cheap rejection with topology-vertex AABBs (the plane
        // separating-axis test isn't sound on concave brushes).
        if !cutter_world
            .iter()
            .any(|cutter| topology_aabbs_overlap(&target.topo, &cutter.topo))
        {
            continue;
        }
        let (pieces, _) = subtract_all(&target, &cutter_world);
        let fragments = recentre_fragments(&pieces);
        if fragments.is_empty() {
            continue;
        }
        results.push(SubtractionResult {
            original_entity: entity,
            fragments,
        });
    }
    if results.is_empty() {
        return;
    }

    let mut removed: Vec<Entity> = results.iter().map(|r| r.original_entity).collect();
    if keep_cutters == Some(false) {
        removed.extend(&cutters);
    }
    crate::commands::deselect_entities(world, &removed);
    let created = spawn_subtract_fragments(world, &results);
    for &entity in &removed {
        despawn_scene_entity(world, entity);
    }
    report_replaced(world, &removed, &created);
}

/// Core logic for CSG Intersect. Replaces the selected brushes with the solid
/// all of them share, cloned from the first selected; `keep_cutters =
/// Some(true)` replaces only that first one and keeps the rest.
pub(crate) fn csg_intersect_selected_impl(world: &mut World, keep_cutters: Option<bool>) {
    let selected_set: Vec<Entity> = world.resource::<Selection>().entities.clone();

    // Bake live mirrors into the authored topology before any geometry is
    // read, so the intersection sees both halves; baking removes the
    // `MeshMirror`, which must not survive onto the result brush.
    for &entity in &selected_set {
        crate::brush::topology_ops::mirror_ops::bake_mirror(world, entity);
    }

    let selected_brushes: Vec<Entity> = selected_set
        .iter()
        .copied()
        .filter(|&e| world.get::<Brush>(e).is_some())
        .collect();
    if selected_brushes.len() < 2 {
        return;
    }

    let mut world_inputs = selected_brushes
        .iter()
        .filter_map(|&entity| world_operand(world, entity));
    let Some(first) = world_inputs.next() else {
        return;
    };
    let rest: Vec<WorldBrush> = world_inputs.collect();
    let running = match intersect_all(first, &rest) {
        Ok(Some(running)) => running,
        Ok(None) => return,
        Err(e) => {
            warn!("CSG intersect kernel error: {e}");
            return;
        }
    };
    let Some((new_brush, transform)) = intersection_brush(running) else {
        return;
    };

    let source = selected_brushes[0];
    let removed: Vec<Entity> = if keep_cutters == Some(true) {
        vec![source]
    } else {
        selected_brushes.clone()
    };
    crate::commands::deselect_entities(world, &removed);
    let created = spawn_subtract_fragments(
        world,
        &[SubtractionResult {
            original_entity: source,
            fragments: vec![(new_brush, transform)],
        }],
    );
    for &entity in &removed {
        despawn_scene_entity(world, entity);
    }
    for &entity in &created {
        world.resource_mut::<Selection>().entities.push(entity);
        world.entity_mut(entity).insert(Selected);
    }
    report_replaced(world, &removed, &created);
}

/// A brush's faces and topology in world space, as the CSG kernel takes them.
#[derive(Clone)]
struct WorldBrush {
    faces: Vec<BrushFaceData>,
    topo: BrushTopology,
}

impl WorldBrush {
    fn input(&self) -> jackdaw_csg::CsgInput<'_> {
        jackdaw_csg::CsgInput::new(&self.faces, &self.topo)
    }

    fn from_csg(brush: jackdaw_csg::CsgBrush) -> Self {
        Self {
            faces: brush.faces,
            topo: brush.topology,
        }
    }

    /// The volume the closed solid encloses.
    fn volume(&self) -> f32 {
        let corner = |index: u32| self.topo.vertices.get(index as usize).map(|v| v.position);
        let mut six_times = 0.0;
        for polygon in &self.topo.polygons {
            let start = polygon.loop_start as usize;
            let Some(ring) = self
                .topo
                .loops
                .get(start..start + polygon.loop_total as usize)
            else {
                continue;
            };
            let Some(apex) = ring.first().and_then(|first| corner(first.vert)) else {
                continue;
            };
            for pair in ring[1..].windows(2) {
                if let (Some(b), Some(c)) = (corner(pair[0].vert), corner(pair[1].vert)) {
                    six_times += apex.dot(b.cross(c));
                }
            }
        }
        (six_times / 6.0).abs()
    }
}

/// `target` with every cutter differenced off in turn, as world-space pieces,
/// and the kernel errors met on the way. A piece the kernel fails on is kept
/// whole.
fn subtract_all(
    target: &WorldBrush,
    cutters: &[WorldBrush],
) -> (Vec<WorldBrush>, Vec<jackdaw_csg::CsgError>) {
    let mut current = vec![target.clone()];
    let mut failures = Vec::new();
    for cutter in cutters {
        let mut next: Vec<WorldBrush> = Vec::new();
        for fragment in current {
            if !topology_aabbs_overlap(&fragment.topo, &cutter.topo) {
                next.push(fragment);
                continue;
            }
            match jackdaw_csg::brush_difference_split(&fragment.input(), &cutter.input()) {
                Ok(pieces) => next.extend(pieces.into_iter().map(WorldBrush::from_csg)),
                Err(jackdaw_csg::CsgError::EmptyResult) => {}
                Err(e) => {
                    warn!("CSG subtract kernel error: {e}");
                    failures.push(e);
                    next.push(fragment);
                }
            }
        }
        current = next;
    }
    (current, failures)
}

/// The parts of `target` inside any of `cutters`, one piece per connected
/// part.
fn inside_any(
    target: &WorldBrush,
    cutters: &[WorldBrush],
) -> Result<Vec<WorldBrush>, jackdaw_csg::CsgError> {
    let union = match cutters {
        [] => return Ok(Vec::new()),
        [only] => only.clone(),
        many => {
            let inputs: Vec<jackdaw_csg::CsgInput<'_>> =
                many.iter().map(WorldBrush::input).collect();
            WorldBrush::from_csg(jackdaw_csg::brush_batch_union(&inputs)?)
        }
    };
    match jackdaw_csg::brush_intersection_split(&target.input(), &union.input()) {
        Ok(pieces) => Ok(pieces.into_iter().map(WorldBrush::from_csg).collect()),
        Err(jackdaw_csg::CsgError::EmptyResult) => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Each world-space piece as a brush centred on its own corners, with the
/// translation that puts it back. Pieces too thin to be a solid are dropped.
///
/// The kernel's topology is used as it comes: rebuilding it from the planes
/// would collapse a concave fragment to its convex hull.
fn recentre_fragments(pieces: &[WorldBrush]) -> Vec<(Brush, Transform)> {
    pieces
        .iter()
        .filter(|piece| piece.topo.vertices.len() >= 4 && piece.faces.len() >= 4)
        .map(|piece| {
            let centroid: Vec3 = piece.topo.vertices.iter().map(|v| v.position).sum::<Vec3>()
                / piece.topo.vertices.len() as f32;
            let faces = piece
                .faces
                .iter()
                .map(|f| BrushFaceData {
                    plane: BrushPlane {
                        normal: f.plane.normal,
                        distance: f.plane.distance - f.plane.normal.dot(centroid),
                    },
                    ..f.clone()
                })
                .collect();
            let mut topology = piece.topo.clone();
            for v in &mut topology.vertices {
                v.position -= centroid;
            }
            (
                Brush { faces, topology },
                Transform::from_translation(centroid),
            )
        })
        .collect()
}

/// The solid `first` shares with every one of `rest`; `None` when they share
/// nothing.
fn intersect_all(
    first: WorldBrush,
    rest: &[WorldBrush],
) -> Result<Option<WorldBrush>, jackdaw_csg::CsgError> {
    let mut running = first;
    for next in rest {
        match jackdaw_csg::brush_boolean(
            &running.input(),
            &next.input(),
            jackdaw_csg::BooleanOp::Intersection,
        ) {
            Ok(shared) => running = WorldBrush::from_csg(shared),
            Err(jackdaw_csg::CsgError::EmptyResult) => return Ok(None),
            Err(e) => return Err(e),
        }
    }
    Ok(Some(running))
}

/// An intersection as one recentred brush with degenerate faces dropped;
/// `None` when too little of it is left to be a solid.
fn intersection_brush(running: WorldBrush) -> Option<(Brush, Transform)> {
    let (brush, transform) = recentre_fragments(std::slice::from_ref(&running))
        .into_iter()
        .next()?;
    let clean = clean_degenerate_faces(&brush.faces);
    if clean.len() < 4 {
        return None;
    }
    let topology = if clean.len() == brush.faces.len() {
        brush.topology
    } else {
        compute_brush_topology(&clean)
    };
    Some((
        Brush {
            faces: clean,
            topology,
        },
        transform,
    ))
}

/// The operands a call named for subtract or intersect, checked.
struct NamedOperands {
    target: Entity,
    /// The node `cutters` named, which `keep_cutters = false` removes.
    cutters_node: Entity,
    /// The brushes that cut: the named one, or every brush under the named
    /// group.
    cutters: Vec<Entity>,
    keep_cutters: bool,
}

/// The parameters that name operands rather than work on the selection.
const OPERAND_KEYS: [&str; 2] = ["target", "cutters"];

impl NamedOperands {
    /// The operands `params` names, `None` when it names none and the call
    /// works on the selection.
    fn read(
        world: &World,
        params: &OperatorParameters,
        keep_cutters: Option<bool>,
    ) -> Result<Option<Self>, String> {
        if !OPERAND_KEYS.iter().any(|key| params.contains_key(*key)) {
            return Ok(None);
        }
        let target = operand_node(world, params, "target")?;
        if world.get::<Brush>(target).is_none() {
            return Err(format!("`target` {target} is not a brush"));
        }
        let cutters_node = operand_node(world, params, "cutters")?;
        if is_at_or_under(world, cutters_node, target) {
            return Err(format!(
                "`cutters` {cutters_node} is the target or sits under it"
            ));
        }
        let cutters = brushes_under(world, cutters_node);
        if cutters.is_empty() {
            return Err(format!(
                "`cutters` {cutters_node} is neither a brush nor a group holding brushes"
            ));
        }
        if cutters.contains(&target) {
            return Err(format!("`target` {target} is also one of `cutters`"));
        }
        Ok(Some(Self {
            target,
            cutters_node,
            cutters,
            keep_cutters: keep_cutters.unwrap_or(true),
        }))
    }
}

/// `keep_cutters` as the call gave it, `None` when it did not.
fn keep_cutters_param(params: &OperatorParameters) -> Result<Option<bool>, String> {
    match params.get("keep_cutters") {
        None => Ok(None),
        Some(_) => params
            .as_bool("keep_cutters")
            .map(Some)
            .ok_or_else(|| "`keep_cutters` must be true or false".to_string()),
    }
}

fn operand_node(world: &World, params: &OperatorParameters, key: &str) -> Result<Entity, String> {
    match params.get(key) {
        Some(PropertyValue::Entity(entity)) => {
            crate::primitive_ops::scene_node(world, *entity, key)
        }
        Some(other) => Err(format!("`{key}` {other:?} names no node in this scene")),
        None => Err(format!(
            "name both `target` and `cutters`; `{key}` is missing"
        )),
    }
}

/// Whether `node` is `ancestor` or sits somewhere under it.
fn is_at_or_under(world: &World, node: Entity, ancestor: Entity) -> bool {
    std::iter::successors(Some(node), |&at| {
        world.get::<ChildOf>(at).map(ChildOf::parent)
    })
    .any(|at| at == ancestor)
}

/// `node` and every scene node under it that is a brush.
fn brushes_under(world: &World, node: Entity) -> Vec<Entity> {
    let Some(document) = world.get_resource::<jackdaw_bsn::SceneBsnAst>() else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let mut stack = vec![node];
    while let Some(at) = stack.pop() {
        if world.get::<Brush>(at).is_some() && document.ast_for(at).is_some() {
            found.push(at);
        }
        if let Some(children) = world.get::<Children>(at) {
            stack.extend(children.iter());
        }
    }
    found
}

/// `entity`'s brush in world space with its modifier stack evaluated, read
/// without touching the scene.
fn world_operand(world: &World, entity: Entity) -> Option<WorldBrush> {
    let mut brush = world.get::<Brush>(entity)?.clone();
    if let Some(stack) = world.get::<ModifierStack>(entity) {
        let modifiers: Vec<&Modifier> = stack.modifiers.iter().map(|e| &e.modifier).collect();
        crate::modifier_ops::bake_modifier_stack(&mut brush, &modifiers);
    }
    let global = unsorted_global(world, entity)?;
    let (faces, topo) = jackdaw_csg::brush_to_world(&brush.faces, &brush.topology, global.affine());
    Some(WorldBrush { faces, topo })
}

#[derive(Clone, Copy)]
enum CsgOp {
    Subtract,
    Intersect,
}

impl CsgOp {
    fn label(self) -> &'static str {
        match self {
            Self::Subtract => "CSG Subtract",
            Self::Intersect => "CSG Intersect",
        }
    }
}

/// End a call that leaves the scene as it was, telling the caller why.
fn refuse(world: &mut World, op: CsgOp, outcome: Outcome, why: impl AsRef<str>) -> OperatorResult {
    warn_caller(world, format!("{}: {}", op.label(), why.as_ref()));
    set_outcome(world, outcome);
    OperatorResult::Cancelled
}

/// Run `op` from its parameters: on the operands they name, or on the
/// selection when they name none.
fn run_csg(world: &mut World, op: CsgOp, params: &OperatorParameters) -> OperatorResult {
    let keep_cutters = match keep_cutters_param(params) {
        Ok(keep) => keep,
        Err(why) => return refuse(world, op, Outcome::Invalid, why),
    };
    match NamedOperands::read(world, params, keep_cutters) {
        Ok(Some(operands)) => run_on_named_operands(world, op, &operands),
        Ok(None) => {
            match op {
                CsgOp::Subtract => csg_subtract_selected_impl(world, keep_cutters),
                CsgOp::Intersect => csg_intersect_selected_impl(world, keep_cutters),
            }
            OperatorResult::Finished
        }
        Err(why) => refuse(world, op, Outcome::Invalid, why),
    }
}

/// Run `op` on the operands a call named: only the target is replaced, by
/// what is left of it, and is removed when nothing is. Everything is computed
/// before the scene is touched, so a call that changes nothing leaves it as it
/// was.
fn run_on_named_operands(world: &mut World, op: CsgOp, operands: &NamedOperands) -> OperatorResult {
    let Some(target) = world_operand(world, operands.target) else {
        let why = format!("`target` {} has no transform", operands.target);
        return refuse(world, op, Outcome::Invalid, why);
    };
    let reaching: Vec<WorldBrush> = operands
        .cutters
        .iter()
        .filter_map(|&cutter| world_operand(world, cutter))
        .filter(|cutter| topology_aabbs_overlap(&target.topo, &cutter.topo))
        .collect();

    let pieces = match op {
        CsgOp::Subtract => {
            if reaching.is_empty() {
                return refuse(
                    world,
                    op,
                    Outcome::Unchanged,
                    "the cutters do not reach the target",
                );
            }
            let (pieces, failures) = subtract_all(&target, &reaching);
            if let Some(e) = failures.first() {
                let why = format!("the kernel failed, so nothing was cut: {e}");
                return refuse(world, op, Outcome::Unchanged, why);
            }
            pieces
        }
        CsgOp::Intersect => match inside_any(&target, &reaching) {
            Ok(pieces) => pieces,
            Err(e) => {
                let why = format!("the kernel failed, so nothing was trimmed: {e}");
                return refuse(world, op, Outcome::Unchanged, why);
            }
        },
    };
    let kept: f32 = pieces.iter().map(WorldBrush::volume).sum();
    let whole = target.volume();
    if !pieces.is_empty() && (kept - whole).abs() <= whole.max(1e-6) * 1e-4 {
        return refuse(
            world,
            op,
            Outcome::Unchanged,
            "the cutters leave the target as it is",
        );
    }

    replace_target(world, operands, recentre_fragments(&pieces))
}

/// Put `fragments` in place of the named target, moving the target's other
/// children up to its parent so they outlive it, and remove the cutters when
/// the call asked.
fn replace_target(
    world: &mut World,
    operands: &NamedOperands,
    fragments: Vec<(Brush, Transform)>,
) -> OperatorResult {
    let target = operands.target;
    crate::brush::topology_ops::mirror_ops::bake_mirror(world, target);
    let mut removed = vec![target];
    if !operands.keep_cutters {
        removed.push(operands.cutters_node);
    }
    crate::commands::deselect_entities(world, &removed);

    let created = spawn_subtract_fragments(
        world,
        &[SubtractionResult {
            original_entity: target,
            fragments,
        }],
    );
    for &entity in &created {
        crate::commands::SpawnedEntities::record(world, entity);
    }

    let parent = world.get::<ChildOf>(target).map(ChildOf::parent);
    let children: Vec<Entity> = world
        .get::<Children>(target)
        .map(|children| children.iter().collect())
        .unwrap_or_default();
    let document = world.resource::<jackdaw_bsn::SceneBsnAst>();
    let adopted: Vec<Entity> = children
        .into_iter()
        .filter(|&child| document.ast_for(child).is_some())
        .collect();
    for child in adopted {
        crate::commands::set_hierarchy_location(
            world,
            child,
            crate::commands::HierarchyLocation {
                parent,
                index: usize::MAX,
            },
        );
    }

    for &entity in &removed {
        despawn_scene_entity(world, entity);
    }
    report_replaced(world, &removed, &created);
    set_outcome(
        world,
        if created.is_empty() {
            Outcome::Empty
        } else {
            Outcome::Changed
        },
    );
    OperatorResult::Finished
}

#[operator(
    id = "brush.join",
    label = "Join (Convex Merge)",
    description = "Merge all selected brushes into a single convex-hull brush. \
                   Requires at least two `Brush` entities in \
                   `Selection::entities`; availability (`can_run_binary_brush_op`) \
                   is false otherwise. The first selected brush keeps its entity \
                   id and transform; others are despawned.",
    is_available = can_run_binary_brush_op,
)]
pub(crate) fn brush_join(_: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    commands.queue(join_selected_brushes_impl);
    OperatorResult::Finished
}

#[operator(
    id = "brush.csg_subtract",
    label = "CSG Subtract",
    description = "Cut brushes away from other brushes. Naming `target` and `cutters` works \
                   whatever is selected and cuts only the target: it is replaced by its \
                   fragments, or removed when nothing is left, and its other children move up \
                   to its parent. Without them, the selected brushes are the cutters and every \
                   unselected brush they overlap is a target; at least one brush must be \
                   selected.",
    is_available = can_run_csg_subtract,
    params(
        target(Entity, doc = "Brush to cut. Every other brush is left alone."),
        cutters(
            Entity,
            doc = "Brush to cut with, or a group whose brushes all cut."
        ),
        keep_cutters(
            bool,
            doc = "Keep the cutters after the cut. Defaults to true."
        ),
    )
)]
pub(crate) fn brush_csg_subtract(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    run_csg(world, CsgOp::Subtract, &params)
}

#[operator(
    id = "brush.csg_intersect",
    label = "CSG Intersect",
    description = "Keep only the solid brushes share. Naming `target` and `cutters` works \
                   whatever is selected and trims only the target: it is replaced by its parts \
                   inside any of the cutters, or removed when there are none, and its other \
                   children move up to its parent. Without them, the selected brushes are \
                   replaced by the solid all of them share; at least two brushes must be \
                   selected.",
    is_available = can_run_csg_intersect,
    params(
        target(Entity, doc = "Brush to trim. Every other brush is left alone."),
        cutters(
            Entity,
            doc = "Brush to trim to, or a group whose brushes together make the shape kept."
        ),
        keep_cutters(
            bool,
            doc = "Keep the cutters after the trim. Defaults to true with `target` and \
                   `cutters`; on the selection, true replaces only the first selected brush."
        ),
    )
)]
pub(crate) fn brush_csg_intersect(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    run_csg(world, CsgOp::Intersect, &params)
}

/// Shared environment gate: brush-level operators never run mid-draw,
/// mid-modal, or while a text input is focused. Each specific op
/// composes this with its own selection-state precondition check.
pub(crate) fn env_allows_brush_op(
    keybind_focus: &KeybindFocus,
    modal: &crate::modal_transform::ModalTransformState,
    draw_state: &DrawBrushState,
) -> bool {
    !keybind_focus.keyboard_is_spoken_for() && modal.active.is_none() && draw_state.active.is_none()
}

/// What the join and CSG gates read.
#[derive(SystemParam)]
struct BrushOpGate<'w, 's> {
    keybind_focus: KeybindFocus<'w, 's>,
    modal: Res<'w, crate::modal_transform::ModalTransformState>,
    draw_state: Res<'w, DrawBrushState>,
    selection: Res<'w, Selection>,
    brushes: Query<'w, 's, (), With<Brush>>,
    viewport: crate::viewport_2d::FrontedViewport<'w, 's>,
    call: Option<Res<'w, AvailabilityParams>>,
}

impl BrushOpGate<'_, '_> {
    /// Whether the editor is free for a brush op and either at least `min`
    /// brushes are selected in a 3D view or, for an op that `takes_operands`,
    /// the call names its own, which the op then checks itself.
    fn allows(&self, min: usize, takes_operands: bool) -> bool {
        if !env_allows_brush_op(&self.keybind_focus, &self.modal, &self.draw_state) {
            return false;
        }
        let names_operands = takes_operands
            && self
                .call
                .as_ref()
                .is_some_and(|call| OPERAND_KEYS.iter().any(|key| call.0.contains_key(*key)));
        let selected = self
            .selection
            .entities
            .iter()
            .filter(|&&e| self.brushes.contains(e))
            .count();
        names_operands || (self.viewport.is_three_d() && selected >= min)
    }
}

fn can_run_binary_brush_op(gate: BrushOpGate) -> bool {
    gate.allows(2, false)
}

fn can_run_csg_subtract(gate: BrushOpGate) -> bool {
    gate.allows(1, true)
}

fn can_run_csg_intersect(gate: BrushOpGate) -> bool {
    gate.allows(2, true)
}
