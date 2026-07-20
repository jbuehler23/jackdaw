<p align="center">
  <img src="https://raw.githubusercontent.com/jbuehler23/jackdaw/main/assets/logo/jackdaw_icon.png" alt="Jackdaw" width="200" />
</p>

# Introduction

Jackdaw is a 3D level editor built with
[Bevy](https://bevyengine.org/). It does brush-based geometry,
material and texture management, heightmap terrain, and a
human-readable scene format (`.bsn`). Your project stays a
normal Bevy crate: the editor builds it as a dynamic library
and loads it, so your own `cargo build` and `cargo run` keep
compiling plain crates.io Bevy with nothing jackdaw-specific
in the manifest.

We are pre-1.0. Things change. Some pieces are still in
active flux, and this book tries to call out what is solid
versus what is in flight.

## What you can do today

- Author levels by drawing brushes, carving them with
  boolean operations, and applying materials.
- Build heightmap terrain with sculpt and erosion tools.
- Add Bevy-reflect components to entities through a picker,
  edit their fields, and see your custom components round-
  trip through save/load.
- Load the same scene in a standalone Bevy binary through
  `jackdaw_runtime`, with no editor in the dependency graph.
- Play the game from inside the editor, out of process, with
  live frames streamed into a panel.
- Write extensions in plain Rust that plug into the editor's
  operator and panel system.

## Who this is for

Two audiences:

- Bevy developers who want a level editor for their game
  and don't want to glue something together themselves.
- Editor / tooling developers who want to build on top of a
  pluggable Bevy editor.

If you have used a brush-based level editor before, the
geometry model will feel familiar: convex volumes carved and
combined in place rather than meshes imported from a
modelling tool. If you have used a scene editor, `.bsn` files
play the same role as its scene files, except they are plain
text you can read and diff.

## What this book covers

- **Getting Started**: install, scaffold a project, save a
  scene, or bring an existing Bevy game in.
- **User Guide**: the panels and tools you actually click on.
- **Developer Guide**: how the editor is put together, how
  to write custom components, how to extend the editor with
  your own operators and windows.
- **Reference**: configuration, file paths.
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

Bug reports are most useful with the scene file, the steps that
reproduced the problem, and what you expected instead.

If you find a missing page or an instruction that doesn't
match what the editor does, the book lives at `book/` in the
repo. PRs welcome.
