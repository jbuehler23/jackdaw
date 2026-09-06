//! Every operator stays reachable without a pointer. `jackdaw/call_operator` is
//! the whole remote vocabulary, and two things quietly break that: a parameter
//! type the remote cannot spell from JSON, and a modal operator that only a
//! pointer can drive.

use jackdaw::remote::server::property_from_json;
use jackdaw_api_internal::lifecycle::OperatorEntity;
use jackdaw_api_internal::operator::ParamSpec;
use serde_json::json;

use crate::util;

/// Modal operators, and the parametric operator a remote caller uses instead.
/// Each is listed with the operator that does the same job from parameters, or
/// with `None` and the reason no such operator exists yet; adding a modal
/// operator without a row here fails `every_modal_operator_has_a_parametric_answer`.
const POINTER_ONLY: &[(&str, Option<&str>, &str)] = &[
    (
        "brush.box_select",
        None,
        "a rubber band over brush vertices, edges and faces; sub-element selection has no \
         parametric form yet",
    ),
    (
        "selection.box_select",
        Some("selection.select"),
        "a rubber band over the viewport; select entities by name instead",
    ),
    (
        "gizmo.drag",
        Some("entity.set_transform"),
        "dragging a gizmo handle; by value instead",
    ),
    (
        "gizmo.drag_edit",
        Some("entity.set_transform"),
        "dragging a gizmo handle; by value instead",
    ),
    (
        "mirror.plane.drag",
        None,
        "placing the mirror plane by dragging it; no parametric form yet",
    ),
    (
        "hierarchy.rename_begin",
        Some("component.set"),
        "an inline text field; set the Name component instead",
    ),
    (
        "terrain.sculpt",
        Some("terrain.sculpt.stamp"),
        "a held brush stroke; one stamp at a named place instead",
    ),
    (
        "terrain.paint",
        Some("terrain.paint.stamp"),
        "a held brush stroke; one stamp at a named place instead, or \
         terrain.tint.stamp for the colour domain of the same tool",
    ),
    (
        "physics.activate",
        Some("physics.enable"),
        "grab-and-throw in the viewport; enable the body instead",
    ),
    (
        "viewport.draw_brush_modal",
        Some("mesh.add_brush"),
        "drawing a polygon with the cursor; add the brush instead",
    ),
    (
        "brush.vertex.drag",
        Some("brush.nudge_up"),
        "dragging one vertex; nudge by a step instead",
    ),
    (
        "brush.edge.drag",
        Some("brush.nudge_up"),
        "dragging one edge; nudge by a step instead",
    ),
    (
        "brush.face.drag",
        Some("brush.nudge_up"),
        "dragging one face; nudge by a step instead",
    ),
    (
        "tools.measure_distance",
        None,
        "two clicks in the viewport; reading a distance is not authoring",
    ),
    (
        "brush.mesh.extrude",
        None,
        "a drag distance; no parametric form yet",
    ),
    (
        "brush.mesh.inset",
        None,
        "a drag distance; no parametric form yet",
    ),
    (
        "brush.mesh.loop_cut",
        None,
        "a hovered edge ring plus a drag; no parametric form yet",
    ),
    (
        "brush.mesh.edge_bevel",
        None,
        "a drag width; no parametric form yet",
    ),
    (
        "brush.mesh.vertex_bevel",
        None,
        "a drag width; no parametric form yet",
    ),
    (
        "brush.mesh.edge_slide_modal",
        None,
        "a drag along the edge; no parametric form yet",
    ),
    (
        "brush.mesh.vertex_slide_modal",
        None,
        "a drag along the edge; no parametric form yet",
    ),
];

/// Camera gestures that are not operators at all, and the operator a remote
/// caller uses instead. Orbit, pan and dolly are raw input on the viewport's
/// camera controller, so `POINTER_ONLY` never sees them, but they still have to
/// be reachable: `view.frame_all` keeps whatever orientation it finds.
const POINTER_CAMERA_GESTURES: &[(&str, &str, &str)] = &[
    (
        "orbit",
        "view.orbit",
        "dragging with the secondary button; an angle pair and a radius instead",
    ),
    (
        "pan",
        "view.look_at",
        "dragging with the middle button; an eye and a target instead",
    ),
    (
        "dolly",
        "view.dolly",
        "the scroll wheel; a distance along the sightline instead",
    ),
];

/// Every camera gesture a remote caller cannot make has an operator that does
/// the same job, and that operator is itself parametric.
#[test]
fn every_pointer_camera_gesture_has_a_parametric_operator() {
    let mut app = util::editor_test_app();
    let all = registered(&mut app);

    for (gesture, id, why) in POINTER_CAMERA_GESTURES {
        assert!(!why.is_empty(), "{gesture} is listed with no reason");
        let op = all
            .iter()
            .find(|op| op.id() == *id)
            .unwrap_or_else(|| panic!("{gesture}'s operator `{id}` is not registered"));
        assert!(
            !op.is_modal(),
            "{gesture}'s operator `{id}` is modal, so it needs a pointer too"
        );
        assert!(
            !op.parameters().is_empty(),
            "{gesture}'s operator `{id}` takes no parameters, so it cannot stand in for a drag"
        );
    }
}

/// A value of each declared parameter type, as a client would send it. A new
/// `ParamSpec::ty` with no entry fails
/// `every_parameter_type_can_be_written_as_text`.
fn text_for(ty: &str) -> Option<serde_json::Value> {
    Some(match ty {
        "Bool" => json!("true"),
        "Int" => json!("7"),
        "Float" => json!("1.5"),
        // Entity parameters are filled in from a name or the selection
        // before dispatch (`boot_ops::resolve_entity_params`).
        "String" | "Entity" => json!("Cube"),
        "Vec2" => json!("1,2"),
        "Vec3" => json!("1,2,3"),
        "Color" => json!("1,1,1,1"),
        _ => return None,
    })
}

/// Every registered operator, without the duplicate registrations two
/// extensions can produce for one id.
fn registered(app: &mut bevy::prelude::App) -> Vec<OperatorEntity> {
    let mut state = app.world_mut().query::<&OperatorEntity>();
    let mut all: Vec<OperatorEntity> = state.iter(app.world()).cloned().collect();
    all.sort_by_key(OperatorEntity::id);
    all.dedup_by_key(|op| op.id());
    all
}

/// A pointer-driven operator is either answered by a parametric twin or declared
/// out of a remote caller's reach.
#[test]
fn every_modal_operator_has_a_parametric_answer() {
    let mut app = util::editor_test_app();
    let all = registered(&mut app);
    let ids: Vec<&'static str> = all.iter().map(OperatorEntity::id).collect();

    let mut undeclared = Vec::new();
    for op in &all {
        if !op.is_modal() || op.remote_hidden().is_some() {
            continue;
        }
        match POINTER_ONLY.iter().find(|(id, _, _)| *id == op.id()) {
            Some((_, Some(twin), _)) => assert!(
                ids.contains(twin),
                "{}'s listed twin `{twin}` is not a registered operator",
                op.id()
            ),
            Some((_, None, _)) => {}
            None => undeclared.push(op.id()),
        }
    }
    assert!(
        undeclared.is_empty(),
        "these modal operators need a pointer and nothing says what a remote caller should call \
         instead. Add each to POINTER_ONLY in this file with its parametric twin (or `None` \
         and why there is none), or declare it `remote_hidden` if it is not authoring at all: \
         {undeclared:?}"
    );
}

/// Every listed twin is still a real operator and still parametric, so
/// the table cannot rot into advice that no longer works.
#[test]
fn the_pointer_only_table_names_operators_that_exist() {
    let mut app = util::editor_test_app();
    let all = registered(&mut app);

    for (id, twin, why) in POINTER_ONLY {
        assert!(
            !why.is_empty(),
            "{id} is listed with no reason; the next reader has to know why"
        );
        assert!(
            all.iter().any(|op| op.id() == *id),
            "{id} is in POINTER_ONLY but no longer registered; drop the row"
        );
        let Some(twin) = twin else { continue };
        let twin_op = all
            .iter()
            .find(|op| op.id() == *twin)
            .unwrap_or_else(|| panic!("{id}'s twin `{twin}` is not registered"));
        assert!(
            !twin_op.is_modal(),
            "{id}'s twin `{twin}` is itself modal, so it needs a pointer too"
        );
    }
}

/// Every parameter of every non-modal operator can be written as text. An
/// operator kept out of the remote vocabulary says why: `remote_hidden` is the
/// one way past this test, so a bare flag would be a silencer.
#[test]
fn a_hidden_operator_gives_a_reason() {
    let mut app = util::editor_test_app();
    let hidden: Vec<(&str, &str)> = registered(&mut app)
        .iter()
        .filter_map(|op| op.remote_hidden().map(|why| (op.id(), why)))
        .collect();

    assert!(
        !hidden.is_empty(),
        "nothing is declared remote_hidden, so this test proves nothing"
    );
    for (id, why) in hidden {
        assert!(
            why.split_whitespace().count() >= 3,
            "{id} is hidden with the reason {why:?}, which explains nothing"
        );
    }
}

#[test]
fn every_parameter_type_can_be_written_as_text() {
    let mut app = util::editor_test_app();
    let mut unspellable: Vec<(&str, &str, &str)> = Vec::new();

    for op in registered(&mut app) {
        for spec in op.parameters() {
            let Some(value) = text_for(spec.ty) else {
                unspellable.push((op.id(), spec.name, spec.ty));
                continue;
            };
            let typed = property_from_json(Some(spec), &value);
            assert!(
                typed.is_some(),
                "{}: `{}` is a {} and the remote cannot type {value} as one",
                op.id(),
                spec.name,
                spec.ty
            );
        }
    }

    assert!(
        unspellable.is_empty(),
        "these parameters have a type no text client can spell. Give the type a spelling in \
         `text_for` here and in `property_from_json` (src/remote/server.rs): {unspellable:?}"
    );
}

/// The operators added for callers with no pointer are registered and
/// declare the parameters the remote documents.
#[test]
fn the_parametric_authoring_operators_are_registered() {
    let mut app = util::editor_test_app();
    let all = registered(&mut app);

    for (id, wanted) in [
        ("entity.add.group", &["name", "parent"][..]),
        ("entity.set_transform", &["entity", "x", "yaw", "sx"][..]),
        (
            "component.set",
            &["entity", "type_path", "field", "value"][..],
        ),
        (
            "terrain.sculpt.stamp",
            &["terrain", "x", "z", "radius", "mode", "strength"][..],
        ),
        (
            "terrain.paint.stamp",
            &["terrain", "x", "z", "radius", "slot", "opacity"][..],
        ),
        ("selection.extend", &["name"][..]),
        ("prefab.pack", &["entity", "path", "overwrite"][..]),
        (
            "prefab.pack_matching",
            &["entity", "path", "match", "prefix", "overwrite"][..],
        ),
        ("terrain.scatter.adopt", &["entity", "terrain", "key"][..]),
        ("terrain.scatter.group.select", &["terrain", "key"][..]),
        (
            "view.look_at",
            &[
                "eye_x", "eye_y", "eye_z", "target_x", "target_y", "target_z",
            ][..],
        ),
        ("view.orbit", &["yaw", "pitch", "distance"][..]),
        ("view.set_axis", &["axis", "sign"][..]),
    ] {
        let op = all
            .iter()
            .find(|op| op.id() == id)
            .unwrap_or_else(|| panic!("{id} is not registered"));
        let declared: Vec<&str> = op
            .parameters()
            .iter()
            .map(|spec: &ParamSpec| spec.name)
            .collect();
        for name in wanted {
            assert!(
                declared.contains(name),
                "{id} does not declare `{name}`; it declares {declared:?}"
            );
        }
    }
}
