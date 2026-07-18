# Scene management

A "scene" in jackdaw is one `.bsn` file. A "project" is a
normal Bevy crate: a folder with a `Cargo.toml`, a
`jackdaw.toml`, an `assets/` directory, and a
`.jsn/project.jsn` editor-settings file (legacy projects keep
it at the root). Scenes live under `assets/`.

## Save and load

- `Ctrl+S` saves the current scene to its on-disk path. The
  first save prompts for a path; pick something under
  `assets/`.
- `Ctrl+O` opens a scene from disk. The picker starts in the
  current project's `assets/` folder.
- `Ctrl+Shift+N` creates a new empty scene in memory; it is
  unsaved until you `Ctrl+S` it.

Scene files are human-readable, line-diffable, and designed to
read in `git diff` without making you cry. Legacy `.jsn` scenes
still open (import-only); see
[JSN Format](../developer-guide/jsn-format.md).

## Project select screen

The launcher (`AppState::ProjectSelect`) is the first thing
you see when you run `jackdaw` with no arguments. It shows:

- Recent projects, with timestamp and last-opened scene.
- A **New Project** button: pick **Game** or **Extension**,
  instantiated from a template embedded in the editor.
- An **Import** action for opening an existing Bevy project;
  see [Migrating an Existing Project](../getting-started/migrating-an-existing-project.md).

Recent projects with missing folders are filtered out (we
fixed this in #222). Click a project to open it; the editor
transitions into `AppState::Editor` and reads the project's
default scene.

## Default scene per project

`.jsn/project.jsn` carries a `default_scene` field. When set,
the editor opens that scene automatically when you load the
project. If unset, the launcher tries `assets/scene.bsn` as
the convention; if that's missing too, you start in an empty
viewport and `Ctrl+O` from there.

You can change the default from the file menu or by editing
the file directly. The editor watches the file, so external
edits show up without a restart.

## Multi-scene projects

Nothing stops you from putting many `.bsn` files in
`assets/scenes/`. The editor doesn't currently have a "scene
list" panel, so you switch between them via `File > Open`.

If you reference one scene from another (sub-scenes,
prefabs), that pattern is not built yet. Today scenes are
flat. See [Open Challenges](../developer-guide/open-challenges.md)
for what scene-as-asset would look like.

## Project files outside `assets/`

The editor only watches `assets/`. Code lives next to it
(`src/`), and Bevy's runtime asset path points at `assets/`.
If you put a scene file somewhere else, jackdaw can load it
with `File > Open`, but the standalone binary won't find it
via Bevy's asset server.

## Common gotchas

- **Scene loaded but the viewport is empty.** Camera might
  be inside geometry. Press `F` with nothing selected (or
  with a known-visible entity selected) to reframe.
- **`File > Save` greys out.** No scene is open. Either
  `File > New Scene` or open one from the launcher.
- **Saved file has a weird path.** First save from a "New
  Scene" defaults to the project's `assets/scene.bsn`. If
  you want a different path, use `File > Save As`.
