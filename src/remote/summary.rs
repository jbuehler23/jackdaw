//! Compact reads of the open scene for a caller verifying its own edits:
//! `jackdaw/scene_summary` answers one row per node in columns the caller
//! picks, or the rows that changed since a revision it was handed, and
//! `jackdaw/check` evaluates geometric assertions and reports what each one
//! measured. Neither writes anything or leaves an undo entry.

use std::collections::{HashMap, HashSet, VecDeque};

use bevy::camera::primitives::Aabb;
use bevy::ecs::entity::EntityHashSet;
use bevy::prelude::*;
use bevy::remote::{BrpError, BrpResult};
use jackdaw_commands::CommandHistory;
use jackdaw_geometry::ModifierStack;
use jackdaw_scene_types::{Brush, GltfSource, Terrain, unsorted_global};
use serde_json::{Value, json};

use super::server::{
    belongs_to_the_scene, check_enabled, depth_param, entity_from_params, invalid_params,
    is_scene_descendant, root_param,
};

/// Tolerance an assertion uses when the caller names none, in metres.
const DEFAULT_TOLERANCE: f32 = 0.01;

/// How many earlier summaries a `since_revision` read can diff against.
const SNAPSHOTS_KEPT: usize = 16;

/// How many offending positions or pairs an assertion lists.
const LISTED: usize = 4;

/// Two intervals closer than this share no area.
const CONTACT_EPSILON: f32 = 1e-4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Field {
    Name,
    Parent,
    Pos,
    Rot,
    Size,
    Bounds,
    Material,
    Kind,
    Faces,
    Verts,
    Modifiers,
}

const FIELDS: [(&str, Field); 11] = [
    ("name", Field::Name),
    ("parent", Field::Parent),
    ("pos", Field::Pos),
    ("rot", Field::Rot),
    ("size", Field::Size),
    ("bounds", Field::Bounds),
    ("material", Field::Material),
    ("kind", Field::Kind),
    ("faces", Field::Faces),
    ("verts", Field::Verts),
    ("modifiers", Field::Modifiers),
];

const DEFAULT_FIELDS: [Field; 5] = [
    Field::Name,
    Field::Parent,
    Field::Kind,
    Field::Pos,
    Field::Size,
];

/// What a node is, as `kind` reports it and a filter names it.
const KINDS: [&str; 9] = [
    "brush", "terrain", "prefab", "light", "camera", "model", "mesh", "ui", "empty",
];

impl Field {
    fn name(self) -> &'static str {
        FIELDS
            .iter()
            .find(|(_, field)| *field == self)
            .map_or("", |(name, _)| name)
    }
}

/// Which nodes a read covers: an entity reference, a name prefix and a kind,
/// each optional.
#[derive(Default, Clone)]
struct NodeFilter {
    reference: Option<String>,
    matched: Option<EntityHashSet>,
    name_prefix: Option<String>,
    kind: Option<String>,
}

impl NodeFilter {
    /// `given` is an entity reference such as `"#roof"`, or
    /// `{ref?, name_prefix?, kind?}`.
    fn from_params(world: &mut World, given: Option<&Value>) -> Result<Self, BrpError> {
        let Some(given) = given.filter(|given| !given.is_null()) else {
            return Ok(Self::default());
        };
        let mut filter = Self::default();
        if let Some(reference) = given.as_str() {
            filter.reference = Some(reference.to_string());
            return filter.matched_by(world);
        }
        let Some(object) = given.as_object() else {
            return Err(invalid_params(format!(
                "`filter` is a reference or {{ref?, name_prefix?, kind?}}, not {given}"
            )));
        };
        for (key, value) in object {
            match key.as_str() {
                "ref" => {
                    let reference = value
                        .as_str()
                        .ok_or_else(|| invalid_params("`ref` is an entity reference"))?;
                    filter.reference = Some(reference.to_string());
                }
                "name_prefix" => {
                    filter.name_prefix = Some(
                        value
                            .as_str()
                            .ok_or_else(|| invalid_params("`name_prefix` is a string"))?
                            .to_string(),
                    );
                }
                "kind" => {
                    let kind = value
                        .as_str()
                        .ok_or_else(|| invalid_params("`kind` is a string"))?;
                    if !KINDS.contains(&kind) {
                        return Err(invalid_params(format!(
                            "unknown kind `{kind}`; one of {}",
                            KINDS.join(", ")
                        )));
                    }
                    filter.kind = Some(kind.to_string());
                }
                other => {
                    return Err(invalid_params(format!(
                        "unknown filter `{other}`; use ref, name_prefix or kind"
                    )));
                }
            }
        }
        filter.matched_by(world)
    }

    /// Resolve the reference, if any, once for the whole read.
    fn matched_by(mut self, world: &mut World) -> Result<Self, BrpError> {
        if let Some(reference) = &self.reference {
            self.matched = Some(
                super::server::reference_matches(world, reference)?
                    .into_iter()
                    .collect(),
            );
        }
        Ok(self)
    }

    fn admits(&self, world: &World, entity: Entity) -> bool {
        if self
            .matched
            .as_ref()
            .is_some_and(|matched| !matched.contains(&entity))
        {
            return false;
        }
        if let Some(prefix) = &self.name_prefix
            && !label(world, entity).is_some_and(|name| name.starts_with(prefix.as_str()))
        {
            return false;
        }
        // A prefab instance is a prefab whatever its root is, so `prefab`
        // matches every instance root while `kind` reports the root's own kind.
        self.kind.as_deref().is_none_or(|kind| {
            kind_of(world, entity) == kind
                || (kind == "prefab" && world.get::<crate::prefab::IsA>(entity).is_some())
        })
    }

    fn key(&self) -> Value {
        json!([self.reference, self.name_prefix, self.kind])
    }
}

/// The summaries taken so far, so a later read can answer with what changed.
#[derive(Resource, Default)]
struct SummarySnapshots(VecDeque<SummarySnapshot>);

struct SummarySnapshot {
    revision: u64,
    scope: Value,
    rows: Vec<Vec<Value>>,
}

/// The open scene as compact rows. `fields` picks the columns after `id`;
/// `root`, `depth` and `filter` narrow the nodes; `since_revision` answers
/// with only the rows added, changed and removed since that summary.
pub fn scene_summary_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    check_enabled(world)?;
    let params = params.unwrap_or(Value::Null);
    let fields = fields_from_params(params.get("fields"))?;
    let filter = NodeFilter::from_params(world, params.get("filter"))?;
    let depth = depth_param(&params)?;
    let root = root_param(world, &params)?;
    let since = match params.get("since_revision") {
        None | Some(Value::Null) => None,
        Some(given) => Some(
            given
                .as_u64()
                .ok_or_else(|| invalid_params("`since_revision` is a revision number"))?,
        ),
    };

    let scope = json!({
        "root": root.map(Entity::to_bits),
        "depth": depth,
        "fields": fields.iter().map(|field| field.name()).collect::<Vec<_>>(),
        "filter": filter.key(),
    });
    let earlier = match since {
        Some(revision) => Some(
            world
                .get_resource::<SummarySnapshots>()
                .and_then(|taken| {
                    taken
                        .0
                        .iter()
                        .find(|snapshot| snapshot.revision == revision && snapshot.scope == scope)
                })
                .map(|snapshot| snapshot.rows.clone())
                .ok_or_else(|| {
                    invalid_params(format!(
                        "no summary of this scope was taken at revision {revision}; \
                         ask again without `since_revision`"
                    ))
                })?,
        ),
        None => None,
    };

    let entities: Vec<Entity> = scene_nodes(world, root, depth)
        .into_iter()
        .filter(|entity| filter.admits(world, *entity))
        .collect();
    let rows: Vec<Vec<Value>> = entities
        .into_iter()
        .map(|entity| row(world, entity, &fields))
        .collect();
    let revision = world
        .get_resource::<CommandHistory>()
        .map_or(0, CommandHistory::revision);
    remember(world, revision, scope, rows.clone());

    let mut columns = vec!["id"];
    columns.extend(fields.iter().map(|field| field.name()));
    let Some(earlier) = earlier else {
        return Ok(json!({ "revision": revision, "columns": columns, "rows": rows }));
    };

    let before: HashMap<&Value, &Vec<Value>> = earlier.iter().map(|row| (&row[0], row)).collect();
    let now: HashSet<&Value> = rows.iter().map(|row| &row[0]).collect();
    let mut added = Vec::new();
    let mut changed = Vec::new();
    for row in &rows {
        match before.get(&row[0]) {
            None => added.push(row),
            Some(old) if *old != row => changed.push(row),
            Some(_) => {}
        }
    }
    let removed: Vec<&Value> = earlier
        .iter()
        .map(|row| &row[0])
        .filter(|id| !now.contains(id))
        .collect();
    Ok(json!({
        "revision": revision,
        "since": since,
        "columns": columns,
        "added": added,
        "changed": changed,
        "removed": removed,
    }))
}

fn fields_from_params(given: Option<&Value>) -> Result<Vec<Field>, BrpError> {
    let Some(given) = given.filter(|given| !given.is_null()) else {
        return Ok(DEFAULT_FIELDS.to_vec());
    };
    let names = given
        .as_array()
        .ok_or_else(|| invalid_params("`fields` is a list of field names"))?;
    names
        .iter()
        .map(|name| {
            let name = name.as_str().unwrap_or_default();
            FIELDS
                .iter()
                .find(|(known, _)| *known == name)
                .map(|(_, field)| *field)
                .ok_or_else(|| {
                    let known: Vec<&str> = FIELDS.iter().map(|(known, _)| *known).collect();
                    invalid_params(format!(
                        "unknown field `{name}`; one of {}",
                        known.join(", ")
                    ))
                })
        })
        .collect()
}

/// Keep `rows` as the baseline for `revision` and `scope`. The first summary
/// taken at a revision stays its baseline, so a later read at the same
/// revision cannot move it under a caller still diffing against it.
fn remember(world: &mut World, revision: u64, scope: Value, rows: Vec<Vec<Value>>) {
    let mut taken = world.get_resource_or_init::<SummarySnapshots>();
    if taken
        .0
        .iter()
        .any(|snapshot| snapshot.revision == revision && snapshot.scope == scope)
    {
        return;
    }
    taken.0.push_back(SummarySnapshot {
        revision,
        scope,
        rows,
    });
    while taken.0.len() > SNAPSHOTS_KEPT {
        taken.0.pop_front();
    }
}

/// The scene nodes under `root` (or every scene root) down to `depth`
/// generations, parents before their children, as the outliner draws them.
fn scene_nodes(world: &mut World, root: Option<Entity>, depth: u32) -> Vec<Entity> {
    let roots: Vec<Entity> = match root {
        Some(root) => vec![root]
            .into_iter()
            .filter(|entity| belongs_to_the_scene(world, *entity))
            .collect(),
        None => {
            let mut roots = super::server::scene_roots(world);
            roots.sort();
            roots
        }
    };
    let mut nodes = Vec::new();
    let mut stack: Vec<(Entity, u32)> = roots.into_iter().rev().map(|e| (e, depth)).collect();
    while let Some((entity, left)) = stack.pop() {
        nodes.push(entity);
        if left == 0 {
            continue;
        }
        let children: Vec<Entity> = world
            .get::<Children>(entity)
            .map(|children| children.iter().collect())
            .unwrap_or_default();
        stack.extend(
            children
                .into_iter()
                .rev()
                .filter(|child| is_scene_descendant(world, *child))
                .map(|child| (child, left - 1)),
        );
    }
    nodes
}

fn row(world: &World, entity: Entity, fields: &[Field]) -> Vec<Value> {
    let wants = |wanted: &[Field]| fields.iter().any(|field| wanted.contains(field));
    let global = wants(&[Field::Pos, Field::Rot])
        .then(|| unsorted_global(world, entity))
        .flatten();
    let bounds = wants(&[Field::Size, Field::Bounds])
        .then(|| world_bounds(world, entity))
        .flatten();
    let counts = wants(&[Field::Faces, Field::Verts])
        .then(|| brush_counts(world, entity))
        .flatten();
    let mut row = vec![json!(entity.to_bits())];
    row.extend(fields.iter().map(|field| {
        match field {
            Field::Name => json!(label(world, entity)),
            Field::Parent => json!(
                world
                    .get::<ChildOf>(entity)
                    .map(|parent| parent.parent().to_bits())
            ),
            Field::Pos => global.map_or(Value::Null, |global| vector(global.translation())),
            Field::Rot => global.map_or(Value::Null, |global| {
                let (yaw, pitch, roll) = global.rotation().to_euler(EulerRot::YXZ);
                vector(Vec3::new(yaw, pitch, roll) * 180.0 / std::f32::consts::PI)
            }),
            Field::Size => bounds.map_or(Value::Null, |(min, max)| vector(max - min)),
            Field::Bounds => {
                bounds.map_or(Value::Null, |(min, max)| json!([vector(min), vector(max)]))
            }
            Field::Material => material(world, entity),
            Field::Kind => json!(kind_of(world, entity)),
            Field::Faces => counts.map_or(Value::Null, |(faces, _)| json!(faces)),
            Field::Verts => counts.map_or(Value::Null, |(_, verts)| json!(verts)),
            Field::Modifiers => world
                .get::<ModifierStack>(entity)
                .filter(|stack| !stack.modifiers.is_empty())
                .map_or(Value::Null, |stack| {
                    stack
                        .modifiers
                        .iter()
                        .map(|entry| entry.modifier.kind_str())
                        .collect()
                }),
        }
    }));
    row
}

fn label(world: &World, entity: Entity) -> Option<String> {
    world
        .get::<Name>(entity)
        .map(|name| name.as_str().to_string())
        .or_else(|| crate::hierarchy::prefab_stem_label(world, entity))
}

fn kind_of(world: &World, entity: Entity) -> &'static str {
    if world.get::<Brush>(entity).is_some() {
        "brush"
    } else if world.get::<Terrain>(entity).is_some() {
        "terrain"
    } else if world.get::<crate::prefab::IsA>(entity).is_some() {
        "prefab"
    } else if world.get::<PointLight>(entity).is_some()
        || world.get::<SpotLight>(entity).is_some()
        || world.get::<DirectionalLight>(entity).is_some()
    {
        "light"
    } else if world.get::<Camera>(entity).is_some() {
        "camera"
    } else if world.get::<GltfSource>(entity).is_some() {
        "model"
    } else if world.get::<Mesh3d>(entity).is_some() {
        "mesh"
    } else if world
        .get::<jackdaw_scene_types::UiSceneRoot>(entity)
        .is_some()
        || world.get::<Node>(entity).is_some()
    {
        "ui"
    } else {
        "empty"
    }
}

/// A brush's corners in its own space as its modifiers evaluate them: the
/// mesh cache when the renderer has built one, else the stack evaluated here
/// over the authored geometry, which is what a headless editor has.
fn brush_corners(world: &World, entity: Entity, brush: &Brush) -> Vec<Vec3> {
    if let Some(cache) = world.get::<crate::brush::BrushMeshCache>(entity)
        && !cache.vertices.is_empty()
    {
        return cache.vertices.clone();
    }
    let (vertices, polygons) = authored_geometry(brush);
    let modifiers: Vec<&jackdaw_geometry::Modifier> = world
        .get::<ModifierStack>(entity)
        .map(|stack| {
            stack
                .modifiers
                .iter()
                .filter(|entry| entry.enabled)
                .map(|entry| &entry.modifier)
                .collect()
        })
        .unwrap_or_default();
    if modifiers.is_empty() || polygons.is_empty() {
        return vertices;
    }
    jackdaw_geometry::evaluate_modifier_stack(&vertices, &polygons, &brush.faces, &modifiers)
        .vertices
}

/// A brush's authored corners and face rings, from its topology or, for a
/// brush that carries planes alone, the plane intersections. A topology whose
/// rings run past its loops or corners yields its corners with no rings.
fn authored_geometry(brush: &Brush) -> (Vec<Vec3>, Vec<Vec<usize>>) {
    let topology = &brush.topology;
    if topology.vertices.is_empty() {
        return crate::brush::compute_brush_geometry_from_planes(&brush.faces);
    }
    let vertices: Vec<Vec3> = topology
        .vertices
        .iter()
        .map(|vertex| vertex.position)
        .collect();
    let rings: Option<Vec<Vec<usize>>> = topology
        .polygons
        .iter()
        .map(|polygon| {
            let start = polygon.loop_start as usize;
            topology
                .loops
                .get(start..start + polygon.loop_total as usize)
                .filter(|ring| {
                    ring.iter()
                        .all(|corner| (corner.vert as usize) < vertices.len())
                })
                .map(|ring| ring.iter().map(|corner| corner.vert as usize).collect())
        })
        .collect();
    (vertices, rings.unwrap_or_default())
}

fn brush_counts(world: &World, entity: Entity) -> Option<(usize, usize)> {
    let brush = world.get::<Brush>(entity)?;
    if brush.topology.polygons.is_empty() {
        let (corners, faces) = crate::brush::compute_brush_geometry_from_planes(&brush.faces);
        Some((faces.len(), corners.len()))
    } else {
        Some((brush.topology.polygons.len(), brush.topology.vertices.len()))
    }
}

/// The world-space box around a node and everything under it: a brush's
/// evaluated corners through its transform, or the culling bounds of a mesh.
/// `None` when nothing in the subtree has either.
pub(super) fn world_bounds(world: &World, entity: Entity) -> Option<(Vec3, Vec3)> {
    crate::view_ops::subtree_bounds(
        entity,
        |at| {
            let drawn: &[Entity] = world
                .get::<crate::brush::BrushMeshCache>(at)
                .map_or(&[], |cache| cache.chunk_entities.as_slice());
            world
                .get::<Children>(at)
                .map(|children| {
                    children
                        .iter()
                        .filter(|child| !drawn.contains(child))
                        .collect()
                })
                .unwrap_or_default()
        },
        |at| own_bounds(world, at),
    )
}

fn own_bounds(world: &World, entity: Entity) -> Option<(Vec3, Vec3)> {
    if let Some(brush) = world.get::<Brush>(entity) {
        let global = unsorted_global(world, entity)?;
        let mut points = brush_corners(world, entity, brush)
            .into_iter()
            .map(|corner| global.transform_point(corner));
        let first = points.next()?;
        return Some(points.fold((first, first), |(min, max), point| {
            (min.min(point), max.max(point))
        }));
    }
    if world.get::<crate::ViewDependentBounds>(entity).is_some() {
        return None;
    }
    let aabb = world.get::<Aabb>(entity)?;
    let global =
        unsorted_global(world, entity).or_else(|| world.get::<GlobalTransform>(entity).copied())?;
    Some(crate::view_ops::aabb_world_bounds(&global, aabb))
}

/// The material names a node draws with: one name, a list when its faces
/// differ, or null when nothing names them.
fn material(world: &World, entity: Entity) -> Value {
    let handles: Vec<Handle<StandardMaterial>> = if let Some(brush) = world.get::<Brush>(entity) {
        brush
            .faces
            .iter()
            .map(|face| face.material.clone())
            .collect()
    } else if let Some(material) = world.get::<MeshMaterial3d<StandardMaterial>>(entity) {
        vec![material.0.clone()]
    } else {
        Vec::new()
    };
    let registry = world.get_resource::<crate::material_assets::MaterialRegistry>();
    let mut names: Vec<String> = Vec::new();
    for handle in &handles {
        let name = registry
            .and_then(|registry| registry.name_of(handle))
            .map(str::to_string)
            .or_else(|| handle.path().map(ToString::to_string));
        if let Some(name) = name
            && !names.contains(&name)
        {
            names.push(name);
        }
    }
    match names.len() {
        0 => Value::Null,
        1 => json!(names[0]),
        _ => json!(names),
    }
}

pub(super) fn round(value: f32) -> Value {
    let rounded = (f64::from(value) * 1000.0).round() / 1000.0;
    json!(if rounded == 0.0 { 0.0 } else { rounded })
}

pub(super) fn vector(value: Vec3) -> Value {
    json!([round(value.x), round(value.y), round(value.z)])
}

/// Evaluate each assertion in `assertions` against the open scene, answering
/// pass or fail with the value it measured. An assertion naming no node, or
/// malformed, refuses the whole call; one that cannot measure what it names
/// fails alone with a `reason`.
pub fn check_handler(In(params): In<Option<Value>>, world: &mut World) -> BrpResult {
    check_enabled(world)?;
    let params = params.unwrap_or(Value::Null);
    let assertions = params
        .get("assertions")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_params("expected {\"assertions\": [{\"<kind>\": {...}}]}"))?;
    run_checks(world, assertions)
}

/// Evaluate `assertions` as `jackdaw/check` does and answer what it answers.
pub(super) fn run_checks(world: &mut World, assertions: &[Value]) -> BrpResult {
    let mut results = Vec::with_capacity(assertions.len());
    for (index, assertion) in assertions.iter().enumerate() {
        let result = evaluate(world, assertion)
            .map_err(|err| invalid_params(format!("assertion {index}: {}", err.message)))?;
        results.push(result);
    }
    let passed = results
        .iter()
        .filter(|result| result["pass"] == json!(true))
        .count();
    Ok(json!({
        "passed": passed,
        "failed": results.len() - passed,
        "results": results,
    }))
}

/// One `jackdaw/check` assertion: measures what its arguments name.
type Assertion = fn(&mut World, &Value) -> Result<Value, BrpError>;

/// The assertions `jackdaw/check` knows, by kind.
const ASSERTIONS: [(&str, Assertion); 6] = [
    ("rests_on", rests_on),
    ("inside", inside),
    ("no_overlap", no_overlap),
    ("bounds", bounds),
    ("count", count),
    ("watertight", watertight),
];

/// An assertion's kind, arguments and evaluator, refusing one that is not
/// shaped `{"<kind>": {...}}` with a kind `jackdaw/check` knows.
pub(super) fn check_shape(assertion: &Value) -> Result<(&str, &Value, Assertion), BrpError> {
    let (kind, args) = assertion
        .as_object()
        .filter(|object| object.len() == 1)
        .and_then(|object| object.iter().next())
        .ok_or_else(|| {
            invalid_params(format!("expected {{\"<kind>\": {{...}}}}, not {assertion}"))
        })?;
    let (kind, evaluate) = ASSERTIONS
        .into_iter()
        .find(|(known, _)| *known == kind.as_str())
        .ok_or_else(|| {
            let kinds: Vec<&str> = ASSERTIONS.iter().map(|(kind, _)| *kind).collect();
            invalid_params(format!(
                "unknown assertion `{kind}`; one of {}",
                kinds.join(", ")
            ))
        })?;
    Ok((kind, args, evaluate))
}

fn evaluate(world: &mut World, assertion: &Value) -> Result<Value, BrpError> {
    let (kind, args, evaluate) = check_shape(assertion)?;
    let mut result = evaluate(world, args)?;
    result["check"] = json!(kind);
    Ok(result)
}

/// The node an assertion argument names, by id or by name.
fn named(world: &mut World, args: &Value, key: &str) -> Result<Entity, BrpError> {
    match args.get(key) {
        Some(Value::Number(bits)) => entity_from_params(world, &json!({ "entity": bits })),
        Some(Value::String(name)) => entity_from_params(world, &json!({ "name": name })),
        _ => Err(invalid_params(format!("`{key}` is an entity id or a name"))),
    }
}

/// A failed assertion that could not measure, and why.
fn unmeasured(reason: String) -> Value {
    json!({ "pass": false, "reason": reason })
}

fn no_bounds(entity: Entity) -> Value {
    unmeasured(format!("entity {} has no bounds", entity.to_bits()))
}

fn tolerance(args: &Value) -> f32 {
    args.get("tolerance")
        .and_then(Value::as_f64)
        .map_or(DEFAULT_TOLERANCE, |tolerance| tolerance as f32)
}

fn point(args: &Value, key: &str) -> Result<Option<Vec3>, BrpError> {
    let Some(given) = args.get(key).filter(|given| !given.is_null()) else {
        return Ok(None);
    };
    let parts: Option<Vec<f32>> = given
        .as_array()
        .filter(|parts| parts.len() == 3)
        .map(|parts| {
            parts
                .iter()
                .filter_map(Value::as_f64)
                .map(|part| part as f32)
                .collect()
        });
    match parts {
        Some(parts) if parts.len() == 3 => Ok(Some(Vec3::new(parts[0], parts[1], parts[2]))),
        _ => Err(invalid_params(format!("`{key}` is [x, y, z]"))),
    }
}

/// Whether two intervals share some length, not merely an end.
fn spans_overlap(a_min: f32, a_max: f32, b_min: f32, b_max: f32) -> bool {
    a_min < b_max - CONTACT_EPSILON && b_min < a_max - CONTACT_EPSILON
}

/// `entity`'s bottom against the top of `on` (or the ground of a terrain
/// under the middle of its footprint), with gravity along -Y. Passes when the
/// gap is within `tolerance` and the two overlap seen from above.
fn rests_on(world: &mut World, args: &Value) -> Result<Value, BrpError> {
    let entity = named(world, args, "entity")?;
    let on = named(world, args, "on")?;
    let tolerance = tolerance(args);
    let Some((min, max)) = world_bounds(world, entity) else {
        return Ok(no_bounds(entity));
    };
    let (top, overlap_xz) = if let Some(terrain) = world.get::<Terrain>(on) {
        let middle = (min + max) * 0.5;
        (ground_under(world, on, terrain, middle.xz()), true)
    } else {
        let Some((on_min, on_max)) = world_bounds(world, on) else {
            return Ok(no_bounds(on));
        };
        let overlap = spans_overlap(min.x, max.x, on_min.x, on_max.x)
            && spans_overlap(min.z, max.z, on_min.z, on_max.z);
        (Some(on_max.y), overlap)
    };
    let Some(top) = top else {
        return Ok(unmeasured(format!(
            "terrain {} has no ground under entity {}",
            on.to_bits(),
            entity.to_bits()
        )));
    };
    let gap = min.y - top;
    Ok(json!({
        "pass": overlap_xz && gap.abs() <= tolerance,
        "gap": round(gap),
        "overlap_xz": overlap_xz,
    }))
}

/// The world height of `terrain`'s surface under a world XZ point.
fn ground_under(world: &World, entity: Entity, terrain: &Terrain, point: Vec2) -> Option<f32> {
    let global = unsorted_global(world, entity)?;
    let store = world.get_resource::<crate::terrain::TerrainDataStore>()?;
    crate::terrain::ground::height_on(terrain, &global, store, point)
}

/// How far `entity`'s box reaches outside the box of `of`, along the worst
/// axis.
fn inside(world: &mut World, args: &Value) -> Result<Value, BrpError> {
    let entity = named(world, args, "entity")?;
    let of = named(world, args, "of")?;
    let tolerance = tolerance(args);
    let Some((min, max)) = world_bounds(world, entity) else {
        return Ok(no_bounds(entity));
    };
    let Some((of_min, of_max)) = world_bounds(world, of) else {
        return Ok(no_bounds(of));
    };
    let outside = (of_min - min)
        .max(max - of_max)
        .max(Vec3::ZERO)
        .max_element();
    Ok(json!({ "pass": outside <= tolerance, "outside": round(outside) }))
}

/// The deepest overlap between any two of `entities`, and the pairs that
/// overlap by more than `tolerance`.
fn no_overlap(world: &mut World, args: &Value) -> Result<Value, BrpError> {
    let names = args
        .get("entities")
        .and_then(Value::as_array)
        .ok_or_else(|| invalid_params("`entities` lists two or more entity ids or names"))?;
    let tolerance = tolerance(args);
    let mut entities: Vec<Entity> = Vec::with_capacity(names.len());
    for name in names {
        let entity = named(world, &json!({ "entity": name }), "entity")?;
        if !entities.contains(&entity) {
            entities.push(entity);
        }
    }
    if entities.len() < 2 {
        return Err(invalid_params(
            "`entities` lists two or more distinct entity ids or names",
        ));
    }
    let mut boxes = Vec::with_capacity(entities.len());
    for entity in entities {
        let Some(bounds) = world_bounds(world, entity) else {
            return Ok(no_bounds(entity));
        };
        boxes.push((entity, bounds));
    }
    let mut deepest = 0.0_f32;
    let mut pairs = Vec::new();
    for (index, (a, (a_min, a_max))) in boxes.iter().enumerate() {
        for (b, (b_min, b_max)) in &boxes[index + 1..] {
            let depth = (a_max.min(*b_max) - a_min.max(*b_min))
                .min_element()
                .max(0.0);
            deepest = deepest.max(depth);
            if depth > tolerance {
                pairs.push(json!([a.to_bits(), b.to_bits(), round(depth)]));
            }
        }
    }
    let overlapping = pairs.len();
    pairs.truncate(LISTED);
    Ok(json!({
        "pass": overlapping == 0,
        "depth": round(deepest),
        "overlapping": overlapping,
        "pairs": pairs,
    }))
}

/// Whether `entity`'s box stays above `min` and below `max`.
fn bounds(world: &mut World, args: &Value) -> Result<Value, BrpError> {
    let entity = named(world, args, "entity")?;
    let tolerance = tolerance(args);
    let low = point(args, "min")?;
    let high = point(args, "max")?;
    if low.is_none() && high.is_none() {
        return Err(invalid_params("give `min`, `max` or both"));
    }
    let Some((min, max)) = world_bounds(world, entity) else {
        return Ok(no_bounds(entity));
    };
    let pass = low.is_none_or(|low| (min - low).min_element() >= -tolerance)
        && high.is_none_or(|high| (high - max).min_element() >= -tolerance);
    Ok(json!({ "pass": pass, "bounds": [vector(min), vector(max)] }))
}

/// How many scene nodes match `filter`, against `eq`, `min` or `max`.
fn count(world: &mut World, args: &Value) -> Result<Value, BrpError> {
    let filter = NodeFilter::from_params(world, args.get("filter"))?;
    let bound = |key: &str| args.get(key).and_then(Value::as_u64);
    let (eq, min, max) = (bound("eq"), bound("min"), bound("max"));
    if eq.is_none() && min.is_none() && max.is_none() {
        return Err(invalid_params("give `eq`, `min` or `max`"));
    }
    let count = scene_nodes(world, None, u32::MAX)
        .into_iter()
        .filter(|entity| filter.admits(world, *entity))
        .count() as u64;
    let pass = eq.is_none_or(|eq| count == eq)
        && min.is_none_or(|min| count >= min)
        && max.is_none_or(|max| count <= max);
    Ok(json!({ "pass": pass, "count": count }))
}

/// Whether a brush's authored faces close up: every edge shared by exactly two
/// faces. Reports how many edges are open and where the first few are.
fn watertight(world: &mut World, args: &Value) -> Result<Value, BrpError> {
    let entity = named(world, args, "entity")?;
    let Some(brush) = world.get::<Brush>(entity) else {
        return Ok(unmeasured(format!(
            "entity {} is not a brush",
            entity.to_bits()
        )));
    };
    let topology = &brush.topology;
    if topology.polygons.is_empty() {
        return Ok(unmeasured(format!(
            "brush {} has no authored faces to judge",
            entity.to_bits()
        )));
    }
    let open = match topology.open_edges() {
        Ok(open) => open,
        Err(face) => {
            return Ok(unmeasured(format!(
                "brush {} face {face} names corners outside its loops",
                entity.to_bits()
            )));
        }
    };
    let global = unsorted_global(world, entity).unwrap_or_default();
    let position = |index: u32| {
        topology
            .vertices
            .get(index as usize)
            .map_or(Vec3::ZERO, |vertex| vertex.position)
    };
    let at: Vec<Value> = open
        .iter()
        .take(LISTED)
        .map(|(a, b)| vector(global.transform_point((position(*a) + position(*b)) * 0.5)))
        .collect();
    Ok(json!({ "pass": open.is_empty(), "open_edges": open.len(), "at": at }))
}
