# Writing editor tooling

An extension that adds a tool to the editor, rather than a window, needs to
know what is selected, where the cursor points in the scene, and how to
change the scene so the change can be undone. `jackdaw_api` exposes those as
ordinary Bevy resources, components, system sets and operators. This chapter
walks through them; the Physics module in `integrations/physics` uses every
one and is a complete example.

## What an extension can reach

| Need | Where |
| --- | --- |
| Whether a project is open | `AppState` |
| What is selected | `selection::{Selection, Selected}` |
| The viewport camera and the ray under the cursor | `viewport::ViewportCursor` |
| Entities the editor spawned for itself | `EditorEntity` |
| A modal tool, its toolbar button and its status line | `tools` |
| When brushes move and rebuild | `brush::BrushSystems` |
| Components new brushes start with | `register_brush_default` |
| Toggles in an inspector category's header | `register_inspector_chip` |
| A row in the viewport's overlay menu | `register_viewport_overlay` |
| What the open scene authored | `document::SceneDocument` |
| Several changes as one undo step | `op::with_history_span` |

Everything registered through `ExtensionRegistrar` is a child entity of the
extension, so disabling the extension removes its toolbar buttons, chips,
overlay rows and brush defaults with it.

## Selection

`Selection` lists the selected entities, the last being the primary one, and
each selected entity carries `Selected`. Read either, or observe
`On<Add, Selected>` and `On<Remove, Selected>`. Change the selection through
the `selection.select`, `selection.extend` and `selection.clear` operators.

## Changing the scene

The editor keeps a scene document beside the live world; that document is
what gets saved and what undo restores. An extension that writes a component
straight onto an entity changes the live world only, and the change is gone
on the next load. Go through the editor's operators instead:

- `component.add` and `component.remove` take an `entity` and a `type_path`;
- `component.set` sets a field, or with an empty `field` the whole component,
  from JSON;
- `entity.set_transform` moves an entity.

Each pushes its own undo entry. To make several calls one step, wrap them:

```rust
use jackdaw_api::op::with_history_span;

with_history_span(world, "Enable Physics", |world| {
    world
        .operator("component.add")
        .param("entity", entity)
        .param("type_path", AvianCollider::type_path().to_string())
        .call()?;
    // ...
});
```

Do not call operators from inside another operator's system. Record the
request in a resource and apply it from an ordinary system on the next
frame, as `physics.enable` does.

`SceneDocument::authored_components(world, entity)` lists the component type
paths the scene authored on an entity, as opposed to the companions
`#[require]` and plugins add at runtime. Use it to decide what to remove.

## A modal tool

A tool is a modal operator: its system runs every frame while it returns
`OperatorResult::Running`. While any modal runs, the editor's gizmo, numeric
entry and click selection stand down.

```rust
#[operator(id = "example.measure", label = "Measure", modal = true, allows_undo = false)]
fn measure(
    _: In<OperatorParameters>,
    active: ActiveModalQuery,
    keys: ModalInputs,
    cursor: ViewportCursor,
    mut status: ResMut<ToolStatus>,
) -> OperatorResult {
    if !active.is_modal_running() {
        status.0 = Some("Measure | Esc to stop".into());
    }
    if keys.cancel() {
        return OperatorResult::Cancelled;
    }
    if let Some(ray) = cursor.ray() {
        // ...
    }
    OperatorResult::Running
}
```

Register it with a toolbar button and a key:

```rust
ctx.register_operator::<MeasureOp>()
    .register_toolbar_button::<MeasureOp>(Icon::Ruler)
    .bind_operator_host::<MeasureOp>([PresetInput::key("KeyM")]);
```

The button lights while the tool runs. `ModalInputs` reports the editor's
confirm (Enter) and cancel (Esc) keys. The editor clears `ToolStatus` once
no modal runs. A tool that captured a viewport when a drag began can keep
following it with `ViewportCursor::ray_for(camera, viewport, cursor)`.

When picking with `MeshRayCast`, filter out `EditorEntity`: material
previews and gizmo meshes sit at the origin and would hide the scene.

## Brushes

Brushes are the `Brush` component in `jackdaw_scene_types`, evaluated with
`evaluate_brush_geometry`. The editor moves a brush's origin to the centre of
its geometry in `BrushSystems::Recenter` and rebuilds its meshes in
`BrushSystems::Rebuild`; order systems that follow a brush's shape after
them. `register_brush_default` gives every brush the editor creates from then
on a component, written to the document as if the person had added it.

## Inspector and viewport rows

`register_inspector_chip(category, label, type_path)` puts a toggle in a
category's header that adds or removes the component through the editor's
operators. A brush shows the category even before it carries one of the
components.

`register_viewport_overlay::<Op>(label, shown)` adds a row to the viewport
overlay menu. Clicking it calls `Op`, which should flip its overlay and write
the new state to the row's `ViewportOverlay::shown`.

## Comparison with other editors

Unity users will recognise `Selection` and `Selection.selectionChanged`,
`EditorTool` with its `ToolManager`, `Undo.CollapseUndoOperations` and
`HandleUtility.GUIPointToWorldRay`. Godot's `EditorPlugin` covers the same
ground with `get_selection`, `_forward_3d_gui_input`,
`add_control_to_container`, inspector plugins and `EditorUndoRedoManager`.
Unreal's editor modes, toolbar extenders and detail customisations are the
nearest match there. Jackdaw differs in using plain ECS data for all of it:
registrations are entities, state is resources, and every change goes
through an operator.
