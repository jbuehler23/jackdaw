# Remote control and jd mcp

Jackdaw's rule is that everything the editor does is an operator: every menu
item, panel button, terrain tool, bake and save dispatches one. That makes the
whole editor scriptable from a single call, and `jd mcp` is what puts that call
in front of an MCP client.

The editor serves the Bevy Remote Protocol on loopback while a project is open.
`jd mcp` runs `jd-mcp`, a Model Context Protocol server that speaks MCP over
stdio to the client and BRP to the editor. It holds no state of its own: every
tool is one BRP method, and every edit to the open document is undoable -- the
operators through their own history, `apply_bsn` through the command the editor
pushes for it -- so a remote edit lands on the same undo stack the user's clicks
do. Undo does not reach the disk: an operator that saves, exports or bakes is as
reachable here as it is from the menus.

## Setting it up

Open a project in the editor, then register `jd mcp` with your MCP client as a
stdio server. Most clients take a JSON entry of this shape:

```json
{
  "mcpServers": {
    "jackdaw": { "command": "jd", "args": ["mcp", "--project", "/path/to/my-game"] }
  }
}
```

`--project` names the project root. Without it, `jd mcp` uses the working
directory. It finds the editor through `<project>/.jackdaw/editor.json`, which
the editor writes when it opens the project and removes when it exits, so
nothing has to be configured with a port. A client that starts before the editor
reports that no editor is running rather than failing to connect.

The port defaults to 15703, one past the game's 15702, so an editor and the game
it launches never contend for the socket. An editor that finds the port already
taken says so and serves nothing, rather than publishing an endpoint that points
at the editor already there; give the second one its own port with
`JACKDAW_REMOTE_PORT`. Remote control is on by default, and a project turns it
off with `{"remote": {"enabled": false}}` under the `remote` key of
`.jackdaw/settings.json`.

Running the editor from a source checkout needs its libraries on the path, from
the checkout root:

```bash
LD_LIBRARY_PATH=target/debug:target/debug/deps:$(rustc --print target-libdir)
```

`rustc --print sysroot` is not enough on its own: `<sysroot>/lib` holds no
`libstd`, which lives a level down in the target lib dir that command prints.

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

Two read-only resources sit beside them: `jackdaw://operators` is the operator
catalogue, and `jackdaw://scene` is the open document as BSN.

`save_scene` and `screenshot` are the only tools that write to disk, though
`call_operator` reaches the operators that save, export and bake. The server only
ever talks to loopback, and both a screenshot path and a scene path have to name
a file inside the project: the editor runs as the user, and an unconfined path
would let a page or a prompt aim a write anywhere they can write.

## Working with it

Start from `list_operators`. It is the whole vocabulary, and each parameter
carries the same documentation the editor's own tooltips show, so a caller can
discover a tool rather than be told about it:

```text
list_operators(prefix: "terrain.")
call_operator(id: "terrain.sculpt.stamp",
              params: { terrain: "Ground", x: 12, z: -8, radius: 6, strength: 2 })
```

Parameters are typed from the operator's declared schema rather than from the
JSON's own spelling, so `radius: "6"` reaches a float parameter and `name: 7`
reaches a string one. An `Entity` parameter takes a name, and the operators that
act on the selection take it from there when nothing is named -- the same
resolution `JACKDAW_RUN_OP` clauses get.

Group a run of calls that mean one action with `batch`. Inside one span they are
one undo entry, so one Ctrl-Z takes the whole action back rather than a
fraction of it:

```text
batch(label: "Fence the north plot", calls: [
  { id: "entity.add.group", params: { name: "Fence_North" } },
  { id: "entity.place_gltf", params: { path: "kit/Prop_Fence_01.gltf", pos_x: 0, pos_y: 0, pos_z: 0 } },
])
```

A call answers with the entities it added, under `entities`, so the next call
can name what the last one made rather than guessing which node in the tree is
new. Every call in a batch reports its own, and `apply_bsn` reports what its
text spawned:

```text
call_operator(id: "entity.add.cube")            -> { entities: [4294967301], ... }
call_operator(id: "entity.set_transform",
              params: { entity: 4294967301, x: 4, y: 0, z: -2 })
```

Reads are cheap and worth doing often. `scene_tree` says what is there,
`get_entity` says what one node holds, and `screenshot` shows the result --
it waits for the capture to reach the disk, so the image it returns is the
frame it captured. `scene_tree` takes a `root` as an entity id or as a name,
and a `depth` counting generations below it: `0` is the node alone, `1` adds
its children, and no `depth` reports the whole subtree.

```text
scene_tree(root: "Terrain", depth: 1)
```

After anything that takes time (opening a scene, a navmesh
bake, a project build), `wait(until_idle: true)` holds until nothing is running,
including the models an opened scene is still pulling off disk.

A screenshot of a 3D scene is only worth taking once the camera is pointed at
it. `view.frame_all` and `view.frame_selected` keep whatever orientation the
camera already has, so a camera left level with the ground frames a terrain
edge-on and the picture is empty. Aim it first:

```text
call_operator(id: "view.look_at",
              params: { eye_x: 120, eye_y: 90, eye_z: 120,
                        target_x: 0, target_y: 0, target_z: 0 })
call_operator(id: "view.orbit", params: { yaw: 135, pitch: 40, distance: 200 })
```

`view.look_at` takes an eye and a target in world metres and switches the
viewport to perspective. `view.orbit` turns around the focus point the last
`look_at` (or orbit) set, taking a compass `yaw` and a `pitch` above the ground
in degrees plus a `distance` in metres. `view.dolly` moves along the sightline.
Between them they cover the orbit, pan and dolly the pointer does, none of which
is an operator a caller could dispatch.

`select` frames what it selected when asked, and `screenshot` aims before it
captures, so each is one call rather than three:

```text
select(names: ["Village"], frame: true)
screenshot(look_at: { eye: [120, 90, 120], target: [0, 0, 0] })
```

## Playing the game from a client

`pie.play` builds the project's game binary and launches it. In the default
embedded mode the game streams into the editor's Game panel, so a window
capture shows the running game rather than a separate OS window.

The launch is a cargo build and then a process, which is minutes rather than
frames, so `status` reports where it has got to under `pie` -- `building`,
`running` or `stopped` -- and `wait` holds for either end of it:

```text
call_operator(id: "pie.play")
wait(until: "pie_running")
screenshot(kind: "window")
call_operator(id: "pie.stop")
wait(until: "pie_stopped")
```

A paused game counts as running: its process is up and its frames are still
there to capture.

## Packing repeated groups

`prefab.pack` writes a group out as a prefab file and leaves an instance of
that file standing where the group stood. `prefab.pack_matching` does that
once and then replaces every other top-level group that matches with an
instance of the same file, each keeping the placement its group had:

```text
call_operator(id: "prefab.pack_matching",
              params: { entity: 4294967301, path: "prefabs/steading.bsn" })
```

`match` is `structural` by default, which compares whole subtrees -- two
groups that agree on their direct children and differ below are not copies
-- or `prefix`, which compares names against `prefix`. `path` is relative
to the project's assets directory and cannot leave it, and an existing file
is not replaced without `overwrite: true`.

How many groups became instances comes back in the call's `reports`, which
is where an operator says what it did when the number is the answer.

## When a call did not do what you asked

An operator reports a parameter it could not use in the `warnings` of the call
result, not only in the editor's log. `input.pointer` is the one to watch:
`button` is `primary`, `secondary` or `middle` (with `left` and `right` as
aliases), and `space` is `window` or `canvas`. Anything else is refused with a
warning rather than quietly treated as the default, so a `space: "viewport"`
comes back as text saying so instead of as a gesture aimed at the wrong place.
Warnings belong to the call that produced them and do not carry over.

## Operators that need a pointer

A few operators are modal: they hold a gesture open across frames and end when
the mouse button comes up. A caller with no pointer cannot drive one, so each has
a parametric operator that does the same job -- `terrain.sculpt.stamp` for the
sculpt brush, `entity.set_transform` for a gizmo drag, `selection.select` for a
rubber band. The pairs are listed in `tests/operators/remote_coverage.rs`, and a
new modal operator fails that test until someone has said what a remote caller should
call instead.

Calling one anyway returns `running` and leaves that operator holding the
editor, which refuses every later modal call. `status` reports it under `modal`,
`cancel` ends it, and `batch` cancels one it started rather than walking away
from a wedged editor. `wait(until_idle: true)` deliberately does not wait on a
modal -- nothing is coming to finish it -- so it answers and names it instead.
