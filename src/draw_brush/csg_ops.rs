use crate::commands::{
    CommandGroup, CommandHistory, DespawnEntity, EditorCommand, collect_entity_ids,
    deselect_entities,
};
use crate::draw_brush::{
    ActiveDraw, BrushData, BrushOrGroup, BrushStableId, DrawBrushState, MIN_FRAGMENT_SIZE,
    PUNCH_THROUGH_DEPTH, StableIdCounter, brush_data_from_entity, build_cutter_planes,
    build_cutter_planes_polygon, entity_by_stable_id, spawn_brush_from_data, spawn_brush_or_group,
    topology_aabbs_overlap,
};
use crate::keybind_focus::KeybindFocus;
use crate::prelude::*;
use crate::selection::{Selected, Selection};
use bevy::prelude::*;
use jackdaw_geometry::{
    clean_degenerate_faces, compute_brush_geometry_from_planes, compute_brush_topology,
};
use jackdaw_jsn::{Brush, BrushFaceData, BrushGroup, BrushPlane};

/// If `entity` is a child of a `BrushGroup`, return (`parent_entity`, `parent_translation`).
pub(crate) fn brush_parent_group(world: &World, entity: Entity) -> Option<(Entity, Vec3)> {
    let parent = world.get::<ChildOf>(entity)?.0;
    world.get::<BrushGroup>(parent)?;
    let translation = world.get::<GlobalTransform>(parent)?.translation();
    Some((parent, translation))
}

/// Perform CSG subtraction: subtract the drawn cuboid from all intersecting brushes.
/// Routes through the mesh-CSG kernel so concave targets are handled correctly.
pub(crate) fn subtract_drawn_brush(active: &ActiveDraw, commands: &mut Commands) {
    // Box-cut always punches through: extend the cutter far into the brush
    // along the inward normal so it traverses any reasonably-sized target.
    // The face plane the user clicked is the cutter's near cap; the far cap
    // is `PUNCH_THROUGH_DEPTH` behind it (into the brush). The user's drag
    // for depth is ignored here, matching BoxCutter's default behavior.
    let mut punch_active = active.clone();
    punch_active.depth = -PUNCH_THROUGH_DEPTH;

    let cutter_planes = if punch_active.polygon_vertices.is_empty() {
        build_cutter_planes(&punch_active)
    } else {
        build_cutter_planes_polygon(&punch_active)
    };
    let cutter_topology = compute_brush_topology(&cutter_planes);

    // Diagnostic logging for CSG subtract: log cutter geometry so a buggy
    // op can be reconstructed from the log output. Remove this block once
    // the box-cutter bugs are pinned down.
    {
        let bbox_min = cutter_topology
            .vertices
            .iter()
            .map(|v| v.position)
            .fold(Vec3::MAX, Vec3::min);
        let bbox_max = cutter_topology
            .vertices
            .iter()
            .map(|v| v.position)
            .fold(Vec3::MIN, Vec3::max);
        info!(
            "csg-subtract: cutter faces={} verts={} bbox=({:.4},{:.4},{:.4})..({:.4},{:.4},{:.4})",
            cutter_planes.len(),
            cutter_topology.vertices.len(),
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
        struct SubtractionResult {
            original_entity: Entity,
            fragments: Vec<(Brush, Transform)>,
        }

        let mut results: Vec<SubtractionResult> = Vec::new();
        let cutter_input = jackdaw_csg::CsgInput::new(&cutter_planes, &cutter_topology);

        for (entity, brush, global_transform) in &targets {
            // Transform target faces + topology to world space.
            let (_, rotation, translation) = global_transform.to_scale_rotation_translation();
            let (world_target_faces, world_target_topo) =
                jackdaw_csg::brush_to_world(&brush.faces, &brush.topology, rotation, translation);

            // Cheap AABB rejection before invoking the kernel. See
            // `topology_aabbs_overlap` above for why we don't use the plane
            // separating-axis test on concave brushes.
            if !topology_aabbs_overlap(&world_target_topo, &cutter_topology) {
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

        // Third, capture brush data for originals (assigns stable IDs).
        let mut originals: Vec<BrushData> = Vec::new();
        for result in &results {
            originals.push(brush_data_from_entity(world, result.original_entity));
        }

        // Capture parent group info before despawning originals
        // Now using stable IDs: (original_entity -> (parent_stable_id, parent_translation))
        let mut parent_groups: std::collections::HashMap<Entity, (BrushStableId, Vec3)> =
            std::collections::HashMap::new();
        for result in &results {
            if let Some((parent_entity, parent_translation)) =
                brush_parent_group(world, result.original_entity)
            {
                // Ensure the parent group has a stable ID
                let parent_sid = if let Some(sid) = world.get::<BrushStableId>(parent_entity) {
                    *sid
                } else {
                    let sid = world.resource_mut::<StableIdCounter>().next();
                    world.entity_mut(parent_entity).insert(sid);
                    sid
                };
                parent_groups.insert(result.original_entity, (parent_sid, parent_translation));
            }
        }

        // Clean up selection: remove originals that are about to be despawned
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

        // Despawn originals
        for result in &results {
            if let Ok(e) = world.get_entity_mut(result.original_entity) {
                e.despawn();
            }
        }

        // Spawn fragments and build BrushOrGroup data
        let mut fragments: Vec<BrushOrGroup> = Vec::new();
        let mut counter = world.resource_mut::<StableIdCounter>();
        // Pre-allocate stable IDs for all new fragments
        let fragment_stable_ids: Vec<Vec<BrushStableId>> = results
            .iter()
            .map(|r| r.fragments.iter().map(|_| counter.next()).collect())
            .collect();
        let group_stable_ids: Vec<Option<BrushStableId>> = results
            .iter()
            .map(|r| {
                if r.fragments.len() > 1 && !parent_groups.contains_key(&r.original_entity) {
                    Some(counter.next())
                } else {
                    None
                }
            })
            .collect();

        for (result_idx, result) in results.iter().enumerate() {
            if let Some(&(parent_sid, parent_translation)) =
                parent_groups.get(&result.original_entity)
            {
                // Fragments stay in existing parent group
                for (frag_idx, (brush, transform)) in result.fragments.iter().enumerate() {
                    let brush_data = BrushData {
                        stable_id: fragment_stable_ids[result_idx][frag_idx],
                        brush: brush.clone(),
                        transform: Transform::from_translation(
                            transform.translation - parent_translation,
                        ),
                        name: "Brush".to_string(),
                        parent_stable_id: Some(parent_sid),
                    };
                    spawn_brush_from_data(world, &brush_data);
                    fragments.push(BrushOrGroup::Single(Box::new(brush_data)));
                }
            } else if result.fragments.len() == 1 {
                // Single fragment: spawn standalone
                let (brush, transform) = &result.fragments[0];
                let brush_data = BrushData {
                    stable_id: fragment_stable_ids[result_idx][0],
                    brush: brush.clone(),
                    transform: *transform,
                    name: "Brush".to_string(),
                    parent_stable_id: None,
                };
                spawn_brush_from_data(world, &brush_data);
                fragments.push(BrushOrGroup::Single(Box::new(brush_data)));
            } else if result.fragments.len() > 1 {
                // Multiple fragments: group under a BrushGroup parent
                let group_center = result
                    .fragments
                    .iter()
                    .map(|(_, tf)| tf.translation)
                    .sum::<Vec3>()
                    / result.fragments.len() as f32;

                let group_sid = group_stable_ids[result_idx].unwrap();
                let children: Vec<BrushData> = result
                    .fragments
                    .iter()
                    .enumerate()
                    .map(|(frag_idx, (brush, transform))| BrushData {
                        stable_id: fragment_stable_ids[result_idx][frag_idx],
                        brush: brush.clone(),
                        transform: Transform::from_translation(
                            transform.translation - group_center,
                        ),
                        name: "Brush".to_string(),
                        parent_stable_id: None, // filled in by spawn_brush_or_group
                    })
                    .collect();

                let group_data = BrushOrGroup::Group {
                    stable_id: group_sid,
                    transform: Transform::from_translation(group_center),
                    name: "Brush Group".to_string(),
                    parent_stable_id: None,
                    children,
                };
                spawn_brush_or_group(world, &group_data);
                fragments.push(group_data);
            }
        }

        // Push undo command
        let cmd = SubtractBrushCommand {
            originals,
            fragments,
        };
        let mut history = world.resource_mut::<CommandHistory>();
        history.push_executed(Box::new(cmd));
    });
}

struct SubtractBrushCommand {
    originals: Vec<BrushData>,
    fragments: Vec<BrushOrGroup>,
}

impl SubtractBrushCommand {
    /// Resolve the stable ID of a `BrushOrGroup` to its current entity.
    fn fragment_stable_id(data: &BrushOrGroup) -> BrushStableId {
        match data {
            BrushOrGroup::Single(d) => d.stable_id,
            BrushOrGroup::Group { stable_id, .. } => *stable_id,
        }
    }
}

impl EditorCommand for SubtractBrushCommand {
    fn execute(&mut self, world: &mut World) {
        // Despawn originals by stable ID lookup
        let orig_entities: Vec<Entity> = self
            .originals
            .iter()
            .filter_map(|d| entity_by_stable_id(world, d.stable_id))
            .collect();
        deselect_entities(world, &orig_entities);
        for entity in &orig_entities {
            if let Ok(e) = world.get_entity_mut(*entity) {
                e.despawn();
            }
        }
        // Spawn fragments (stable IDs are reassigned from stored data)
        for data in &self.fragments {
            spawn_brush_or_group(world, data);
        }
    }

    fn undo(&mut self, world: &mut World) {
        // Despawn fragments by stable ID lookup
        let mut all_entities = Vec::new();
        for data in &self.fragments {
            let sid = Self::fragment_stable_id(data);
            if let Some(entity) = entity_by_stable_id(world, sid) {
                collect_entity_ids(world, entity, &mut all_entities);
            }
        }
        deselect_entities(world, &all_entities);
        for data in &self.fragments {
            let sid = Self::fragment_stable_id(data);
            if let Some(entity) = entity_by_stable_id(world, sid)
                && let Ok(e) = world.get_entity_mut(entity)
            {
                e.despawn();
            }
        }
        // Respawn originals (stable IDs are reassigned from stored data)
        for data in &self.originals {
            spawn_brush_from_data(world, data);
        }
    }

    fn description(&self) -> &str {
        "Subtract brush"
    }
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
    // we simply gather every vertex from each brush's topology (rather than
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

        // Deselect entities before despawning so that `On<Remove, Selected>`
        // observers can clean up tree-row UI while the entities still exist.
        for &other in &others {
            if let Ok(mut ec) = world.get_entity_mut(other) {
                ec.remove::<Selected>();
            }
        }
        {
            let mut selection = world.resource_mut::<Selection>();
            selection.entities.retain(|e| !others.contains(e));
        }

        // Despawn others
        for &other in &others {
            if let Ok(entity_mut) = world.get_entity_mut(other) {
                entity_mut.despawn();
            }
        }

        // Push grouped undo command
        let mut history = world.resource_mut::<CommandHistory>();
        history.push_executed(Box::new(CommandGroup {
            commands: undo_commands,
            label: "Join brushes".to_string(),
        }));
    }
}

/// Core logic for CSG Subtract. Selected brushes are cutters, non-selected are targets.
pub(crate) fn csg_subtract_selected_impl(world: &mut World) {
    let selection = world.resource::<Selection>();
    let selected_set: Vec<Entity> = selection.entities.clone();

    // Bake live mirrors into the authored topology before any geometry is
    // read: cutters unconditionally, mirrored targets only when their
    // evaluated bounds reach a cutter. Baking removes the `MeshMirror`, so
    // the fragments below cannot get re-mirrored by the mesh rebuild.
    crate::brush::topology_ops::mirror_ops::bake_engaged_mirrors(world, &selected_set);

    let mut brush_query = world.query::<(Entity, &Brush, &GlobalTransform)>();
    let all_brushes: Vec<(Entity, Brush, GlobalTransform)> = brush_query
        .iter(world)
        .map(|(e, b, gt)| (e, b.clone(), *gt))
        .collect();

    // Cutters = selected brushes, targets = non-selected brushes
    let cutters: Vec<&(Entity, Brush, GlobalTransform)> = all_brushes
        .iter()
        .filter(|(e, _, _)| selected_set.contains(e))
        .collect();
    let targets: Vec<&(Entity, Brush, GlobalTransform)> = all_brushes
        .iter()
        .filter(|(e, _, _)| !selected_set.contains(e))
        .collect();

    if cutters.is_empty() || targets.is_empty() {
        return;
    }

    // Subtract via mesh-CSG (manifold kernel). Works on both convex and
    // concave inputs. For each target, every cutter is differenced off
    // iteratively; the kernel handles cuts that split the target into
    // multiple disconnected fragments via `brush_difference_split`.

    // Transform every cutter into world space once (faces + topology).
    let cutter_world: Vec<(Vec<BrushFaceData>, jackdaw_jsn::BrushTopology)> = cutters
        .iter()
        .map(|(_, brush, gt)| {
            let (_, rotation, translation) = gt.to_scale_rotation_translation();
            jackdaw_csg::brush_to_world(&brush.faces, &brush.topology, rotation, translation)
        })
        .collect();

    struct SubtractionResult {
        original_entity: Entity,
        fragments: Vec<(Brush, Transform)>,
    }

    let mut results: Vec<SubtractionResult> = Vec::new();

    for (entity, brush, global_transform) in &targets {
        let entity = *entity;
        let (_, rotation, translation) = global_transform.to_scale_rotation_translation();
        let (target_world_faces, target_world_topo) =
            jackdaw_csg::brush_to_world(&brush.faces, &brush.topology, rotation, translation);

        // Cheap rejection: if no cutter's AABB even touches the target's,
        // skip the whole op. Mesh-CSG would handle it correctly but the
        // convert-and-decompose round-trip isn't free. Use topology-vertex
        // AABBs (the plane separating-axis test isn't sound on concave
        // brushes).
        let any_cutter_touches = cutter_world
            .iter()
            .any(|(_, ct)| topology_aabbs_overlap(&target_world_topo, ct));
        if !any_cutter_touches {
            continue;
        }

        // Iteratively subtract each cutter from the fragment list.
        struct WorldBrush {
            faces: Vec<BrushFaceData>,
            topo: jackdaw_jsn::BrushTopology,
        }
        let mut current: Vec<WorldBrush> = vec![WorldBrush {
            faces: target_world_faces,
            topo: target_world_topo,
        }];
        for (cutter_faces, cutter_topo) in &cutter_world {
            let mut next: Vec<WorldBrush> = Vec::new();
            for fragment in &current {
                if !topology_aabbs_overlap(&fragment.topo, cutter_topo) {
                    next.push(WorldBrush {
                        faces: fragment.faces.clone(),
                        topo: fragment.topo.clone(),
                    });
                    continue;
                }
                let target_input = jackdaw_csg::CsgInput::new(&fragment.faces, &fragment.topo);
                let cutter_input = jackdaw_csg::CsgInput::new(cutter_faces, cutter_topo);
                match jackdaw_csg::brush_difference_split(&target_input, &cutter_input) {
                    Ok(pieces) => {
                        for piece in pieces {
                            next.push(WorldBrush {
                                faces: piece.faces,
                                topo: piece.topology,
                            });
                        }
                    }
                    Err(jackdaw_csg::CsgError::EmptyResult) => {
                        // Cutter swallowed the fragment whole.
                    }
                    Err(e) => {
                        warn!("CSG subtract kernel error: {e}");
                        next.push(WorldBrush {
                            faces: fragment.faces.clone(),
                            topo: fragment.topo.clone(),
                        });
                    }
                }
            }
            current = next;
        }

        // Recentre each fragment to its own local space.
        let mut fragment_data: Vec<(Brush, Transform)> = Vec::new();
        for fragment in &current {
            if fragment.topo.vertices.len() < 4 {
                continue;
            }
            let centroid: Vec3 = fragment
                .topo
                .vertices
                .iter()
                .map(|v| v.position)
                .sum::<Vec3>()
                / fragment.topo.vertices.len() as f32;
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
            let mut local_topo = fragment.topo.clone();
            for v in &mut local_topo.vertices {
                v.position -= centroid;
            }
            // Use the mesh-CSG kernel's geometry directly. The
            // `clean_degenerate_faces` + plane-intersection fallback
            // path is intentionally skipped here because it collapses
            // concave fragments to a convex hull and makes the brush
            // appear deleted in the editor.
            fragment_data.push((
                Brush {
                    faces: local_faces,
                    topology: local_topo,
                },
                Transform::from_translation(centroid),
            ));
        }

        if fragment_data.is_empty() {
            continue;
        }

        results.push(SubtractionResult {
            original_entity: entity,
            fragments: fragment_data,
        });
    }

    if results.is_empty() {
        return;
    }

    // Capture brush data for originals (assigns stable IDs)
    let mut originals: Vec<BrushData> = Vec::new();
    for result in &results {
        originals.push(brush_data_from_entity(world, result.original_entity));
    }

    // Capture parent group info before despawning originals
    let mut parent_groups: std::collections::HashMap<Entity, (BrushStableId, Vec3)> =
        std::collections::HashMap::new();
    for result in &results {
        if let Some((parent_entity, parent_translation)) =
            brush_parent_group(world, result.original_entity)
        {
            let parent_sid = if let Some(sid) = world.get::<BrushStableId>(parent_entity) {
                *sid
            } else {
                let sid = world.resource_mut::<StableIdCounter>().next();
                world.entity_mut(parent_entity).insert(sid);
                sid
            };
            parent_groups.insert(result.original_entity, (parent_sid, parent_translation));
        }
    }

    // Clean up selection: remove targets about to be despawned
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

    // Despawn originals
    for result in &results {
        if let Ok(e) = world.get_entity_mut(result.original_entity) {
            e.despawn();
        }
    }

    // Spawn fragments and build BrushOrGroup data
    let mut fragments: Vec<BrushOrGroup> = Vec::new();
    let mut counter = world.resource_mut::<StableIdCounter>();
    let fragment_stable_ids: Vec<Vec<BrushStableId>> = results
        .iter()
        .map(|r| r.fragments.iter().map(|_| counter.next()).collect())
        .collect();
    let group_stable_ids: Vec<Option<BrushStableId>> = results
        .iter()
        .map(|r| {
            if r.fragments.len() > 1 && !parent_groups.contains_key(&r.original_entity) {
                Some(counter.next())
            } else {
                None
            }
        })
        .collect();

    for (result_idx, result) in results.iter().enumerate() {
        if let Some(&(parent_sid, parent_translation)) = parent_groups.get(&result.original_entity)
        {
            for (frag_idx, (brush, transform)) in result.fragments.iter().enumerate() {
                let brush_data = BrushData {
                    stable_id: fragment_stable_ids[result_idx][frag_idx],
                    brush: brush.clone(),
                    transform: Transform::from_translation(
                        transform.translation - parent_translation,
                    ),
                    name: "Brush".to_string(),
                    parent_stable_id: Some(parent_sid),
                };
                spawn_brush_from_data(world, &brush_data);
                fragments.push(BrushOrGroup::Single(Box::new(brush_data)));
            }
        } else if result.fragments.len() == 1 {
            let (brush, transform) = &result.fragments[0];
            let brush_data = BrushData {
                stable_id: fragment_stable_ids[result_idx][0],
                brush: brush.clone(),
                transform: *transform,
                name: "Brush".to_string(),
                parent_stable_id: None,
            };
            spawn_brush_from_data(world, &brush_data);
            fragments.push(BrushOrGroup::Single(Box::new(brush_data)));
        } else if result.fragments.len() > 1 {
            let group_center = result
                .fragments
                .iter()
                .map(|(_, tf)| tf.translation)
                .sum::<Vec3>()
                / result.fragments.len() as f32;

            let group_sid = group_stable_ids[result_idx].unwrap();
            let children: Vec<BrushData> = result
                .fragments
                .iter()
                .enumerate()
                .map(|(frag_idx, (brush, transform))| BrushData {
                    stable_id: fragment_stable_ids[result_idx][frag_idx],
                    brush: brush.clone(),
                    transform: Transform::from_translation(transform.translation - group_center),
                    name: "Brush".to_string(),
                    parent_stable_id: None,
                })
                .collect();

            let group_data = BrushOrGroup::Group {
                stable_id: group_sid,
                transform: Transform::from_translation(group_center),
                name: "Brush Group".to_string(),
                parent_stable_id: None,
                children,
            };
            spawn_brush_or_group(world, &group_data);
            fragments.push(group_data);
        }
    }

    // Push undo command
    let cmd = SubtractBrushCommand {
        originals,
        fragments,
    };
    let mut history = world.resource_mut::<CommandHistory>();
    history.push_executed(Box::new(cmd));
}

/// Core logic for CSG Intersect. Replaces all selected brushes with their intersection.
pub(crate) fn csg_intersect_selected_impl(world: &mut World) {
    let selection = world.resource::<Selection>();
    let selected_set: Vec<Entity> = selection.entities.clone();

    // Bake live mirrors into the authored topology before any geometry is
    // read, so the intersection sees both halves; baking removes the
    // `MeshMirror`, which must not survive onto the result brush.
    for &entity in &selected_set {
        crate::brush::topology_ops::mirror_ops::bake_mirror(world, entity);
    }

    let mut brush_query = world.query::<(Entity, &Brush, &GlobalTransform)>();
    let selected_brushes: Vec<(Entity, Brush, GlobalTransform)> = brush_query
        .iter(world)
        .filter(|(e, _, _)| selected_set.contains(e))
        .map(|(e, b, gt)| (e, b.clone(), *gt))
        .collect();

    if selected_brushes.len() < 2 {
        return;
    }

    // Intersect via mesh-CSG (manifold kernel). Cumulatively intersect
    // each subsequent brush into the running result. Works for both
    // convex and concave inputs.

    let world_inputs: Vec<(Vec<BrushFaceData>, jackdaw_jsn::BrushTopology)> = selected_brushes
        .iter()
        .map(|(_, brush, gt)| {
            let (_, rotation, translation) = gt.to_scale_rotation_translation();
            jackdaw_csg::brush_to_world(&brush.faces, &brush.topology, rotation, translation)
        })
        .collect();

    let mut running = jackdaw_csg::CsgBrush {
        faces: world_inputs[0].0.clone(),
        topology: world_inputs[0].1.clone(),
    };
    for (next_faces, next_topo) in world_inputs.iter().skip(1) {
        let lhs = jackdaw_csg::CsgInput::new(&running.faces, &running.topology);
        let rhs = jackdaw_csg::CsgInput::new(next_faces, next_topo);
        match jackdaw_csg::brush_boolean(&lhs, &rhs, jackdaw_csg::BooleanOp::Intersection) {
            Ok(b) => running = b,
            Err(jackdaw_csg::CsgError::EmptyResult) => return,
            Err(e) => {
                warn!("CSG intersect kernel error: {e}");
                return;
            }
        }
    }
    if running.topology.vertices.len() < 4 || running.faces.len() < 4 {
        return;
    }

    let centroid: Vec3 = running
        .topology
        .vertices
        .iter()
        .map(|v| v.position)
        .sum::<Vec3>()
        / running.topology.vertices.len() as f32;

    let local_faces: Vec<BrushFaceData> = running
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
    let clean = clean_degenerate_faces(&local_faces);
    if clean.len() < 4 {
        return;
    }
    let mut local_topo = running.topology.clone();
    for v in &mut local_topo.vertices {
        v.position -= centroid;
    }

    // Capture brush data for originals (assigns stable IDs)
    let mut originals: Vec<BrushData> = Vec::new();
    for (entity, _, _) in &selected_brushes {
        originals.push(brush_data_from_entity(world, *entity));
    }

    // Clean up selection
    {
        let despawning: Vec<Entity> = selected_brushes.iter().map(|(e, _, _)| *e).collect();
        let mut selection = world.resource_mut::<Selection>();
        selection.entities.retain(|e| !despawning.contains(e));
    }
    for (entity, _, _) in &selected_brushes {
        if let Ok(mut e) = world.get_entity_mut(*entity) {
            e.remove::<Selected>();
        }
    }

    // Despawn originals
    for (entity, _, _) in &selected_brushes {
        if let Ok(e) = world.get_entity_mut(*entity) {
            e.despawn();
        }
    }

    // Spawn the intersection brush. Reuse the manifold-derived topology
    // when cleaning didn't prune faces; otherwise re-derive from planes
    // so the parallel-array invariant with `faces` is preserved.
    let topology = if clean.len() == running.faces.len() {
        local_topo
    } else {
        compute_brush_topology(&clean)
    };
    let new_brush = Brush {
        faces: clean,
        topology,
    };
    let frag_sid = world.resource_mut::<StableIdCounter>().next();
    let brush_data = BrushData {
        stable_id: frag_sid,
        brush: new_brush,
        transform: Transform::from_translation(centroid),
        name: "Brush".to_string(),
        parent_stable_id: None,
    };
    let entity = spawn_brush_from_data(world, &brush_data);

    // Select the new brush
    {
        let mut selection = world.resource_mut::<Selection>();
        selection.entities.push(entity);
    }
    world.entity_mut(entity).insert(Selected);

    // Push undo command (reuses SubtractBrushCommand, same undo/redo pattern).
    let cmd = SubtractBrushCommand {
        originals,
        fragments: vec![BrushOrGroup::Single(Box::new(brush_data))],
    };
    let mut history = world.resource_mut::<CommandHistory>();
    history.push_executed(Box::new(cmd));
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
    description = "Subtract the non-first selected brushes from the first. \
                   Requires at least two `Brush` entities in `Selection::entities` \
                   (first is the target, rest are cutters); availability \
                   (`can_run_binary_brush_op`) is false otherwise. The target may \
                   be split into multiple fragment brushes.",
    is_available = can_run_binary_brush_op,
)]
pub(crate) fn brush_csg_subtract(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(csg_subtract_selected_impl);
    OperatorResult::Finished
}

#[operator(
    id = "brush.csg_intersect",
    label = "CSG Intersect",
    description = "Replace the selected brushes with the solid shared by all of \
                   them. Requires at least two `Brush` entities in \
                   `Selection::entities`; availability \
                   (`can_run_binary_brush_op`) is false otherwise. When the \
                   intersection is empty the impl exits silently without \
                   mutating the world.",
    is_available = can_run_binary_brush_op,
)]
pub(crate) fn brush_csg_intersect(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(csg_intersect_selected_impl);
    OperatorResult::Finished
}

/// Shared environment gate: brush-level operators never run mid-draw,
/// mid-modal, or while a text input is focused. Each specific op
/// composes this with its own selection-state precondition check.
pub(crate) fn env_allows_brush_op(
    keybind_focus: &KeybindFocus,
    modal: &crate::modal_transform::ModalTransformState,
    draw_state: &DrawBrushState,
) -> bool {
    !keybind_focus.is_typing() && modal.active.is_none() && draw_state.active.is_none()
}

/// `brush.join` / `brush.csg_subtract` / `brush.csg_intersect` all
/// require at least two `Brush` entities in the current selection.
fn can_run_binary_brush_op(
    keybind_focus: KeybindFocus,
    modal: Res<crate::modal_transform::ModalTransformState>,
    draw_state: Res<DrawBrushState>,
    selection: Res<Selection>,
    brushes: Query<(), With<Brush>>,
) -> bool {
    if !env_allows_brush_op(&keybind_focus, &modal, &draw_state) {
        return false;
    }
    selection
        .entities
        .iter()
        .filter(|&&e| brushes.contains(e))
        .count()
        >= 2
}
