# Scene management

A "scene" in jackdaw is one `.jsn` file. A "project" is a
folder with a `.jsn/project.jsn` config file (legacy projects
keep it at the root), an `assets/` directory, and a
`Cargo.toml` if you scaffolded with the static template.
Scenes live under `assets/`.

## Save and load

- `Ctrl+S` saves the current scene to its on-disk path. The
  first save prompts for a path; pick something under
  `assets/`.
- `Ctrl+O` opens a scene from disk. The picker starts in the
  current project's `assets/` folder.
- `Ctrl+Shift+N` creates a new empty scene in memory; it is
  unsaved until you `Ctrl+S` it.

The format details are in [JSN Format](../developer-guide/jsn-format.md).
The short version: human-readable, line-diffable, and
designed to read in `git diff` without making you cry.

## Project select screen

The launcher (`AppState::ProjectSelect`) is the first thing
you see when you run `jackdaw` with no arguments. It shows:

- Recent projects, with timestamp and last-opened scene.
- A `+ New Project` button for the scaffolds.
- A `+ New Extension` and `+ New Game` button if you want to
  start an extension or game crate.

Recent projects with missing folders are filtered out (we
fixed this in #222). Click a project to open it; the editor
transitions into `AppState::Editor` and reads the project's
default scene.

## Default scene per project

`.jsn/project.jsn` carries a `default_scene` field. When set,
the editor opens that scene automatically when you load the
project. If unset, the launcher tries `assets/scene.jsn` as
the convention; if that's missing too, you start in an empty
viewport and `Ctrl+O` from there.

You can change the default from the file menu or by editing
the file directly. The editor watches the file, so external
edits show up without a restart.

## Multi-scene projects

Nothing stops you from putting many `.jsn` files in
`assets/scenes/`. The editor doesn't currently have a "scene
list" panel, so you switch between them via `File > Open`.

If you reference one scene from another (sub-scenes,
prefabs), that pattern is not built yet. Today scenes are
flat. See [Open Challenges](../developer-guide/open-challenges.md)
for what scene-as-asset would look like.

## Project files outside `assets/`

The editor only watches `assets/`. Code lives next to it
(`src/`, `bin/`), and Bevy's runtime asset path points at
`assets/`. If you put a `.jsn` somewhere else, jackdaw can
load it with `File > Open`, but the standalone binary won't
find it via Bevy's asset server.

## Common gotchas

- **Scene loaded but the viewport is empty.** Camera might
  be inside geometry. Press `F` with nothing selected (or
  with a known-visible entity selected) to reframe.
- **`File > Save` greys out.** No scene is open. Either
  `File > New Scene` or open one from the launcher.
- **Saved file has a weird path.** First save from a "New
  Scene" defaults to the project's `assets/scene.jsn`. If
  you want a different path, use `File > Save As`.
