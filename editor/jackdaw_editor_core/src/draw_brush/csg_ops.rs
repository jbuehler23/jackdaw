use crate::commands::{
    CommandGroup, CommandHistory, DespawnEntity, EditorCommand, despawn_scene_entity,
};
use crate::draw_brush::{
    ActiveDraw, DrawBrushState, MIN_FRAGMENT_SIZE, cut_brush_from_active, topology_aabbs_overlap,
};
use crate::keybind_focus::KeybindFocus;
use crate::prelude::*;
use crate::primitive_ops::bool_param;
use crate::selection::{Selected, Selection};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use jackdaw_api_internal::operator::{
    AvailabilityParams, BooleanReport, OperatorBooleanReport, report_boolean, report_replaced,
};
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

/// Spawn `result`'s fragments beside its original, under the same parent, at
/// the world placement the kernel gave them.
fn spawn_subtract_fragments(world: &mut World, result: &SubtractionResult) -> Vec<Entity> {
    let source = result.original_entity;
    let parent = world
        .get::<ChildOf>(source)
        .and_then(|child_of| unsorted_global(world, child_of.parent()));
    let mut created = Vec::new();
    for (brush, transform) in &result.fragments {
        let local = parent.map_or(*transform, |parent| {
            GlobalTransform::from(*transform).reparented_to(&parent)
        });
        match crate::entity_ops::spawn_cloned_brush_with_geometry(
            world,
            source,
            brush.clone(),
            local,
        ) {
            Some(entity) => created.push(entity),
            None => warn!("CSG fragment spawn failed: source {source:?} has no document node"),
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
        let cutter = WorldBrush::new(world_cutter_faces, world_cutter_topo);
        let cutter_input = cutter.input();

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
            if !topology_aabbs_overlap(&world_target_topo, &cutter.topo) {
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

            let target = WorldBrush::new(world_target_faces, world_target_topo);
            // `EmptyResult` here means the cutter fully consumed the target;
            // we treat that the same as "no fragments survive" and let the
            // downstream code despawn the original. A kernel error is
            // different: leave the original untouched.
            let mut pieces: Vec<WorldBrush> =
                match jackdaw_csg::brush_difference_split(&target.input(), &cutter_input) {
                    Ok(pieces) => pieces
                        .into_iter()
                        .map(|piece| WorldBrush::from_csg(piece, &target))
                        .collect(),
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
                pieces.len()
            );
            for (i, piece) in pieces.iter().enumerate() {
                let (bbox_min, bbox_max) = piece_bounds(piece);
                info!(
                    "csg-subtract:   fragment {} faces={} verts={} bbox=({:.4},{:.4},{:.4})..({:.4},{:.4},{:.4})",
                    i,
                    piece.faces.len(),
                    piece.topo.vertices.len(),
                    bbox_min.x,
                    bbox_min.y,
                    bbox_min.z,
                    bbox_max.x,
                    bbox_max.y,
                    bbox_max.z,
                );
            }

            pieces.retain(|piece| {
                let (bbox_min, bbox_max) = piece_bounds(piece);
                (bbox_max - bbox_min).min_element() >= MIN_FRAGMENT_SIZE
            });
            let fragments = shape_pieces(
                &target,
                pieces,
                &CsgOptions::default(),
                &mut Tally::default(),
            );
            results.push(SubtractionResult {
                original_entity: *entity,
                fragments,
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

        for result in &results {
            spawn_subtract_fragments(world, result);
        }

        for result in &results {
            despawn_scene_entity(world, result.original_entity);
        }
    });
}

fn piece_bounds(piece: &WorldBrush) -> (Vec3, Vec3) {
    piece
        .topo
        .vertices
        .iter()
        .fold((Vec3::MAX, Vec3::MIN), |(min, max), v| {
            (min.min(v.position), max.max(v.position))
        })
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
        crate::brush::modifier_bake::bake_modifier_stack_entity(world, entity);
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
/// brushes they overlap are targets. A target the cutters leave as it was, or
/// that the kernel fails on, is left alone; one they swallow is removed.
/// `keep_cutters = Some(false)` removes the cutters once something is cut.
fn csg_subtract_selected_impl(world: &mut World, options: &CsgOptions) {
    let selected_set: Vec<Entity> = world.resource::<Selection>().entities.clone();
    let mut brush_query = world.query_filtered::<Entity, With<Brush>>();
    let all_brushes: Vec<Entity> = brush_query.iter(world).collect();
    let (cutters, targets): (Vec<Entity>, Vec<Entity>) = all_brushes
        .iter()
        .partition(|entity| selected_set.contains(entity));
    if cutters.is_empty() || targets.is_empty() {
        set_outcome(world, Outcome::Unchanged);
        return;
    }

    // Subtract via mesh-CSG (manifold kernel). Works on both convex and
    // concave inputs; a cut that splits a target yields several fragments.
    let cutter_world: Vec<WorldBrush> = cutters
        .iter()
        .filter_map(|&cutter| world_operand(world, cutter))
        .collect();

    let measure = options.measures(world);
    let mut tally = Tally::new(measure);
    let mut results: Vec<SubtractionResult> = Vec::new();
    for &entity in &targets {
        let Some(target) = world_operand(world, entity) else {
            continue;
        };
        // Cheap rejection with topology-vertex AABBs (the plane
        // separating-axis test isn't sound on concave brushes).
        let reaching: Vec<&WorldBrush> = cutter_world
            .iter()
            .filter(|cutter| topology_aabbs_overlap(&target.topo, &cutter.topo))
            .collect();
        if reaching.is_empty() {
            continue;
        }
        let (pieces, failures) = subtract_all(&target, &reaching);
        if let Some(e) = failures.first() {
            warn_caller(
                world,
                format!("CSG Subtract: the kernel failed on {entity}, so it was left alone: {e}"),
            );
            continue;
        }
        let mut share = Tally::new(measure);
        let fragments = shape_pieces(&target, pieces, options, &mut share);
        share.add(&target, &reaching, &fragments);
        if share.leaves_as_is(&fragments) {
            continue;
        }
        tally.merge(share);
        results.push(SubtractionResult {
            original_entity: entity,
            fragments,
        });
    }
    if results.is_empty() {
        set_outcome(world, Outcome::Unchanged);
        return;
    }
    let outcome = if results.iter().any(|result| !result.fragments.is_empty()) {
        Outcome::Changed
    } else {
        Outcome::Empty
    };
    if measure {
        report_boolean(world, tally.report(outcome));
    }
    if options.dry_run {
        set_outcome(world, Outcome::Unchanged);
        return;
    }

    let mut also_removed = Vec::new();
    if options.keep_cutters == Some(false) {
        also_removed = cutters;
    }
    replace_with_fragments(world, results, options.fragments, also_removed, outcome);
}

/// Core logic for CSG Intersect. Replaces the selected brushes with the solid
/// all of them share, cloned from the first selected; `keep_cutters =
/// Some(true)` replaces only that first one and keeps the rest.
fn csg_intersect_selected_impl(world: &mut World, options: &CsgOptions) {
    let selected_set: Vec<Entity> = world.resource::<Selection>().entities.clone();
    let selected_brushes: Vec<Entity> = selected_set
        .iter()
        .copied()
        .filter(|&e| world.get::<Brush>(e).is_some())
        .collect();
    if selected_brushes.len() < 2 {
        set_outcome(world, Outcome::Unchanged);
        return;
    }

    let mut world_inputs = selected_brushes
        .iter()
        .filter_map(|&entity| world_operand(world, entity));
    let Some(first) = world_inputs.next() else {
        set_outcome(world, Outcome::Unchanged);
        return;
    };
    let rest: Vec<WorldBrush> = world_inputs.collect();
    let running = match intersect_all(&first, &rest) {
        Ok(Some(running)) => running,
        Ok(None) => {
            set_outcome(world, Outcome::Unchanged);
            return;
        }
        Err(e) => {
            warn!("CSG intersect kernel error: {e}");
            set_outcome(world, Outcome::Unchanged);
            return;
        }
    };
    let measure = options.measures(world);
    let mut tally = Tally::new(measure);
    let Some(fragment) = shape_pieces(&first, vec![running], options, &mut tally)
        .into_iter()
        .next()
        .and_then(|fragment| clean_fragment(fragment, &mut tally.degenerate))
    else {
        set_outcome(world, Outcome::Unchanged);
        return;
    };
    let rest_refs: Vec<&WorldBrush> = rest.iter().collect();
    tally.add(&first, &rest_refs, std::slice::from_ref(&fragment));
    let source = selected_brushes[0];
    let removed: Vec<Entity> = if options.keep_cutters == Some(true) {
        vec![source]
    } else {
        tally.volume_before += rest
            .iter()
            .map(|other| topology_volume(&other.topo))
            .sum::<f32>();
        selected_brushes.clone()
    };
    if measure {
        report_boolean(world, tally.report(Outcome::Changed));
    }
    if options.dry_run {
        set_outcome(world, Outcome::Unchanged);
        return;
    }

    // Bake the first brush's live mirror before the result is cloned from it;
    // baking removes the `MeshMirror`, which must not survive onto the result.
    crate::brush::modifier_bake::bake_modifier_stack_entity(world, source);

    crate::commands::deselect_entities(world, &removed);
    let created = spawn_subtract_fragments(
        world,
        &SubtractionResult {
            original_entity: source,
            fragments: vec![fragment],
        },
    );
    for &entity in &removed {
        despawn_scene_entity(world, entity);
    }
    for &entity in &created {
        world.resource_mut::<Selection>().entities.push(entity);
        world.entity_mut(entity).insert(Selected);
    }
    report_replaced(world, &removed, &created);
    set_outcome(world, Outcome::Changed);
}

/// What the faces a cut creates wear.
enum CutFaces {
    Cutter,
    Target,
    Material(Handle<StandardMaterial>),
}

/// What a subtract does with the pieces it cuts a target into.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Fragments {
    /// Every piece is a new brush where the target was.
    Loose,
    /// The pieces go under a new group named after the target, as
    /// `<target> 1`, `<target> 2`...
    Group,
    /// One connected piece stays in the target's entity; several are grouped.
    Merge,
}

/// How a call wants its boolean run.
struct CsgOptions {
    keep_cutters: Option<bool>,
    fragments: Fragments,
    /// Work the result out and report it without touching the scene.
    dry_run: bool,
    cut_faces: CutFaces,
    /// Give the faces a cut creates a world-aligned projection.
    align_cut_uvs: bool,
}

impl Default for CsgOptions {
    fn default() -> Self {
        Self {
            keep_cutters: None,
            fragments: Fragments::Loose,
            dry_run: false,
            cut_faces: CutFaces::Cutter,
            align_cut_uvs: true,
        }
    }
}

impl CsgOptions {
    fn read(world: &World, op: CsgOp, params: &OperatorParameters) -> Result<Self, String> {
        let fragments = match op {
            _ if !params.contains_key("fragments") => Fragments::Loose,
            CsgOp::Intersect => {
                return Err("`fragments` applies to CSG Subtract only".to_string());
            }
            CsgOp::Subtract => match params.as_str("fragments") {
                Some("loose") => Fragments::Loose,
                Some("group") => Fragments::Group,
                Some("merge") => Fragments::Merge,
                _ => return Err("`fragments` must be `merge`, `group` or `loose`".to_string()),
            },
        };
        let cut_faces = match params.get("cut_faces") {
            None => CutFaces::Cutter,
            Some(_) => match params.as_str("cut_faces") {
                Some("cutter") => CutFaces::Cutter,
                Some("target") => CutFaces::Target,
                reference => CutFaces::Material(crate::primitive_ops::standard_material(
                    world,
                    "cut_faces",
                    reference,
                )?),
            },
        };
        Ok(Self {
            keep_cutters: bool_param(params, "keep_cutters")?,
            fragments,
            dry_run: bool_param(params, "dry_run")?.unwrap_or(false),
            cut_faces,
            align_cut_uvs: bool_param(params, "align_cut_uvs")?.unwrap_or(true),
        })
    }

    /// Whether the call's report is wanted: always on a dry run, else only
    /// when the caller asked for one.
    fn measures(&self, world: &World) -> bool {
        self.dry_run
            || world
                .get_resource::<OperatorBooleanReport>()
                .is_some_and(OperatorBooleanReport::wanted)
    }
}

/// A boolean's report, summed over the brushes it cut. The bounds, watertight
/// and coplanar measures are taken only when `measure` is set.
#[derive(Default)]
struct Tally {
    measure: bool,
    fragments: usize,
    volume_before: f32,
    volume_after: f32,
    bounds: Option<(Vec3, Vec3)>,
    open_edges: usize,
    cut_faces: usize,
    coplanar: usize,
    degenerate: usize,
}

impl Tally {
    fn new(measure: bool) -> Self {
        Self {
            measure,
            ..Default::default()
        }
    }

    /// Count `target`, cut by `cutters` into `fragments`.
    fn add(
        &mut self,
        target: &WorldBrush,
        cutters: &[&WorldBrush],
        fragments: &[(Brush, Transform)],
    ) {
        self.fragments += fragments.len();
        self.volume_before += topology_volume(&target.topo);
        self.volume_after += fragments
            .iter()
            .map(|(brush, _)| topology_volume(&brush.topology))
            .sum::<f32>();
        if !self.measure {
            return;
        }
        let eps = plane_distance_eps(&target.topo);
        self.coplanar += cutters
            .iter()
            .flat_map(|cutter| &cutter.faces)
            .filter(|face| {
                target
                    .faces
                    .iter()
                    .any(|own| coplanar(&own.plane, &face.plane, eps))
            })
            .count();
        for (brush, transform) in fragments {
            self.open_edges += brush.topology.open_edges().map_or(1, |open| open.len());
            for vertex in &brush.topology.vertices {
                let point = transform.transform_point(vertex.position);
                self.bounds = Some(self.bounds.map_or((point, point), |(min, max)| {
                    (min.min(point), max.max(point))
                }));
            }
        }
    }

    /// Whether the fragments counted are the target as it was.
    fn leaves_as_is(&self, fragments: &[(Brush, Transform)]) -> bool {
        let whole = self.volume_before;
        !fragments.is_empty() && (self.volume_after - whole).abs() <= whole.max(1e-6) * 1e-4
    }

    fn merge(&mut self, other: Tally) {
        self.fragments += other.fragments;
        self.volume_before += other.volume_before;
        self.volume_after += other.volume_after;
        self.bounds = match (self.bounds, other.bounds) {
            (Some((a_min, a_max)), Some((b_min, b_max))) => {
                Some((a_min.min(b_min), a_max.max(b_max)))
            }
            (bounds, None) | (None, bounds) => bounds,
        };
        self.open_edges += other.open_edges;
        self.cut_faces += other.cut_faces;
        self.coplanar += other.coplanar;
        self.degenerate += other.degenerate;
    }

    fn report(&self, outcome: Outcome) -> BooleanReport {
        BooleanReport {
            outcome,
            fragments: self.fragments,
            volume_before: self.volume_before,
            volume_after: self.volume_after,
            bounds: self.bounds,
            watertight: self.open_edges == 0,
            cut_faces: self.cut_faces,
            coplanar: self.coplanar,
            degenerate: self.degenerate,
        }
    }
}

/// Plane-distance tolerance for a brush: a millimetre near the origin, growing
/// with the distance from it as f32 precision does.
pub(crate) fn plane_distance_eps(topo: &BrushTopology) -> f32 {
    let reach = topo
        .vertices
        .iter()
        .map(|v| v.position.abs().max_element())
        .fold(0.0, f32::max);
    1e-3 + reach * 1e-5
}

/// Whether two planes coincide, facing either way.
fn coplanar(a: &BrushPlane, b: &BrushPlane, eps: f32) -> bool {
    let facing = a.normal.dot(b.normal);
    if facing.abs() < 1.0 - 1e-3 {
        return false;
    }
    (a.distance - facing.signum() * b.distance).abs() < eps
}

/// `pieces` of `target` as recentred fragments: pieces too thin to be a solid
/// dropped, and the faces the cut created dressed as `options` asks.
fn shape_pieces(
    target: &WorldBrush,
    mut pieces: Vec<WorldBrush>,
    options: &CsgOptions,
    tally: &mut Tally,
) -> Vec<(Brush, Transform)> {
    let count = pieces.len();
    pieces.retain(|piece| piece.topo.vertices.len() >= 4 && piece.faces.len() >= 4);
    tally.degenerate += count - pieces.len();
    tally.cut_faces += dress_cut_faces(&mut pieces, target, options);
    recentre_fragments(&pieces)
}

/// Give every face of `pieces` the cut created the material and projection
/// `options` asks for. Returns how many there were.
///
/// The projection takes the scale of the material the face ends up wearing:
/// the target's for `cut_faces = target`, the cutter face's own otherwise.
fn dress_cut_faces(pieces: &mut [WorldBrush], target: &WorldBrush, options: &CsgOptions) -> usize {
    let dominant = dominant_face(&target.faces);
    let mut count = 0;
    for piece in pieces.iter_mut() {
        for (face, _) in piece
            .faces
            .iter_mut()
            .zip(&piece.cut)
            .filter(|(_, cut)| **cut)
        {
            count += 1;
            match &options.cut_faces {
                CutFaces::Cutter => {}
                CutFaces::Target => {
                    if let Some(dominant) = dominant {
                        face.material = dominant.material.clone();
                        face.uv_scale = dominant.uv_scale;
                    }
                }
                CutFaces::Material(material) => face.material = material.clone(),
            }
            if options.align_cut_uvs {
                let axes = jackdaw_uv::world_aligned(face.plane.normal);
                face.uv_u_axis = axes.u;
                face.uv_v_axis = axes.v;
                face.uv_rotation = 0.0;
                face.uv_offset = Vec2::ZERO;
            }
        }
    }
    count
}

/// The first face wearing the material most of `faces` wear.
fn dominant_face(faces: &[BrushFaceData]) -> Option<&BrushFaceData> {
    let wearing = |face: &BrushFaceData| {
        faces
            .iter()
            .filter(|other| other.material == face.material)
            .count()
    };
    faces
        .iter()
        .enumerate()
        .max_by_key(|(index, face)| (wearing(face), std::cmp::Reverse(*index)))
        .map(|(_, face)| face)
}

/// A brush's faces and topology in world space, as the CSG kernel takes them.
#[derive(Clone)]
struct WorldBrush {
    faces: Vec<BrushFaceData>,
    topo: BrushTopology,
    /// Per face: whether a cut created it, rather than it being part of the
    /// brush the cut started from.
    cut: Vec<bool>,
}

impl WorldBrush {
    fn new(faces: Vec<BrushFaceData>, topo: BrushTopology) -> Self {
        let cut = vec![false; faces.len()];
        Self { faces, topo, cut }
    }

    fn input(&self) -> jackdaw_csg::CsgInput<'_> {
        jackdaw_csg::CsgInput::new(&self.faces, &self.topo)
    }

    /// A boolean result whose first operand was `from`: a face is cut when it
    /// came from another operand, from nothing, or from a cut face of `from`.
    fn from_csg(brush: jackdaw_csg::CsgBrush, from: &WorldBrush) -> Self {
        let cut = (0..brush.faces.len())
            .map(|index| match brush.origins.get(index).copied().flatten() {
                Some(jackdaw_csg::FaceOrigin { operand: 0, face }) => {
                    from.cut.get(face).copied().unwrap_or(false)
                }
                _ => true,
            })
            .collect();
        Self {
            faces: brush.faces,
            topo: brush.topology,
            cut,
        }
    }
}

/// The volume a closed topology encloses.
fn topology_volume(topo: &BrushTopology) -> f32 {
    let corner = |index: u32| topo.vertices.get(index as usize).map(|v| v.position);
    let mut six_times = 0.0;
    for polygon in &topo.polygons {
        let start = polygon.loop_start as usize;
        let Some(ring) = topo.loops.get(start..start + polygon.loop_total as usize) else {
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

/// `target` with every cutter differenced off in turn, as world-space pieces,
/// and the kernel errors met on the way. A piece the kernel fails on is kept
/// whole.
fn subtract_all(
    target: &WorldBrush,
    cutters: &[&WorldBrush],
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
                Ok(pieces) => next.extend(
                    pieces
                        .into_iter()
                        .map(|piece| WorldBrush::from_csg(piece, &fragment)),
                ),
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
    cutters: &[&WorldBrush],
) -> Result<Vec<WorldBrush>, jackdaw_csg::CsgError> {
    let union;
    let shape = match cutters {
        [] => return Ok(Vec::new()),
        [only] => only.input(),
        many => {
            let inputs: Vec<jackdaw_csg::CsgInput<'_>> =
                many.iter().map(|cutter| cutter.input()).collect();
            union = jackdaw_csg::brush_batch_union(&inputs)?;
            jackdaw_csg::CsgInput::new(&union.faces, &union.topology)
        }
    };
    match jackdaw_csg::brush_intersection_split(&target.input(), &shape) {
        Ok(pieces) => Ok(pieces
            .into_iter()
            .map(|piece| WorldBrush::from_csg(piece, target))
            .collect()),
        Err(jackdaw_csg::CsgError::EmptyResult) => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// Each world-space piece as a brush centred on its own corners, with the
/// translation that puts it back and its textures where they were.
///
/// The kernel's topology is used as it comes: rebuilding it from the planes
/// would collapse a concave fragment to its convex hull.
fn recentre_fragments(pieces: &[WorldBrush]) -> Vec<(Brush, Transform)> {
    pieces
        .iter()
        .filter(|piece| !piece.topo.vertices.is_empty())
        .map(|piece| {
            let centroid: Vec3 = piece.topo.vertices.iter().map(|v| v.position).sum::<Vec3>()
                / piece.topo.vertices.len() as f32;
            let (faces, topology) = jackdaw_csg::brush_to_world(
                &piece.faces,
                &piece.topo,
                bevy::math::Affine3A::from_translation(-centroid),
            );
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
    first: &WorldBrush,
    rest: &[WorldBrush],
) -> Result<Option<WorldBrush>, jackdaw_csg::CsgError> {
    let mut running = first.clone();
    for next in rest {
        match jackdaw_csg::brush_boolean(
            &running.input(),
            &next.input(),
            jackdaw_csg::BooleanOp::Intersection,
        ) {
            Ok(shared) => running = WorldBrush::from_csg(shared, &running),
            Err(jackdaw_csg::CsgError::EmptyResult) => return Ok(None),
            Err(e) => return Err(e),
        }
    }
    Ok(Some(running))
}

/// A recentred fragment with its degenerate faces dropped and counted into
/// `dropped`; `None` when too little of it is left to be a solid.
fn clean_fragment(
    (brush, transform): (Brush, Transform),
    dropped: &mut usize,
) -> Option<(Brush, Transform)> {
    let clean = clean_degenerate_faces(&brush.faces);
    *dropped += brush.faces.len() - clean.len();
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
    /// The brushes cut: the one `target` named, or every brush under the node
    /// `targets` named.
    targets: Vec<Entity>,
    /// The node `cutters` named, which `keep_cutters = false` removes.
    cutters_node: Entity,
    /// The brushes that cut: the named one, or every brush under the named
    /// group.
    cutters: Vec<Entity>,
    keep_cutters: bool,
}

/// The parameters that name operands rather than work on the selection.
const OPERAND_KEYS: [&str; 3] = ["target", "targets", "cutters"];

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
        let targets = if params.contains_key("targets") {
            if params.contains_key("target") {
                return Err("name `target` or `targets`, not both".to_string());
            }
            let node = operand_node(world, params, "targets")?;
            let found = brushes_under(world, node);
            if found.is_empty() {
                return Err(format!(
                    "`targets` {node} is neither a brush nor a group holding brushes"
                ));
            }
            found
        } else {
            let target = operand_node(world, params, "target")?;
            if world.get::<Brush>(target).is_none() {
                return Err(format!("`target` {target} is not a brush"));
            }
            vec![target]
        };
        let cutters_node = operand_node(world, params, "cutters")?;
        if targets
            .iter()
            .any(|&target| is_at_or_under(world, cutters_node, target))
        {
            return Err(format!(
                "`cutters` {cutters_node} is a target or sits under one"
            ));
        }
        let cutters = brushes_under(world, cutters_node);
        if cutters.is_empty() {
            return Err(format!(
                "`cutters` {cutters_node} is neither a brush nor a group holding brushes"
            ));
        }
        if let Some(both) = cutters.iter().find(|cutter| targets.contains(cutter)) {
            return Err(format!("{both} is both a target and one of `cutters`"));
        }
        Ok(Some(Self {
            targets,
            cutters_node,
            cutters,
            keep_cutters: keep_cutters.unwrap_or(true),
        }))
    }
}

pub(crate) fn operand_node(
    world: &World,
    params: &OperatorParameters,
    key: &str,
) -> Result<Entity, String> {
    match params.get(key) {
        Some(PropertyValue::Entity(entity)) => {
            crate::primitive_ops::scene_node(world, *entity, key)
        }
        Some(other) => Err(format!("`{key}` {other:?} names no node in this scene")),
        None => Err(format!(
            "name `target` (or `targets`) and `cutters`; `{key}` is missing"
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

/// `entity`'s brush in its own space with its modifier stack evaluated, read
/// without touching the scene.
pub(crate) fn evaluated_brush(world: &World, entity: Entity) -> Option<Brush> {
    let mut brush = world.get::<Brush>(entity)?.clone();
    if let Some(stack) = world.get::<ModifierStack>(entity) {
        let modifiers: Vec<&Modifier> = stack.modifiers.iter().map(|e| &e.modifier).collect();
        crate::brush::modifier_bake::bake_modifier_stack(&mut brush, &modifiers);
    }
    Some(brush)
}

/// `entity`'s brush in world space with its modifier stack evaluated, read
/// without touching the scene.
fn world_operand(world: &World, entity: Entity) -> Option<WorldBrush> {
    let brush = evaluated_brush(world, entity)?;
    let global = unsorted_global(world, entity)?;
    let (faces, topo) = jackdaw_csg::brush_to_world(&brush.faces, &brush.topology, global.affine());
    Some(WorldBrush::new(faces, topo))
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
    let options = match CsgOptions::read(world, op, params) {
        Ok(options) => options,
        Err(why) => return refuse(world, op, Outcome::Invalid, why),
    };
    match NamedOperands::read(world, params, options.keep_cutters) {
        Ok(Some(operands)) => run_on_named_operands(world, op, &operands, &options),
        Ok(None) => {
            match op {
                CsgOp::Subtract => csg_subtract_selected_impl(world, &options),
                CsgOp::Intersect => csg_intersect_selected_impl(world, &options),
            }
            OperatorResult::Finished
        }
        Err(why) => refuse(world, op, Outcome::Invalid, why),
    }
}

/// Run `op` on the operands a call named: only the targets are replaced, each
/// by what is left of it, and removed when nothing is. Everything is computed
/// before the scene is touched, so a call that changes nothing, a refused one
/// or a dry run leaves it as it was.
fn run_on_named_operands(
    world: &mut World,
    op: CsgOp,
    operands: &NamedOperands,
    options: &CsgOptions,
) -> OperatorResult {
    let cutters: Vec<WorldBrush> = operands
        .cutters
        .iter()
        .filter_map(|&cutter| world_operand(world, cutter))
        .collect();
    let measure = options.measures(world);
    let mut tally = Tally::new(measure);
    let mut untouched = Tally::new(measure);
    let mut results: Vec<SubtractionResult> = Vec::new();
    let mut reached = false;
    for &entity in &operands.targets {
        let Some(target) = world_operand(world, entity) else {
            let why = format!("target {entity} has no transform");
            return refuse(world, op, Outcome::Invalid, why);
        };
        let reaching: Vec<&WorldBrush> = cutters
            .iter()
            .filter(|cutter| topology_aabbs_overlap(&target.topo, &cutter.topo))
            .collect();
        let pieces = match op {
            CsgOp::Subtract => {
                if reaching.is_empty() {
                    continue;
                }
                let (pieces, failures) = subtract_all(&target, &reaching);
                if let Some(e) = failures.first() {
                    let why = format!("the kernel failed on {entity}, so nothing was cut: {e}");
                    return refuse(world, op, Outcome::Unchanged, why);
                }
                pieces
            }
            CsgOp::Intersect => match inside_any(&target, &reaching) {
                Ok(pieces) => pieces,
                Err(e) => {
                    let why = format!("the kernel failed on {entity}, so nothing was trimmed: {e}");
                    return refuse(world, op, Outcome::Unchanged, why);
                }
            },
        };
        reached = true;
        let mut share = Tally::new(measure);
        let fragments = shape_pieces(&target, pieces, options, &mut share);
        share.add(&target, &reaching, &fragments);
        if share.leaves_as_is(&fragments) {
            untouched.merge(share);
            continue;
        }
        tally.merge(share);
        results.push(SubtractionResult {
            original_entity: entity,
            fragments,
        });
    }
    if !reached {
        return refuse(
            world,
            op,
            Outcome::Unchanged,
            "the cutters do not reach the target",
        );
    }
    if results.is_empty() {
        if measure {
            report_boolean(world, untouched.report(Outcome::Unchanged));
        }
        return refuse(
            world,
            op,
            Outcome::Unchanged,
            "the cutters leave the target as it is",
        );
    }
    let outcome = if results.iter().any(|result| !result.fragments.is_empty()) {
        Outcome::Changed
    } else {
        Outcome::Empty
    };
    if measure {
        report_boolean(world, tally.report(outcome));
    }
    if options.dry_run {
        set_outcome(world, Outcome::Unchanged);
        return OperatorResult::Finished;
    }
    let mut also_removed = Vec::new();
    if !operands.keep_cutters {
        also_removed.push(operands.cutters_node);
    }
    replace_with_fragments(world, results, options.fragments, also_removed, outcome);
    OperatorResult::Finished
}

/// Put each result's fragments in place of its target, moving a removed
/// target's other children up to its nearest surviving ancestor so they
/// outlive it, and remove `also_removed` (the cutters, when the call asked).
fn replace_with_fragments(
    world: &mut World,
    results: Vec<SubtractionResult>,
    mode: Fragments,
    also_removed: Vec<Entity>,
    outcome: Outcome,
) {
    let doomed: Vec<Entity> = results
        .iter()
        .filter(|result| removes_target(result, mode))
        .map(|result| result.original_entity)
        .collect();
    let mut removed = Vec::new();
    let mut created = Vec::new();
    for result in results {
        let placed = place_fragments(world, result, mode, &doomed);
        for &entity in &placed.spawned {
            crate::commands::SpawnedEntities::record(world, entity);
        }
        created.extend(placed.created);
        removed.extend(placed.removed);
    }
    for &target in &removed {
        adopt_children(world, target, &removed);
    }
    crate::commands::deselect_entities(world, &also_removed);
    removed.extend(also_removed);
    for &entity in &removed {
        despawn_scene_entity(world, entity);
    }
    report_replaced(world, &removed, &created);
    set_outcome(world, outcome);
}

/// Whether placing `result` as `mode` asks takes its target away.
fn removes_target(result: &SubtractionResult, mode: Fragments) -> bool {
    mode != Fragments::Merge || result.fragments.len() != 1
}

/// `node`'s nearest ancestor that is not among `doomed`; `None` at the top.
fn surviving_ancestor(world: &World, node: Entity, doomed: &[Entity]) -> Option<Entity> {
    std::iter::successors(world.get::<ChildOf>(node).map(ChildOf::parent), |&at| {
        world.get::<ChildOf>(at).map(ChildOf::parent)
    })
    .find(|at| !doomed.contains(at))
}

/// Move `node`'s scene children, other than those also going, up to its
/// nearest ancestor that is not among `doomed`.
fn adopt_children(world: &mut World, node: Entity, doomed: &[Entity]) {
    let parent = surviving_ancestor(world, node, doomed);
    let children: Vec<Entity> = world
        .get::<Children>(node)
        .map(|children| children.iter().collect())
        .unwrap_or_default();
    let document = world.resource::<jackdaw_bsn::SceneBsnAst>();
    let adopted: Vec<Entity> = children
        .into_iter()
        .filter(|&child| document.ast_for(child).is_some() && !doomed.contains(&child))
        .collect();
    for child in adopted {
        crate::commands::set_parent(world, child, parent);
    }
}

/// What placing one target's fragments did.
struct Placed {
    /// The target, when it is to be removed; the caller removes it.
    removed: Option<Entity>,
    /// The brushes standing where the target was: the target itself when it
    /// was kept, else its fragments.
    created: Vec<Entity>,
    /// Every entity spawned, the group included.
    spawned: Vec<Entity>,
}

/// Put `result`'s fragments where its target is, as `mode` asks, under the
/// target's nearest ancestor not among `doomed`. The target is left in the
/// scene for the caller to remove, so its children can move first.
fn place_fragments(
    world: &mut World,
    result: SubtractionResult,
    mode: Fragments,
    doomed: &[Entity],
) -> Placed {
    let target = result.original_entity;
    // Bake the live modifier stack before the target is rewritten or cloned,
    // so the result cannot get re-mirrored by the mesh rebuild.
    crate::brush::modifier_bake::bake_modifier_stack_entity(world, target);
    if !removes_target(&result, mode)
        && let [fragment] = result.fragments.as_slice()
    {
        merge_into(world, target, fragment);
        return Placed {
            removed: None,
            created: vec![target],
            spawned: Vec::new(),
        };
    }
    crate::commands::deselect_entities(world, &[target]);
    let home = surviving_ancestor(world, target, doomed);
    let mut slot = crate::commands::HierarchyLocation::from_world(world, target);
    slot.parent = home;
    let mut taken = names_besides(world, target);
    let created = spawn_subtract_fragments(world, &result);
    let mut spawned = created.clone();
    if mode == Fragments::Loose || created.is_empty() {
        if home != world.get::<ChildOf>(target).map(ChildOf::parent) {
            for &fragment in &created {
                crate::commands::set_parent(world, fragment, home);
            }
        }
    } else {
        let base = name_or_default(world, target);
        let group_name = free_name(&mut taken, &base);
        let group = spawn_fragment_group(world, target, slot, &group_name);
        for (index, &fragment) in created.iter().enumerate() {
            let name = free_name(&mut taken, &format!("{base}{}", index + 1));
            crate::commands::rename_scene_node(world, fragment, &name);
            crate::commands::set_parent(world, fragment, Some(group));
        }
        spawned.insert(0, group);
    }
    Placed {
        removed: Some(target),
        created,
        spawned,
    }
}

/// `entity`'s `Name`, or `"Brush"` when it has none.
pub(crate) fn name_or_default(world: &World, entity: Entity) -> String {
    world
        .get::<Name>(entity)
        .map_or_else(|| "Brush".to_string(), |name| name.as_str().to_string())
}

/// Every scene name in use except `entity`'s own, which it is giving up.
pub(crate) fn names_besides(
    world: &mut World,
    entity: Entity,
) -> std::collections::HashSet<String> {
    let mut query = world.query_filtered::<(Entity, &Name), Without<crate::EditorEntity>>();
    query
        .iter(world)
        .filter(|(other, _)| *other != entity)
        .map(|(_, name)| name.as_str().to_string())
        .collect()
}

/// `wanted`, or the next free numbered form of it, claimed in `taken`.
pub(crate) fn free_name(taken: &mut std::collections::HashSet<String>, wanted: &str) -> String {
    crate::entity_ops::claim_free_name(taken, wanted).unwrap_or_else(|| wanted.to_string())
}

/// Rewrite `target`'s brush as the world-space `fragment`, keeping its
/// entity, node id and transform.
fn merge_into(world: &mut World, target: Entity, (brush, placed): &(Brush, Transform)) {
    let global = unsorted_global(world, target).unwrap_or_default();
    let to_local =
        global.affine().inverse() * bevy::math::Affine3A::from_translation(placed.translation);
    let (faces, topology) = jackdaw_csg::brush_to_world(&brush.faces, &brush.topology, to_local);
    crate::brush::replace_brush(world, target, &Brush { faces, topology });
}

/// A new empty node named `name` at `slot`, placed at `target`'s world
/// position with no turn or scale, for its fragments to go under.
fn spawn_fragment_group(
    world: &mut World,
    target: Entity,
    slot: crate::commands::HierarchyLocation,
    name: &str,
) -> Entity {
    let at = Transform::from_translation(
        unsorted_global(world, target).map_or(Vec3::ZERO, |global| global.translation()),
    );
    let name = name.to_string();
    let group = crate::entity_ops::spawn_shaped_under(
        world,
        crate::entity_ops::EntityTemplate::Empty,
        None,
        move |entity| {
            entity.insert((Name::new(name), at));
        },
    );
    crate::commands::set_hierarchy_location(world, group, slot);
    crate::commands::deselect_entities(world, &[group]);
    group
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
    description = "Cut brushes away from other brushes. Naming `target` (or `targets`) and \
                   `cutters` works whatever is selected and cuts only the targets, all in one \
                   undo step: each is replaced by its fragments as `fragments` says, or removed \
                   when nothing is left, and a removed target's other children move up to its \
                   parent. Without them, the selected brushes are the cutters and every \
                   unselected brush they overlap is a target; at least one brush must be \
                   selected. Every result carries a report: outcome, fragments, volume before \
                   and after, bounds, watertight, and the cut, coplanar and degenerate face \
                   counts.",
    is_available = can_run_csg_subtract,
    params(
        target(Entity, doc = "Brush to cut. Every other brush is left alone."),
        targets(
            Entity,
            doc = "A group whose brushes are all cut, in place of `target`. Brushes outside it \
                   are left alone."
        ),
        cutters(
            Entity,
            doc = "Brush to cut with, or a group whose brushes all cut."
        ),
        keep_cutters(
            bool,
            doc = "Keep the cutters after the cut. Defaults to true."
        ),
        fragments(
            String,
            doc = "What a cut target becomes: `loose` brushes in its place, a `group` named \
                   after it holding `<target> 1`, `<target> 2`..., or `merge`: the target itself \
                   when what is left is one connected solid, keeping its id, else a group. \
                   Defaults to `loose`."
        ),
        dry_run(
            bool,
            doc = "Work the result out and report it without changing the scene. Defaults to false."
        ),
        cut_faces(
            String,
            doc = "What the faces the cut creates wear: `cutter`, `target` (its most worn \
                   material), a palette name or a material file path. Defaults to `cutter`."
        ),
        align_cut_uvs(
            bool,
            doc = "Project the faces the cut creates world-aligned, so they tile on from their \
                   neighbours. Defaults to true."
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
    description = "Keep only the solid brushes share. Naming `target` (or `targets`) and \
                   `cutters` works whatever is selected and trims only the targets, all in one \
                   undo step: each is replaced by its parts inside any of the cutters, or \
                   removed when there are none, and its other children move up to its parent. \
                   Without them, the selected brushes are replaced by the solid all of them \
                   share; at least two brushes must be selected. Every result carries the same \
                   report as CSG Subtract.",
    is_available = can_run_csg_intersect,
    params(
        target(Entity, doc = "Brush to trim. Every other brush is left alone."),
        targets(
            Entity,
            doc = "A group whose brushes are all trimmed, in place of `target`. Brushes outside \
                   it are left alone."
        ),
        cutters(
            Entity,
            doc = "Brush to trim to, or a group whose brushes together make the shape kept."
        ),
        keep_cutters(
            bool,
            doc = "Keep the cutters after the trim. Defaults to true with `target` and \
                   `cutters`; on the selection, true replaces only the first selected brush."
        ),
        dry_run(
            bool,
            doc = "Work the result out and report it without changing the scene. Defaults to false."
        ),
        cut_faces(
            String,
            doc = "What the faces the cut creates wear: `cutter`, `target` (its most worn \
                   material), a palette name or a material file path. Defaults to `cutter`."
        ),
        align_cut_uvs(
            bool,
            doc = "Project the faces the cut creates world-aligned, so they tile on from their \
                   neighbours. Defaults to true."
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
#[doc(hidden)]
pub fn env_allows_brush_op(
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
