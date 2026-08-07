# Installation

Jackdaw provides a signed precompiled release. You can also
build with a Cargo installation or source checkout. All
options provide everything you need to get started with
Jackdaw.

Jackdaw versions track Bevy minors: Jackdaw
0.19 targets Bevy 0.19.

## Prerequisites

Install rustup and Cargo. On Linux, install Bevy's system dependencies:

```bash
sudo apt install libasound2-dev libudev-dev libwayland-dev
```

Check an installation with:

```bash
jd doctor
```

## Choose your install method

Install methods are equivalent, however building from
cargo or source will take sigificantly longer initially
as the editor and SDK must be compiled. You should use the
precompiled release if you're unsure which option to choose.

| | Initial build time |
|---|---|
| **Precompiled release** | Already built |
| `cargo install` | ~30 min |
| Source checkout | ~30 min |

The SDK is a full compilation of Bevy and the Jackdaw
API that projects link against. You can use `jd doctor` to see the active SDK:

```
[ ok ] SDK: release bundle (/opt/jackdaw/sdk/x86_64-unknown-linux-gnu/libjackdaw_sdk.so)
```

### Precompiled release

[Tagged
releases](https://github.com/jbuehler23/jackdaw/releases)
provide checksummed archives for x86-64 Linux, x86-64
Windows, and Apple Silicon macOS. Extract the archive and
run `jackdaw`. Intel macOS users currently must build from
source.

### Compile from source

Both install methods provide `jackdaw`, `jd`, `jackdaw-runner`, and
`jackdaw-rustc-wrapper`. Do not install workspace packages individually.


**Cargo installation**

To install via cargo, install pointing to the git
repository.  A crates.io install is not available because
it depends on `bevy_rerecast` by git which crates.io does
not allow.

```bash
cargo install --git https://github.com/jbuehler23/jackdaw jackdaw --locked
```

**Source checkout installation**

You can also install by checkout out the source. Note that 
this method uses the SDK under its own `target/`, which
means `cargo clean` will require the SDK to be rebuilt.


```bash
git clone https://github.com/jbuehler23/jackdaw
cd jackdaw
cargo run --bin jackdaw
```

To build an editor with live native extension loading, use the
same shared-SDK mode as releases:

```bash
cargo run --bin jackdaw --features dylib --target "$(rustc -vV | sed -n 's/host: //p')"
```

**Setup**

Compiling via cargo or by source checkout will take roughly
30 minutes initially as the editor and SDK must be compiled.
You can initiate SDK setup after editor compilation by opening the editor or running `jd
setup` from the terminal. The editor shows a progress screen
while it runs.


## Create or import a project

Use the launcher's **New Game**, **New Extension**, and **Import Bevy
Project**. Jackdaw keeps generated builds in the project's gitignored
`.jackdaw/`.

You can also create or import a project via the command line:


```bash
jd new my-game                          # also: --extension, --path <dir>, --no-git
jd open my-game

jd import /path/to/existing-game        # preview
jd import /path/to/existing-game --apply
```

To import a project and apply changes, use `jd import
/path/to/existing-game --apply`. To preview the changes
first, omit `--apply`.

`cargo run` builds the project without invoking Jackdaw.

`jd new` initialises a git repository, the way `cargo new`
does, unless the destination already sits inside one or you
pass `--no-git`.

If anything looks wrong, `jd doctor` reports the build
prerequisites, and `jd doctor --project <path>` adds the
project's setup state, including whether its dependencies
resolve.

After a Jackdaw update, use `jd upgrade <path>` to update
a project.
