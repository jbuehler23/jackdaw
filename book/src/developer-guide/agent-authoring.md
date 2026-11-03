# Building scenes with an agent

An agent builds a scene the way a script does: it calls operators through
`jd-mcp`, the MCP server described in [Remote Control and jd
mcp](remote-control.md). What costs an agent is reading, so the loop below
keeps each answer small: build in batches, check with numbers, and look at
the scene only when how it looks is the question.

## Setting up Claude Code

Build the server from the Jackdaw checkout:

```bash
cargo build -p jackdaw_mcp --bin jd-mcp
```

Open your project in the editor, then register the server from the
directory you run Claude Code in:

```bash
claude mcp add jackdaw -s project -- /path/to/jackdaw/target/debug/jd-mcp --project /path/to/my-game
```

- `-s project` writes the entry to `.mcp.json` in that directory, so anyone
  who opens it there gets the server too.
- `--project` names the project root. The server finds the editor through
  `<project>/.jackdaw/editor.json`, so no port is configured.
- Run `/mcp` in Claude Code. `jackdaw` shows as connected and its tools
  appear as `mcp__jackdaw__*`. A tool that answers that no editor is running
  means the project is not open in the editor yet.

## The loop

### Learn an operator before calling it

`describe_operator` returns one operator's parameters with their
documentation. `list_operators` with a prefix such as `mesh.` or `brush.`
finds ids. Neither costs much; the `operators` resource holds the whole
catalogue and is never worth reading whole.

### Build in stages

Each part of a build is one `batch`, and each batch builds into a named
`stage`. Run a stage again and it replaces what it built last time rather
than adding a copy, so fixing a part means editing its calls and running
them again. A stage is atomic: a call that fails takes the whole stage back.

```text
batch(stage: "Inn", label: "Inn walls", calls: [
  { id: "entity.add.cube", params: { name: "Inn_Floor", size: [8, 0.2, 6] } },
  { id: "entity.add.cube", params: { name: "Inn_Wall_N", size: [8, 3, 0.25],
                                     pos: [0, 1.6, -2.875] } },
],
checks: [
  { rests_on: { entity: "$1.entities.0", on: "$0.entities.0" } },
])
```

- `$N.path` names a value from call N's result, such as `$0.entities.0` for
  the first entity call 0 added. It works inside lists too, so
  `entities: ["$0.entities.0", "$1.entities.0"]` names both.
- A batch outside a stage takes `atomic: true` for the same all-or-nothing
  behaviour, and `dry_run: true` to run it, report and take it all back.
- `after: ["Inn"]` makes a stage refuse to build until the stages it sits on
  exist.
- A long build goes in a JSON file under the project, run with
  `run_batch_file`. It answers with a summary: calls run, entities added,
  warnings and where it stopped.

### Verify with numbers

`checks` on a batch, or `check` on its own, measure the scene: `rests_on`,
`inside`, `no_overlap`, `bounds`, `count` and `watertight`, each with the
value it measured. A failing check fails the batch, and an atomic batch then
takes itself back.

`scene_summary` reads the scene as compact rows of id, name, parent, kind,
position and size. Pass its `revision` back as `since_revision` and the next
answer holds only the rows added, changed and removed since.

### Look only when it matters

`screenshot` returns a 512 px JPEG thumbnail, a few tens of kilobytes, and
keeps the full PNG on disk. `look_at` aims the camera for the shot and then
puts it back where the person at the editor left it; `keep_camera: true`
leaves it aimed. Ask for `format: "png"` when edges matter more than size.

```text
screenshot(look_at: { eye: [24, 14, 24], target: [0, 2, 0] })
```

### Save

`save_scene` writes the open scene to its file. A rollback cannot take a save
back, so a staged, atomic or dry-run batch refuses one; save between stages.

## Recording a build

`pace_ms` pauses that many milliseconds between a batch's calls, up to 2000,
so the build plays out on screen instead of appearing in one frame. It is
still one batch and one undo entry. A paced batch cannot be a `dry_run`, and
one plays at a time. While it plays, undo, redo and switching scenes or
projects are unavailable and other remote edits are refused. An edit made by
hand in the editor meanwhile still joins its undo entry. `wait` with `until: "idle"` holds until it ends.
