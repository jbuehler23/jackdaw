# Remote control and jd mcp

Every editor action is an operator, so the whole editor is scriptable through one
call. The editor serves the Bevy Remote Protocol on loopback while a project is
open. `jd mcp` runs `jd-mcp`, an MCP server that speaks MCP over stdio to the
client and BRP to the editor. Each tool is one BRP method, and every edit to the
open document lands on the editor's undo stack. Undo does not reach the disk:
operators that save, export or bake are reachable here too.

## Setting it up

Open a project in the editor, then register `jd mcp` with your MCP client as a
stdio server:

```json
{
  "mcpServers": {
    "jackdaw": { "command": "jd", "args": ["mcp", "--project", "/path/to/my-game"] }
  }
}
```

- `--project` names the project root; without it, the working directory is used.
- The editor is found through `<project>/.jackdaw/editor.json`, written on open
  and removed on exit, so no port needs configuring. A client that starts first
  reports that no editor is running.
- The port defaults to 15703, one past the game's 15702. An editor that finds it
  taken serves nothing; give a second editor its own port with
  `JACKDAW_REMOTE_PORT`.
- Remote control is on by default. Disable it per project with
  `{"remote": {"enabled": false}}` in `.jackdaw/settings.json`.

Running the editor from a source checkout needs its libraries on the path, from
the checkout root:

```bash
LD_LIBRARY_PATH=target/debug:target/debug/deps:$(rustc --print target-libdir)
```

`rustc --print sysroot` is not enough: `<sysroot>/lib` holds no `libstd`, which
lives in the target lib dir that command prints.

## The tools

| Tool | What it does |
| --- | --- |
| `status` | Project, open scene, dirty flag, selection, play state |
| `list_operators` | Every operator with its parameter schema, filtered by prefix |
| `call_operator` | Run one operator by id |
| `batch` | Run several calls as one undo entry |
| `scene_tree` | The scene as the outliner shows it, from a named root |
| `get_entity` | One node and its descendants as BSN text |
| `apply_bsn` | Spawn BSN text, optionally under a named parent |
| `scene_bsn` | The whole open document as BSN |
| `open_scene` | Open a scene by its assets-relative path |
| `save_scene` | Write the open scene to its file |
| `select` | Select entities by name, and frame them |
| `screenshot` | Aim the camera, capture the viewport or the window, return the PNG |
| `wait` | Let frames pass, or wait for a state the editor has reached |
| `cancel` | End the modal operator holding the editor |
| `assets` | Asset paths under `assets/`, by substring or `*` glob |

Two read-only resources: `jackdaw://operators` is the operator catalogue, and
`jackdaw://scene` is the open document as BSN.

`save_scene` and `screenshot` are the only tools that write to disk, though
`call_operator` reaches operators that save, export and bake. The server only
talks to loopback, and screenshot and scene paths must name a file inside the
project.

## Working with it

Start from `list_operators`. Each parameter carries the same documentation the
editor's tooltips show.

```text
list_operators(prefix: "terrain.")
call_operator(id: "terrain.sculpt.stamp",
              params: { terrain: "Ground", x: 12, z: -8, radius: 6, strength: 2 })
```

Parameters are coerced from the operator's declared schema rather than the JSON
spelling, so `radius: "6"` reaches a float parameter and `name: 7` a string one.
An `Entity` parameter takes a name; operators that act on the selection use it
when nothing is named.

Group calls that mean one action with `batch` -- inside one span they are a
single undo entry:

```text
batch(label: "Fence the north plot", calls: [
  { id: "entity.add.group", params: { name: "Fence_North" } },
  { id: "entity.place_gltf", params: { path: "kit/Prop_Fence_01.gltf", pos_x: 0, pos_y: 0, pos_z: 0 } },
])
```

A call answers with the entities it added under `entities`, so the next call can
name what the last one made. Every call in a batch reports its own, and
`apply_bsn` reports what its text spawned:

```text
call_operator(id: "entity.add.cube")            -> { entities: [4294967301], ... }
call_operator(id: "entity.set_transform",
              params: { entity: 4294967301, x: 4, y: 0, z: -2 })
```

`scene_tree` takes a `root` as an entity id or a name, and a `depth` counting
generations below it: `0` is the node alone, `1` adds its children, and no
`depth` reports the whole subtree.

```text
scene_tree(root: "Terrain", depth: 1)
```

After anything that takes time (opening a scene, a navmesh bake, a project
build), `wait(until_idle: true)` holds until nothing is running, including models
an opened scene is still loading.

Aim the camera before screenshotting a 3D scene. `view.frame_all` and
`view.frame_selected` keep the camera's current orientation, so a level camera
frames a terrain edge-on.

```text
call_operator(id: "view.look_at",
              params: { eye_x: 120, eye_y: 90, eye_z: 120,
                        target_x: 0, target_y: 0, target_z: 0 })
call_operator(id: "view.orbit", params: { yaw: 135, pitch: 40, distance: 200 })
```

- `view.look_at` takes an eye and a target in world metres and switches the
  viewport to perspective.
- `view.orbit` turns around the focus point the last `look_at` or orbit set,
  taking a compass `yaw`, a `pitch` above the ground in degrees, and a
  `distance` in metres.
- `view.dolly` moves along the sightline.

`select` frames what it selected when asked, and `screenshot` aims before it
captures:

```text
select(names: ["Village"], frame: true)
screenshot(look_at: { eye: [120, 90, 120], target: [0, 0, 0] })
```

## Playing the game from a client

`pie.play` builds the project's game binary and launches it. In the default
embedded mode the game streams into the editor's Game panel, so a window capture
shows the running game.

The launch is a cargo build then a process, so `status` reports progress under
`pie` -- `building`, `running` or `stopped` -- and `wait` holds for either end:

```text
call_operator(id: "pie.play")
wait(until: "pie_running")
screenshot(kind: "window")
call_operator(id: "pie.stop")
wait(until: "pie_stopped")
```

A paused game counts as running.

## Packing repeated groups

`prefab.pack` writes a group out as a prefab file and leaves an instance standing
where the group stood. `prefab.pack_matching` does that once, then replaces every
other matching top-level group with an instance of the same file, each keeping
its own placement:

```text
call_operator(id: "prefab.pack_matching",
              params: { entity: 4294967301, path: "prefabs/steading.bsn" })
```

`match` is `structural` by default, comparing whole subtrees; `prefix` compares
names against `prefix` instead. `path` is relative to the project's assets
directory and cannot leave it, and an existing file needs `overwrite: true`. The
number of groups turned into instances comes back in the call's `reports`.

## When a call did not do what you asked

An operator reports a parameter it could not use in the call result's
`warnings`. For `input.pointer`, `button` is `primary`, `secondary` or `middle`
(`left` and `right` are aliases) and `space` is `window` or `canvas`; anything
else is refused with a warning rather than treated as the default. Warnings
belong to the call that produced them.

## Operators that need a pointer

Modal operators hold a gesture open across frames and end when the mouse button
comes up. A caller with no pointer cannot drive one, so each has a parametric
equivalent -- `terrain.sculpt.stamp` for the sculpt brush,
`entity.set_transform` for a gizmo drag, `selection.select` for a rubber band.
The pairs are listed in `tests/operators/remote_coverage.rs`, and a new modal
operator fails that test until its remote equivalent is named.

Calling one anyway returns `running` and leaves that operator holding the editor,
which then refuses every later modal call. `status` reports it under `modal`,
`cancel` ends it, and `batch` cancels one it started. `wait(until_idle: true)`
does not wait on a modal -- it answers and names it instead.
