# Introduction

Jackdaw is a 3D level editor built with
[Bevy](https://bevyengine.org/). It does brush-based geometry,
material and texture management, heightmap terrain, and a
human-readable scene format (`.jsn`). The editor is itself a
Bevy plugin, so you can drop it into a project alongside your
own gameplay code without a separate runtime.

We are pre-1.0. Things change. Some pieces are in active
flux (PIE, plugin / dylib loading, the BSN migration), and
this book tries to call out what is solid versus what is in
flight.

## What you can do today

- Author levels by drawing brushes (TrenchBroom-style),
  carving, and applying materials.
- Build heightmap terrain with sculpt and erosion tools.
- Add Bevy-reflect components to entities through a picker,
  edit their fields, and see your custom components round-
  trip through save/load.
- Run the same scene as a standalone Bevy binary, with no
  editor in the dependency graph.
- Write extensions in plain Rust that plug into the editor's
  operator and panel system.

## Who this is for

Two audiences:

- Bevy developers who want a level editor for their game
  and don't want to glue something together themselves.
- Editor / tooling developers who want to build on top of a
  pluggable Bevy editor.

If you are coming from Hammer, TrenchBroom, or Unreal's
brush workflow, the brush model will feel familiar. If you
are coming from Unity or Godot, the closest analogue is the
scene editor; jackdaw's `.jsn` files play the role of
`.unity` / `.tscn` scenes.

## What this book covers

- **Getting Started**: install, scaffold a project, save a
  scene.
- **User Guide**: the panels and tools you actually click on.
- **Developer Guide**: how the editor is put together, how
  to write custom components, how to extend the editor with
  your own operators and windows.
- **Reference**: feature flags, configuration, file paths.
- [Open Challenges](developer-guide/open-challenges.md)
  lists what we have not built yet but want to. If you came
  here looking for something to hack on, start there.

## Where to find us

- **Discord**:
  [discord.gg/S9k2HRwc](https://discord.gg/S9k2HRwc). The
  fastest way to ask a question or share a screenshot.
- **GitHub**:
  [`jbuehler23/jackdaw`](https://github.com/jbuehler23/jackdaw).
  Source, issue tracker, and this book (under `book/`).

See [Giving Feedback](giving-feedback.md) for what kinds of
reports are most useful, and where to drop the RustWeek
questionnaire.

If you find a missing page or an instruction that doesn't
match what the editor does, the book lives at `book/` in the
repo. PRs welcome.
