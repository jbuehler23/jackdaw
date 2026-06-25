# Installation

You need rustup and a recent nightly. Jackdaw currently targets the
toolchain pinned at the top of `.github/workflows/ci.yaml` (as of
writing, `nightly-2026-03-05`). Anything close to that should work,
but if you hit weird type-system errors, set:

```bash
rustup install nightly-2026-03-05
rustup default nightly-2026-03-05
```

## Linux system deps

You'll want the same packages bevy needs:

```bash
sudo apt install libasound2-dev libudev-dev libwayland-dev
```

Adjust for your package manager on other distros. macOS needs
nothing extra.

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

## Install jackdaw

`cargo install jackdaw` once `0.4` ships on crates.io. Until
then, build from source:

```bash
cargo install --git https://github.com/jbuehler23/jackdaw
```

The launcher will open. From there:

1. Click `+ New Game`.
2. Pick a name and a folder. The default template is
   `Game (static)`, which is the recommended path.
3. The launcher scaffolds the project, builds a per-project
   editor binary, and opens it. The first build pulls all of
   bevy and takes a few minutes. Subsequent opens are fast.

## Sanity check

Once the editor is open:

1. Right-click in the outliner. `Add > Cube`. A brush appears
   in the viewport.
2. `File > Save`. A file shows up at `assets/scene.jsn`.
3. `cargo run` from the project folder. The standalone binary
   loads the same scene, no editor.

If those three steps work, you're good. If they don't, file an
issue with what you tried and the error you saw. There's a
[Giving Feedback](../giving-feedback.md) page with more detail.
