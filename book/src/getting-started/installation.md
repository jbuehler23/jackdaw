# Installation

There are two ways to get jackdaw: a downloadable bundle for your
platform, or `cargo install` from source. Either way you need Rust
installed, because the editor builds your project's code with cargo.

## Prerequisites

rustup and cargo on your `PATH`. Jackdaw pins the toolchain it
builds its own SDK with (currently `nightly-2026-03-05`, in the
repo's `rust-toolchain.toml`) and installs that toolchain through
rustup when it needs it. Your project's own `cargo build` keeps
using whatever toolchain you have selected.

To check the prerequisites of an existing install before committing
to a long compile:

```bash
jackdaw-cli doctor
```

It reports cargo, rustup, and whether the pinned SDK toolchain is
already installed.

### Linux system deps

The same packages bevy needs:

```bash
sudo apt install libasound2-dev libudev-dev libwayland-dev
```

Adjust for your package manager on other distros. macOS needs
nothing extra.

## Downloadable bundle

Each tagged release publishes a per-platform archive on GitHub:
x86-64 Linux and Windows (MSVC), plus both Apple architectures.
Linux and macOS ship `.tar.zst`, Windows ships `.zip`.

The archive holds the editor, the `jackdaw-cli` tool, the game
runner, the rustc wrapper, the runtime dylibs, and a prebuilt SDK
under `sdk/`. Extract it and run the `jackdaw` binary from the
extracted folder; it finds the SDK beside itself, so there is no
first-run compile and no source checkout.

## cargo install

```bash
cargo install --git https://github.com/jbuehler23/jackdaw
```

That installs the `jackdaw` editor together with the
`jackdaw-rustc-wrapper` binary its build pipeline drives.
`jackdaw-cli` is a separate package
(`cargo install --git https://github.com/jbuehler23/jackdaw jackdaw_cli`);
the downloadable bundles carry it already.

Jackdaw's version tracks the Bevy minor it targets: the `0.19.x`
line builds against Bevy 0.19. Pick the release matching the Bevy
version your project uses.

### First-run SDK setup

An install that arrives without a prebuilt SDK builds one the first
time it is used. The editor gates its launcher behind a setup screen
with live progress while that runs: it installs the pinned toolchain
through rustup and compiles the SDK into `~/.jackdaw/sdk/` (or under
`$XDG_DATA_HOME` when that is set). Expect roughly 10 to 15 minutes,
once per jackdaw version. Later launches skip it.

To run the same setup from a terminal:

```bash
jackdaw-cli setup
```

A source checkout never does this; it uses the SDK built alongside
the editor in `target/`.

## Windows

Two gotchas, both from dependencies rather than jackdaw itself.

**cmake picks the wrong compiler.** Jackdaw's CSG kernel
(`manifold-csg-sys`) builds a C++ library with cmake. If MinGW GCC
is on your `PATH` (it ships with Git for Windows and Strawberry
Perl), cmake selects it instead of MSVC and the resulting object
files fail to link with `LNK1143: invalid or corrupt file`. Force
the Visual Studio generator before building:

```powershell
$env:CMAKE_GENERATOR = "Visual Studio 17 2022"
cargo install --git https://github.com/jbuehler23/jackdaw --force
```

Do not set `CC=cl` / `CXX=cl` to fix this; that breaks other
crates (e.g. `ring`) that rely on cmake's own compiler detection.

**Prefer the Vulkan backend.** Some DX12 driver/wgpu combinations
hit validation panics in the renderer. If you see a crash inside
`wgpu-core` (an `assertion left == right failed` in `render.rs`),
force Vulkan:

```powershell
$env:WGPU_BACKEND = "vulkan"
```

Vulkan is the more stable backend on Windows.

## Create a project

Run `jackdaw` with no arguments and the launcher opens. From there:

1. Click **New Project** and pick **Game** (or **Extension** for
   an editor extension).
2. Pick a name and a folder.
3. The launcher instantiates the project from a template embedded
   in the editor (no network involved) and opens it. The editor
   starts building the project's library in the background; your
   project's components appear in the inspector and pickers when
   that build finishes.

The same scaffold is available from the terminal as
`jackdaw new <name>` (add `--extension` for an extension). To bring
an existing Bevy game in instead, see
[Migrating an Existing Project](migrating-an-existing-project.md).

The result is a normal Bevy crate. Jackdaw keeps its own build
artifacts in a gitignored `.jackdaw/` directory and never touches
your `Cargo.toml`, `Cargo.lock`, or toolchain.

## Sanity check

Once the editor is open:

1. Right-click in the outliner. `Add > Cube`. A brush appears
   in the viewport.
2. `File > Save`. A file shows up at `assets/scene.bsn`.
3. `cargo run` from the project folder. The standalone binary
   loads the same scene, no editor.

If those three steps work, you're good. If they don't, file an
issue with what you tried and the error you saw.
