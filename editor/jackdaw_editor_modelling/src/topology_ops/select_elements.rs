//! `brush.select_elements`: select a brush's faces, edges or vertices by index
//! or by rule, without a pointer. The rules resolve in
//! [`jackdaw_select::address`]; this adapter reads the parameters, enters the
//! edit mode and writes the result into [`BrushSelection`].

use bevy::math::Affine3A;
use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_scene_types::Brush;
use jackdaw_select::address::{
    self, Combine, Cone, DEFAULT_EPSILON, DEFAULT_TOLERANCE_DEG, ElementKind, Extreme, InsideBox,
    Selector,
};
use serde_json::Value;

pub use jackdaw_editor_core::brush::refuse;
use jackdaw_editor_core::brush::{BrushEditMode, BrushHalfedge, BrushSelection, EditMode};

/// How many selected indices a report lists before it only counts.
const LISTED: usize = 32;

const WHERE_KEYS: [&str; 8] = [
    "normal",
    "direction",
    "tolerance_deg",
    "extreme",
    "epsilon",
    "inside_box",
    "loop_of",
    "all",
];

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<BrushSelectElementsOp>();
}

/// Select faces, edges or vertices of a brush by index or by rule, entering
/// the matching edit mode on it. Selection is not an undo step.
#[operator(
    id = "brush.select_elements",
    label = "Select Elements",
    description = "Select a brush's faces, edges or vertices by index or by rule, entering \
                   that edit mode on it.",
    allows_undo = false,
    params(
        entity(
            Entity,
            doc = "Brush to select on. Defaults to the brush being edited, else the selected one."
        ),
        kind(
            String,
            doc = "face, edge or vertex. Defaults to the edit mode in use, else face."
        ),
        indices(
            String,
            doc = "Element indices, as a list or comma-separated. Edges index the \
                   `jackdaw/brush_topology` edge list."
        ),
        r#where(
            String,
            doc = "Rules every picked element meets, as an object: `normal` (\"+y\" or \
                   [x,y,z]) with `tolerance_deg` (default 5); `direction` (edges along an \
                   axis, either way); `extreme` (\"max_y\", \"min_x\", ...) with `epsilon` \
                   (default 0.001); `inside_box` {min, max, space: local|world}, every \
                   vertex inside; `loop_of` (edge index); `all` (true). Brush-local unless \
                   stated. With `indices`, both must hold."
        ),
        mode(
            String,
            default = "set",
            doc = "set replaces the selection, add extends it, remove takes from it."
        ),
    )
)]
pub(crate) fn brush_select_elements(
    params: In<OperatorParameters>,
    world: &mut World,
) -> OperatorResult {
    let request = match ElementRequest::read(world, &params) {
        Ok(request) => request,
        Err(why) => return refuse(world, "Select Elements", why),
    };
    let resolved = match resolve_on(world, request.entity, request.kind, &request.selector) {
        Ok(resolved) => resolved,
        Err(why) => return refuse(world, "Select Elements", why),
    };
    if let Some(why) = element_mode_blocked(world) {
        return refuse(world, "Select Elements", why);
    }
    enter_element_mode(world, request.entity, request.kind);
    let changed = write_selection(world, request.entity, request.kind, &resolved, request.mode);
    let selected = selected_indices(world, request.entity, request.kind);
    let revision = world
        .get::<Brush>(request.entity)
        .map_or(0, |brush| address::topology_revision(&brush.topology));
    report_to_caller(
        world,
        format!(
            "{} {} selected: {} (topology revision {revision})",
            selected.len(),
            plural(request.kind),
            listed(&selected)
        ),
    );
    set_outcome(
        world,
        if changed {
            Outcome::Changed
        } else {
            Outcome::Unchanged
        },
    );
    OperatorResult::Finished
}

/// A `brush.select_elements` call, checked.
struct ElementRequest {
    entity: Entity,
    kind: ElementKind,
    selector: Selector,
    mode: Combine,
}

impl ElementRequest {
    fn read(world: &World, params: &OperatorParameters) -> Result<Self, String> {
        let kind = match params.get("kind") {
            None => current_kind(world),
            Some(_) => parse_kind(params.as_str("kind").unwrap_or_default())?,
        };
        let mode = match params.get("mode") {
            None => Combine::Set,
            Some(_) => parse_mode(params.as_str("mode").unwrap_or_default())?,
        };
        let mut selector = selector_from_params(params)?;
        if params.contains_key("indices") {
            selector.indices = Some(parse_indices(params.get("indices"))?);
        }
        let entity = edit_target(world, params.get("entity"))?;
        Ok(Self {
            entity,
            kind,
            selector,
            mode,
        })
    }
}

/// The `where` rules a call gives, or none.
pub(crate) fn selector_from_params(params: &OperatorParameters) -> Result<Selector, String> {
    match params.get("where") {
        None => Ok(Selector::default()),
        Some(_) => parse_where(params.as_str("where").unwrap_or_default()),
    }
}

/// Why the edit mode may not change now, if it may not.
pub(crate) fn element_mode_blocked(world: &mut World) -> Option<String> {
    world
        .run_system_cached(
            |guards: jackdaw_editor_core::edit_mode_ops::EditModeGuards| {
                guards.blocked().map(str::to_string)
            },
        )
        .unwrap_or_else(|err| Some(format!("the editor state could not be read: {err}")))
}

/// The brush a call acts on: the `entity` it names, else the brush being
/// edited, else the selected brush.
pub(crate) fn edit_target(
    world: &World,
    given: Option<&jackdaw_scene_types::PropertyValue>,
) -> Result<Entity, String> {
    use jackdaw_scene_types::PropertyValue;
    let entity = match given {
        Some(PropertyValue::Entity(entity)) => {
            jackdaw_editor_core::scene_node(world, *entity, "entity")?
        }
        Some(other) => return Err(format!("`entity` {other:?} names no node in this scene")),
        None => {
            let is_brush = |entity: &Entity| world.get::<Brush>(*entity).is_some();
            world
                .get_resource::<BrushSelection>()
                .and_then(|selection| selection.active_brush)
                .filter(is_brush)
                .or_else(|| {
                    world
                        .get_resource::<jackdaw_editor_core::selection::Selection>()
                        .and_then(jackdaw_editor_core::selection::Selection::primary)
                        .filter(is_brush)
                })
                .ok_or("name a brush with `entity`; none is being edited or selected")?
        }
    };
    let Some(brush) = world.get::<Brush>(entity) else {
        return Err(format!("`entity` {entity} is not a brush"));
    };
    if brush.topology.polygons.is_empty() {
        return Err(format!("brush {entity} has no faces to select"));
    }
    Ok(entity)
}

/// The `kind` element indices of `entity` that `selector` picks.
pub(crate) fn resolve_on(
    world: &World,
    entity: Entity,
    kind: ElementKind,
    selector: &Selector,
) -> Result<Vec<usize>, String> {
    let brush = world
        .get::<Brush>(entity)
        .ok_or_else(|| format!("`entity` {entity} is not a brush"))?;
    let to_world = jackdaw_scene_types::unsorted_global(world, entity)
        .map_or(Affine3A::IDENTITY, |global| global.affine());
    address::resolve(&brush.topology, kind, selector, to_world).map_err(|err| err.to_string())
}

/// Put `entity` in the edit mode for `kind`, as the brush being edited, with
/// its half-edge mesh in place so a mesh operator dispatched straight after
/// finds it. Staying in the same mode keeps the selection; switching mode
/// clears it, as the mode keys do.
pub(crate) fn enter_element_mode(world: &mut World, entity: Entity, kind: ElementKind) {
    jackdaw_api_internal::selection::select_for_edit(world, entity);
    if let Some(mut draw) =
        world.get_resource_mut::<jackdaw_editor_core::draw_brush::DrawBrushState>()
    {
        draw.cancel();
    }
    let target = match kind {
        ElementKind::Face => BrushEditMode::Face,
        ElementKind::Edge => BrushEditMode::Edge,
        ElementKind::Vertex => BrushEditMode::Vertex,
    };
    let current = *world.resource::<EditMode>();
    let mut selection = world.resource_mut::<BrushSelection>();
    match current {
        EditMode::BrushEdit(mode) if mode == target => {}
        EditMode::BrushEdit(BrushEditMode::Face | BrushEditMode::Edge | BrushEditMode::Vertex) => {
            selection.clear_sub_selections();
        }
        _ => selection.clear(),
    }
    selection.sub_mut(entity);
    if selection.active_brush != Some(entity) {
        selection.active_brush = Some(entity);
    }
    if current != EditMode::BrushEdit(target) {
        *world.resource_mut::<EditMode>() = EditMode::BrushEdit(target);
    }
    if world.get::<BrushHalfedge>(entity).is_none()
        && let Some(brush) = world.get::<Brush>(entity)
    {
        let halfedge = BrushHalfedge::from_topology(&brush.topology);
        world.entity_mut(entity).insert(halfedge);
    }
}

/// Combine `resolved` into `entity`'s selection of `kind`. Whether it changed.
pub(crate) fn write_selection(
    world: &mut World,
    entity: Entity,
    kind: ElementKind,
    resolved: &[usize],
    mode: Combine,
) -> bool {
    let pairs: Vec<(usize, usize)> = match kind {
        ElementKind::Edge => {
            let Some(brush) = world.get::<Brush>(entity) else {
                return false;
            };
            resolved
                .iter()
                .filter_map(|&edge| brush.topology.edges.get(edge))
                .map(|edge| (edge.v[0] as usize, edge.v[1] as usize))
                .collect()
        }
        _ => Vec::new(),
    };
    let mut selection = world.resource_mut::<BrushSelection>();
    let mut changed = false;
    if mode == Combine::Set {
        for (other, sub) in selection.bypass_change_detection().brushes.iter_mut() {
            if *other != entity && !sub_is_empty(sub) {
                sub.clear();
                changed = true;
            }
        }
    }
    let sub = selection.bypass_change_detection().sub_mut(entity);
    changed |= match kind {
        ElementKind::Face => {
            let next = address::combine(&sub.faces, resolved, mode);
            replace(&mut sub.faces, next)
        }
        ElementKind::Vertex => {
            let next = address::combine(&sub.vertices, resolved, mode);
            replace(&mut sub.vertices, next)
        }
        ElementKind::Edge => {
            let next = address::combine(&sub.edges, &pairs, mode);
            replace(&mut sub.edges, next)
        }
    };
    if changed {
        selection.set_changed();
    }
    changed
}

fn sub_is_empty(sub: &jackdaw_editor_core::brush::BrushSubSelection) -> bool {
    sub.faces.is_empty() && sub.vertices.is_empty() && sub.edges.is_empty()
}

fn replace<T: PartialEq>(slot: &mut Vec<T>, next: Vec<T>) -> bool {
    if *slot == next {
        return false;
    }
    *slot = next;
    true
}

/// `entity`'s selected `kind` elements as indices, edges into the topology
/// edge list.
fn selected_indices(world: &World, entity: Entity, kind: ElementKind) -> Vec<usize> {
    let Some(sub) = world.resource::<BrushSelection>().sub(entity) else {
        return Vec::new();
    };
    match kind {
        ElementKind::Face => sub.faces.clone(),
        ElementKind::Vertex => sub.vertices.clone(),
        ElementKind::Edge => {
            let Some(brush) = world.get::<Brush>(entity) else {
                return Vec::new();
            };
            let index_of = address::edge_indices(&brush.topology);
            sub.edges
                .iter()
                .filter_map(|pair| index_of.get(pair).copied())
                .collect()
        }
    }
}

fn plural(kind: ElementKind) -> &'static str {
    match kind {
        ElementKind::Face => "faces",
        ElementKind::Edge => "edges",
        ElementKind::Vertex => "vertices",
    }
}

fn listed(indices: &[usize]) -> String {
    let shown: Vec<String> = indices.iter().take(LISTED).map(usize::to_string).collect();
    if indices.len() > LISTED {
        format!("[{}, ...]", shown.join(", "))
    } else {
        format!("[{}]", shown.join(", "))
    }
}

/// The element kind of the edit mode in use, else faces.
fn current_kind(world: &World) -> ElementKind {
    match world.get_resource::<EditMode>().copied() {
        Some(EditMode::BrushEdit(BrushEditMode::Edge)) => ElementKind::Edge,
        Some(EditMode::BrushEdit(BrushEditMode::Vertex)) => ElementKind::Vertex,
        _ => ElementKind::Face,
    }
}

pub(crate) fn parse_kind(text: &str) -> Result<ElementKind, String> {
    match text.trim().to_ascii_lowercase().as_str() {
        "face" | "faces" => Ok(ElementKind::Face),
        "edge" | "edges" => Ok(ElementKind::Edge),
        "vertex" | "vertices" | "vert" | "verts" => Ok(ElementKind::Vertex),
        other => Err(format!(
            "`kind` must be face, edge or vertex, not {other:?}"
        )),
    }
}

fn parse_mode(text: &str) -> Result<Combine, String> {
    match text.trim().to_ascii_lowercase().as_str() {
        "set" => Ok(Combine::Set),
        "add" => Ok(Combine::Add),
        "remove" => Ok(Combine::Remove),
        other => Err(format!("`mode` must be set, add or remove, not {other:?}")),
    }
}

/// Indices given as a JSON list, a single number, or comma-separated text.
fn parse_indices(given: Option<&jackdaw_scene_types::PropertyValue>) -> Result<Vec<usize>, String> {
    use jackdaw_scene_types::PropertyValue;
    let bad = || "`indices` must be whole numbers of zero or more, e.g. [0, 3]".to_string();
    let text = match given {
        Some(PropertyValue::Int(index)) => {
            return usize::try_from(*index)
                .map(|index| vec![index])
                .map_err(|_| bad());
        }
        Some(PropertyValue::String(text)) => text.trim().to_string(),
        _ => return Err(bad()),
    };
    if let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&text) {
        return items
            .iter()
            .map(|item| item.as_u64().map(|index| index as usize).ok_or_else(bad))
            .collect();
    }
    text.trim_matches(|c| c == '[' || c == ']')
        .split(',')
        .filter(|part| !part.trim().is_empty())
        .map(|part| part.trim().parse::<usize>().map_err(|_| bad()))
        .collect()
}

/// A `where` object as a [`Selector`].
pub(crate) fn parse_where(text: &str) -> Result<Selector, String> {
    let value: Value = serde_json::from_str(text.trim()).map_err(|_| {
        format!("`where` must be an object of rules, e.g. {{\"normal\": \"+y\"}}; got {text}")
    })?;
    let Value::Object(rules) = value else {
        return Err(format!(
            "`where` must be an object of rules, e.g. {{\"normal\": \"+y\"}}; got {text}"
        ));
    };
    if let Some(unknown) = rules.keys().find(|key| !WHERE_KEYS.contains(&key.as_str())) {
        return Err(format!(
            "`where` has no rule `{unknown}`; it takes {}",
            WHERE_KEYS.join(", ")
        ));
    }

    let tolerance_deg = match rules.get("tolerance_deg") {
        None => DEFAULT_TOLERANCE_DEG,
        Some(value) => value
            .as_f64()
            .map(|value| value as f32)
            .filter(|value| (0.0..=180.0).contains(value))
            .ok_or("`tolerance_deg` must be a number of degrees from 0 to 180")?,
    };
    let epsilon = match rules.get("epsilon") {
        None => DEFAULT_EPSILON,
        Some(value) => value
            .as_f64()
            .map(|value| value as f32)
            .filter(|value| value.is_finite() && *value >= 0.0)
            .ok_or("`epsilon` must be a distance of zero or more")?,
    };
    let cone = |key: &str| -> Result<Option<Cone>, String> {
        rules
            .get(key)
            .map(|value| {
                parse_direction(value)
                    .map(|direction| Cone {
                        direction,
                        tolerance_deg,
                    })
                    .ok_or_else(|| {
                        format!("`{key}` must be an axis like \"+y\" or a vector [x, y, z]")
                    })
            })
            .transpose()
    };

    let mut selector = Selector {
        normal: cone("normal")?,
        direction: cone("direction")?,
        ..Default::default()
    };
    if let Some(value) = rules.get("extreme") {
        let (axis, max) = value.as_str().and_then(parse_extreme).ok_or(
            "`extreme` must name an end of an axis: max_x, min_x, max_y, min_y, max_z or min_z",
        )?;
        selector.extreme = Some(Extreme { axis, max, epsilon });
    }
    if let Some(value) = rules.get("inside_box") {
        selector.inside_box = Some(parse_box(value)?);
    }
    if let Some(value) = rules.get("loop_of") {
        selector.loop_of = Some(
            value
                .as_u64()
                .map(|edge| edge as usize)
                .ok_or("`loop_of` must be an edge index")?,
        );
    }
    if let Some(value) = rules.get("all") {
        selector.all = value.as_bool().ok_or("`all` must be true or false")?;
    }
    Ok(selector)
}

/// `"+y"`, `"-x"`, `"z"`, or `[x, y, z]`.
fn parse_direction(value: &Value) -> Option<Vec3> {
    match value {
        Value::String(text) => {
            let text = text.trim().to_ascii_lowercase();
            let (sign, axis) = match text.as_bytes() {
                [b'-', axis] => (-1.0, *axis),
                [b'+', axis] | [axis] => (1.0, *axis),
                _ => return None,
            };
            let unit = match axis {
                b'x' => Vec3::X,
                b'y' => Vec3::Y,
                b'z' => Vec3::Z,
                _ => return None,
            };
            Some(unit * sign)
        }
        Value::Array(_) => vec3(value),
        _ => None,
    }
}

/// `"max_y"` style, also `"+y"` for max and `"-y"` for min.
fn parse_extreme(text: &str) -> Option<(usize, bool)> {
    let text = text.trim().to_ascii_lowercase();
    let (max, axis) = if let Some(axis) = text.strip_prefix("max_") {
        (true, axis)
    } else if let Some(axis) = text.strip_prefix("min_") {
        (false, axis)
    } else if let Some(axis) = text.strip_prefix('+') {
        (true, axis)
    } else {
        (false, text.strip_prefix('-')?)
    };
    let axis = match axis {
        "x" => 0,
        "y" => 1,
        "z" => 2,
        _ => return None,
    };
    Some((axis, max))
}

fn parse_box(value: &Value) -> Result<InsideBox, String> {
    let bad = "`inside_box` must be {\"min\": [x, y, z], \"max\": [x, y, z], \"space\": \
               \"local\" or \"world\"} with min no greater than max";
    let Value::Object(fields) = value else {
        return Err(bad.into());
    };
    let min = fields.get("min").and_then(vec3).ok_or(bad)?;
    let max = fields.get("max").and_then(vec3).ok_or(bad)?;
    if min.cmpgt(max).any() {
        return Err(bad.into());
    }
    let world = match fields.get("space").map(|space| space.as_str()) {
        None | Some(Some("local")) => false,
        Some(Some("world")) => true,
        Some(_) => return Err(bad.into()),
    };
    Ok(InsideBox { min, max, world })
}

fn vec3(value: &Value) -> Option<Vec3> {
    let items = value.as_array()?;
    let parts: Vec<f32> = items
        .iter()
        .map(|item| item.as_f64().map(|f| f as f32))
        .collect::<Option<_>>()?;
    match parts[..] {
        [x, y, z] if x.is_finite() && y.is_finite() && z.is_finite() => Some(Vec3::new(x, y, z)),
        _ => None,
    }
}

/// Whether a brush is in vertex, edge or face mode, when the Select menu shows.
pub fn editing_elements(world: &World) -> bool {
    world
        .get_resource::<EditMode>()
        .is_some_and(|mode| is_element_mode(*mode))
}

fn is_element_mode(mode: EditMode) -> bool {
    matches!(
        mode,
        EditMode::BrushEdit(BrushEditMode::Face | BrushEditMode::Edge | BrushEditMode::Vertex)
    )
}

/// Rebuild the menu bar when element editing starts or stops, so the Select
/// menu comes and goes with it.
pub(crate) fn flag_menu_on_element_editing(
    edit_mode: Res<EditMode>,
    mut was_editing: Local<bool>,
    dirty: Option<ResMut<jackdaw_editor_core::MenuBarDirty>>,
) {
    if !edit_mode.is_changed() {
        return;
    }
    let editing = is_element_mode(*edit_mode);
    if editing != *was_editing {
        *was_editing = editing;
        if let Some(mut dirty) = dirty {
            dirty.0 = true;
        }
    }
}

/// The Select menu while editing faces, edges or vertices.
pub fn select_menu(world: &World) -> Option<(String, Vec<(String, String)>)> {
    editing_elements(world).then(|| ("Select".to_string(), select_menu_rows()))
}

/// The edit-mode Select menu: select by rule in the edit mode in use, then
/// grow, shrink and walk the selection.
pub fn select_menu_rows() -> Vec<(String, String)> {
    use super::{select_invert, select_less, select_linked, select_loop, select_more, select_ring};
    use jackdaw_feathers::menu_bar::{SEPARATOR_ACTION, submenu_row};

    let rule = |rule: &str, value: &str| {
        format!(
            "op:{}?where={{\"{rule}\":{value}}}",
            BrushSelectElementsOp::ID
        )
    };
    let row = |action: String, label: &str| (action, label.to_string());
    let op = |id: &str, label: &str| (format!("op:{id}"), label.to_string());
    let separator = || (SEPARATOR_ACTION.to_string(), String::new());

    let axes = ["x", "y", "z"];
    let normals = axes.iter().flat_map(|axis| {
        let upper = axis.to_ascii_uppercase();
        [
            row(
                rule("normal", &format!("\"+{axis}\"")),
                &format!("Facing +{upper}"),
            ),
            row(
                rule("normal", &format!("\"-{axis}\"")),
                &format!("Facing -{upper}"),
            ),
        ]
    });
    let extremes = axes.iter().flat_map(|axis| {
        let upper = axis.to_ascii_uppercase();
        [
            row(
                rule("extreme", &format!("\"max_{axis}\"")),
                &format!("Highest {upper}"),
            ),
            row(
                rule("extreme", &format!("\"min_{axis}\"")),
                &format!("Lowest {upper}"),
            ),
        ]
    });
    [
        vec![row(rule("all", "true"), "All")],
        submenu_row("By Normal", normals),
        submenu_row("Extreme", extremes),
        vec![
            separator(),
            op(select_more::BrushSelectMoreOp::ID, "More"),
            op(select_less::BrushSelectLessOp::ID, "Less"),
            op(select_loop::BrushSelectLoopOp::ID, "Loop"),
            op(select_ring::BrushSelectRingOp::ID, "Ring"),
            op(select_linked::BrushSelectLinkedOp::ID, "Linked"),
            op(select_invert::BrushSelectInvertOp::ID, "Invert"),
        ],
    ]
    .concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn where_reads_every_rule() {
        let selector = parse_where(
            r#"{"normal": "+y", "tolerance_deg": 10, "extreme": "max_x", "epsilon": 0.1,
                "inside_box": {"min": [-1, 0, -1], "max": [1, 1, 1], "space": "world"},
                "loop_of": 3, "all": true, "direction": [0, 0, 2]}"#,
        )
        .expect("parses");
        assert_eq!(
            selector.normal,
            Some(Cone {
                direction: Vec3::Y,
                tolerance_deg: 10.0
            })
        );
        assert_eq!(
            selector.direction.map(|cone| cone.direction),
            Some(Vec3::Z * 2.0)
        );
        assert_eq!(
            selector.extreme,
            Some(Extreme {
                axis: 0,
                max: true,
                epsilon: 0.1
            })
        );
        assert!(selector.inside_box.is_some_and(|region| region.world));
        assert_eq!(selector.loop_of, Some(3));
        assert!(selector.all);
    }

    #[test]
    fn where_refuses_what_it_cannot_read() {
        for bad in [
            "normal",
            r#"{"facing": "+y"}"#,
            r#"{"normal": "up"}"#,
            r#"{"extreme": "top"}"#,
            r#"{"normal": "+y", "tolerance_deg": 200}"#,
            r#"{"inside_box": {"min": [1, 1, 1], "max": [0, 0, 0]}}"#,
            r#"{"loop_of": -1}"#,
        ] {
            assert!(parse_where(bad).is_err(), "{bad} was read");
        }
    }

    #[test]
    fn every_select_menu_rule_reads_back() {
        let rows = select_menu_rows();
        let rules: Vec<&str> = rows
            .iter()
            .filter_map(|(action, _)| action.split_once("?where=").map(|(_, rule)| rule))
            .collect();
        assert_eq!(rules.len(), 13);
        for rule in rules {
            parse_where(rule).unwrap_or_else(|err| panic!("{rule}: {err}"));
        }
    }

    #[test]
    fn indices_read_as_a_list_text_or_one_number() {
        use jackdaw_scene_types::PropertyValue;
        let list = PropertyValue::String("[4, 1]".into());
        assert_eq!(parse_indices(Some(&list)), Ok(vec![4, 1]));
        let text = PropertyValue::String("0, 2,5".into());
        assert_eq!(parse_indices(Some(&text)), Ok(vec![0, 2, 5]));
        assert_eq!(parse_indices(Some(&PropertyValue::Int(3))), Ok(vec![3]));
        assert!(parse_indices(Some(&PropertyValue::Int(-1))).is_err());
        assert!(parse_indices(Some(&PropertyValue::String("[a]".into()))).is_err());
    }
}
