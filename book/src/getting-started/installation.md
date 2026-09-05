# Installation

Jackdaw supports a signed precompiled release, a source checkout, and Cargo
installation. All three provide the GUI, `jd`, the rustc wrapper, project
scaffolding, and import.

## Prerequisites

Install rustup and Cargo. On Linux, install Bevy's system dependencies:

```bash
sudo apt install libasound2-dev libudev-dev libwayland-dev
```

Check an installation with:

```bash
jd doctor
```

## Which one to use

All three give you the editor, `jd`, and project scaffolding. Games build
as ordinary cargo binaries against their own Bevy dependencies; the
editor asks that binary for its type schema and launches it for Play.
They differ mainly in whether the **extension SDK** (used for in-process
editor extensions) is already built.

|                         | Extension SDK                     | First game build   |
| ----------------------- | --------------------------------- | ------------------ |
| **Precompiled release** | already built, nothing to do      | ~9 min             |
| `cargo install`         | ~30 min, once per Jackdaw version | ~9 min             |
| Source checkout         | build the editor, then its SDK    | ~9 min             |

The extension SDK is a full compilation of Bevy and the Jackdaw API that
native editor extensions link against. A release archive ships it
prebuilt. The other two compile it on your machine, once per Jackdaw
version. That is a real half hour, so take the release archive unless you
have a reason not to.

Your game still compiles its own copy of Bevy the first time you build
it, around nine minutes, and every project pays that separately. The
editor learns your component types from that binary's schema extract, not
by linking the game into the editor process. After the first build,
rebuilds are 1 to 4 seconds, which is the number you actually live with.

Whichever you use, `jd doctor` reports which SDK is in play:

```
[ ok ] SDK: release bundle (/opt/jackdaw/sdk/x86_64-unknown-linux-gnu/libjackdaw_sdk.so)
```

## Precompiled release

Tagged releases provide checksummed, provenance-attested archives for
x86-64 Linux, x86-64 Windows, and Apple Silicon macOS. Extract the archive
and run `jackdaw`. Intel macOS users currently build from source.

The archive includes its pinned SDK, so nothing of Jackdaw is compiled on
your machine. Extract it and you can create a project immediately. That
project's first build still takes around nine minutes, since it compiles
its own Bevy; see [Which one to use](#which-one-to-use).

## Cargo install

```bash
cargo install --git https://github.com/jbuehler23/jackdaw jackdaw --locked
```

The editor is installed from git rather than crates.io because it depends on
`bevy_rerecast` by git, which crates.io does not accept. That restriction is
the editor's alone: the crates your own project depends on
(`jackdaw_runtime`, `jackdaw_extension`, and everything under them) are
published normally, so a scaffolded project resolves from the registry like
any other Bevy project.

The install provides `jackdaw`, `jd`, and
`jackdaw-rustc-wrapper`; do not install workspace packages individually.

This path has no prebuilt extension SDK, so it prepares one on first use:
roughly half an hour of compiling Bevy, once per Jackdaw version, before
native extensions can load. The editor shows a progress screen while it
runs; `jd setup` does the same thing from a terminal if you would rather
get it out of the way first. Cargo installs are self-contained; use a
precompiled release to load signed native extensions.

Jackdaw versions track Bevy minors: Jackdaw 0.19 targets Bevy 0.19, and so do
the `jackdaw_*` crates your project depends on.

## Source checkout

```bash
git clone https://github.com/jbuehler23/jackdaw
cd jackdaw
cargo run --bin jackdaw
```

The checkout uses the SDK under its own `target/`, in preference to any
prepared one, because editor extensions must link the SDK co-built with
the editor running them. That also means `cargo clean` throws the SDK
away. `jd doctor` reports which SDK is in play, so it is clear when a
checkout's is the one being used.

To build an editor with live native extension loading, use the same
shared-SDK mode as releases:

```bash
cargo run --bin jackdaw --features dylib --target "$(rustc -vV | sed -n 's/host: //p')"
```

## Create or import a project

Use the launcher's **New Game**, **New Extension**, and **Import Bevy
Project** actions, or:

```bash
jd new my-game                          # also: --extension, --path <dir>, --no-git
jd open my-game

jd import /path/to/existing-game        # preview
jd import /path/to/existing-game --apply
```

`jd import` previews exact file operations and changes nothing without
`--apply`. Jackdaw keeps editor state and the extracted type schema in the
project's gitignored `.jackdaw/` directory. Ordinary `cargo run` remains a
normal game build and does not invoke Jackdaw.

`jd new` initialises a git repository, the way `cargo new` does, unless the
destination already sits inside one or you pass `--no-git`.

If anything looks wrong, `jd doctor` reports the build prerequisites, and
`jd doctor --project <path>` adds the project's own setup state, including
whether its dependencies resolve.

After a Jackdaw update, `jd upgrade <path>` moves a project onto the new
version.
