# Installation

Jackdaw supports a precompiled release, a Cargo installation, and a source
checkout. All three provide the GUI, `jd`, the rustc wrapper, project
scaffolding, and import.

Jackdaw versions track Bevy minors: Jackdaw 0.19 targets Bevy 0.19, and so do
the `jackdaw_*` crates your project depends on.

## Which one to use

|                         | Extension SDK                           |
| ----------------------- | --------------------------------------- |
| **Precompiled release** | ships prebuilt                          |
| `cargo install`         | built on demand, only for extensions    |
| Source checkout         | built from the checkout, for extensions |

Take a precompiled release if one exists for your platform; the
[installer](#install-script) is the quickest way to get one. Games never need
the SDK; see [The extension SDK](#the-extension-sdk).

## Prerequisites

Every install path needs these, because your game compiles on your machine.
`jd doctor` checks each one and prints the fix for anything missing.

### Linux

- rustup and Cargo, from <https://rustup.rs>.
- cmake.
- A C and C++ compiler: `build-essential` (Debian, Ubuntu), `base-devel`
  (Arch) or `gcc-c++` (Fedora).
- pkg-config and Bevy's system libraries: the alsa, udev, wayland and x11
  development packages. See Bevy's
  [Linux dependencies](https://github.com/bevyengine/bevy/blob/main/docs/linux_dependencies.md)
  for your distribution. On Debian or Ubuntu:

```bash
sudo apt install build-essential cmake pkg-config libasound2-dev libudev-dev libwayland-dev libxkbcommon-dev libx11-dev
```

### Windows

- rustup and Cargo, from <https://rustup.rs>.
- Visual Studio Build Tools with the **Desktop development with C++**
  workload, which provides the linker Rust needs:
  <https://visualstudio.microsoft.com/visual-cpp-build-tools/>.
- cmake, from <https://cmake.org/download>. Choose the option that adds it to
  PATH.

If a MinGW `gcc` is also on PATH, cmake may pick it over MSVC and fail to link
(LNK1143). Set `CMAKE_GENERATOR="Visual Studio 17 2022"` before building.

### macOS

- rustup and Cargo, from <https://rustup.rs>.
- The Xcode Command Line Tools: `xcode-select --install`.
- cmake: `brew install cmake`, or from <https://cmake.org/download>.

### Check

```bash
jd doctor
```

See [Troubleshooting](troubleshooting.md) for how to read the report.

## Install script

The install script downloads the release archive for your platform, checks
it against its `.sha256`, and installs it for your user, without admin
rights. It supports x86-64 Linux, Apple Silicon macOS and x86-64 Windows.

Linux and macOS:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/jbuehler23/jackdaw/releases/latest/download/install.sh | sh
```

Windows (PowerShell 5.1 or 7):

```powershell
irm https://github.com/jbuehler23/jackdaw/releases/latest/download/install.ps1 | iex
```

Then open a new terminal and run `jackdaw`, or `jd doctor` to check the
[prerequisites](#prerequisites).

### What it changes

|                  | Linux, macOS                                         | Windows                                     |
| ---------------- | ---------------------------------------------------- | ------------------------------------------- |
| Release folders  | `~/.local/share/jackdaw/install/<version>/`          | `%LOCALAPPDATA%\jackdaw\install\<version>\` |
| Active version   | `current` symlink beside them                        | `current` directory junction beside them    |
| Commands         | `jackdaw` and `jd` in `~/.local/bin`                 | `...\install\current` on the user `PATH`     |
| `PATH`           | a line in `~/.profile`, `~/.bashrc`, `~/.zshenv` and a fish `conf.d` file, if `~/.local/bin` is not already on `PATH` | the user `PATH` in the registry |

Each release folder is a complete archive folder, kept whole: the editor
finds its runtime libraries and SDK next to its own executable. On Linux
`~/.local/bin` holds symlinks. On macOS it holds small scripts that start
the real binary, because macOS reports a program started through a symlink
by the link's path, where there is no SDK.

`XDG_DATA_HOME` moves the Linux and macOS install along with it. The
installer does not touch your projects, settings, extensions or SDK cache.

### Options

Pass options to the piped script after `sh -s --`, for example:

```bash
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/jbuehler23/jackdaw/releases/latest/download/install.sh | sh -s -- --no-modify-path
```

| `install.sh`        | `install.ps1`    | Environment                  | Effect                                       |
| ------------------- | ---------------- | ---------------------------- | -------------------------------------------- |
| `--version <v>`     | `-Version <v>`   | `JACKDAW_VERSION`            | Install release `v<v>`, such as `0.19.0-rc.9` |
| `--prefix <dir>`    | `-Prefix <dir>`  | `JACKDAW_INSTALL_DIR`        | Keep release folders in `<dir>`              |
| `--bin-dir <dir>`   |                  | `JACKDAW_BIN_DIR`            | Put the commands in `<dir>`                   |
| `--no-modify-path`  | `-NoModifyPath`  | `JACKDAW_NO_MODIFY_PATH=1`   | Leave shell files and `PATH` alone           |
| `--yes`             |                  |                              | Do not ask before installing                 |
| `--uninstall`       | `-Uninstall`     |                              | Remove what the installer added              |

`install.sh` asks for confirmation only when run from a file in a terminal;
piped, it does not ask. `iex` cannot pass parameters, so on Windows run the
script as a script block instead:

```powershell
& ([scriptblock]::Create((irm https://github.com/jbuehler23/jackdaw/releases/latest/download/install.ps1))) -Version 0.19.0-rc.9
```

`releases/latest` is the newest final release. Release candidates carry the
installers too: take them from the release's own tag, for example
`https://github.com/jbuehler23/jackdaw/releases/download/v0.19.0-rc.9/install.sh`,
and pass `--version`.

### Upgrade, roll back and uninstall

Run the installer again to upgrade. It installs the new version beside the
old one, switches `current` to it, and keeps the version it replaced for
rollback; older versions are removed. To roll back, run it with
`--version <previous>`. On Windows, close Jackdaw before upgrading.

To uninstall, run it with `--uninstall` (`-Uninstall` on Windows), with the
same `--prefix` if you set one. It removes the release folders, the commands
and the `PATH` lines it added.

## Precompiled release

[Tagged releases](https://github.com/jbuehler23/jackdaw/releases) provide
archives for x86-64 Linux, x86-64 Windows, and Apple Silicon macOS, each with
a `.sha256` checksum and a build provenance attestation. Intel macOS users
install with Cargo or from source.

To install an archive by hand instead of with the
[install script](#install-script), read on. Each archive holds one folder with `jackdaw`, `jd`, `jackdaw-rustc-wrapper`,
their runtime libraries, and the prebuilt SDK. Keep the folder together, and
add it to your `PATH` to use `jd` from a terminal.

To check an archive's provenance:

```bash
gh attestation verify <archive> -R jbuehler23/jackdaw
```

The binaries are not code-signed yet, so Windows and macOS ask before running
them.

### Linux

Download `jackdaw-x86_64-unknown-linux-gnu.tar.zst`, then:

```bash
tar -xf jackdaw-x86_64-unknown-linux-gnu.tar.zst
./jackdaw-x86_64-unknown-linux-gnu/jackdaw
```

Extracting a `.tar.zst` needs the `zstd` tool, which most distributions ship;
install it from your package manager if `tar` reports it missing.

The archive needs glibc 2.31 or newer, as on Ubuntu 20.04 or Debian 11. On
older distributions, use `cargo install` or a source checkout instead.

### Windows

Download `jackdaw-x86_64-pc-windows-msvc.zip`, extract it, and run
`jackdaw.exe` from the extracted folder. SmartScreen shows "Windows protected
your PC" the first time: choose **More info**, then **Run anyway**.

### macOS

Download `jackdaw-aarch64-apple-darwin.zip` and extract it (double-click it
in Finder, or use `unzip`). Then clear the quarantine flag macOS puts on
downloaded files, which otherwise blocks the unsigned binaries and their
libraries:

```bash
unzip jackdaw-aarch64-apple-darwin.zip
xattr -dr com.apple.quarantine jackdaw-aarch64-apple-darwin
./jackdaw-aarch64-apple-darwin/jackdaw
```

## Cargo install

```bash
cargo install jackdaw --locked
```

Any stable Rust from 1.95 on builds the editor. The install provides
`jackdaw`, `jd`, and `jackdaw-rustc-wrapper`; do not install workspace
packages individually.

The build needs about 10 GB of memory at Cargo's default parallelism. With
8 GB, close other programs and add `--jobs 2`, or take the release archive.

Cargo installs are self-contained; use a precompiled release to load signed
native extensions.

## Source checkout

```bash
git clone https://github.com/jbuehler23/jackdaw
cd jackdaw
cargo run --bin jackdaw
```

The checkout pins its compiler in `rust-toolchain.toml`. A debug build of the
editor needs more memory than an install; on a 16 GB machine, build with
`--jobs 4`, or set `jobs = 4` under `[build]` in `~/.cargo/config.toml`.

The checkout uses the SDK under its own `target/`, in preference to any
prepared one, because editor extensions must link the SDK co-built with the
editor running them. That also means `cargo clean` throws the SDK away.

To build an editor with live native extension loading, use the same
shared-SDK mode as releases:

```bash
cargo run --bin jackdaw --features dylib --target "$(rustc -vV | sed -n 's/host: //p')"
```

## The extension SDK

The extension SDK is a full compilation of Bevy and the Jackdaw API that
native editor extensions link against. Games do not use it: a game builds as
an ordinary cargo binary against its own Bevy, and the editor reads its
component types from that binary. A game's first build compiles Bevy, like
any Bevy project.

A release archive ships the SDK prebuilt. A Cargo install builds it the first
time you open an extension project, or when you press **Build SDK** on the
launcher. That takes 20-30 minutes on a typical machine, once per Jackdaw
version. To do it from a terminal instead:

```bash
jd setup
```

The SDK builds with the same Rust release that built the editor. Jackdaw
installs that release through rustup the first time it is needed, without
changing your default; `jd doctor` names it under `editor toolchain`.

A built SDK is cached in `jackdaw/sdk/` under the local data directory:
`~/.local/share` on Linux, `~/Library/Application Support` on macOS, and
`%LOCALAPPDATA%` on Windows. See
[Where the SDK lives](../reference/configuration.md#where-the-sdk-lives).
`jd doctor` reports which SDK is in play:

```
[ ok ] SDK: release bundle (/opt/jackdaw/sdk/x86_64-unknown-linux-gnu/libjackdaw_sdk.so)
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
`--apply`; see [Migrating an Existing Project](migrating-an-existing-project.md).
Jackdaw keeps editor state and the extracted type schema in the project's
gitignored `.jackdaw/` directory. Ordinary `cargo run` remains a normal game
build and does not invoke Jackdaw.

`jd new` initialises a git repository, the way `cargo new` does, unless the
destination already sits inside one or you pass `--no-git`.

`jd doctor --project <path>` adds the project's own setup state, including
whether its dependencies resolve. After a Jackdaw update, `jd upgrade <path>`
moves a project onto the new version.
