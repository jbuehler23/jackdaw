# Troubleshooting

## Start with jd doctor

```bash
jd doctor                        # the machine
jd doctor --project /path/to/game  # the machine and one project
```

Run inside a project, `jd doctor` checks that project too. Every line starts
with a tag:

- `[ ok ]`: fine.
- `[info]`: worth knowing, nothing to do. A game project reports a missing
  extension SDK this way, because games never use it.
- `[warn]`: builds can go ahead, but something may misbehave. Read the
  `fix:` line under it.
- `[fail]`: builds will fail until the `fix:` line under it is done.

The last line says `all checks passed` or `some checks failed`, and the
command exits non-zero on any `[fail]`.

## Installing

### cmake, a compiler, MSVC, or the Xcode Command Line Tools

`[fail] cmake`, `[fail] C/C++ compiler`, `[fail] MSVC build tools` or
`[fail] Xcode Command Line Tools` means a native build tool is missing, and
every game build will fail. Install it as described under
[Prerequisites](installation.md#prerequisites), open a new terminal so PATH
is refreshed, and run `jd doctor` again.

On Windows, `[warn] MinGW gcc` means cmake may pick MinGW over MSVC and fail
to link with LNK1143. Set `CMAKE_GENERATOR="Visual Studio 17 2022"`.

### Rust without rustup

`[fail] rustup` or `[fail] Rust toolchain: ... without rustup` means Rust
came from somewhere other than rustup, such as a distribution package.
Jackdaw installs the toolchain extensions need through rustup, so install
rustup from <https://rustup.rs>.

### Linux development packages

`[warn] pkg-config` or `[warn] Bevy system libraries` means Bevy's audio and
input crates will not find the system libraries they link. Install Bevy's
[Linux dependencies](https://github.com/bevyengine/bevy/blob/main/docs/linux_dependencies.md).

### glibc too old for the Linux archive

The Linux archive needs glibc 2.39 or newer. On an older system the editor
fails to start with an error such as
``version `GLIBC_2.39' not found``. Install with `cargo install` instead.

### macOS says the app cannot be opened

The release binaries are not code-signed yet. Clear the quarantine flag on
the extracted folder:

```bash
xattr -dr com.apple.quarantine jackdaw-aarch64-apple-darwin
```

### Windows SmartScreen blocks the editor

Choose **More info**, then **Run anyway**.

## Running the editor

### Black window or crash on start on Linux

On Wayland the editor uses a transparent window. A surface that cannot
blend alpha, which some older GPUs and drivers report, falls back to an
opaque window. If the window is still black or the editor stops with a
surface error, force the opaque window:

```bash
JACKDAW_OPAQUE_WINDOW=1 jackdaw
```

X11 sessions always get the opaque window.

### Components do not show up, or Play times out

Run `jd doctor --project <path>`. A `[fail]` under runtime dependency,
runtime feature, runtime plugin or embedded Play means the runtime wiring is
incomplete; see
[Runtime wiring](migrating-an-existing-project.md#runtime-wiring).
`[warn] project types: not built yet` means the editor has not read the game's
types; run `jd build` or **Rebuild Project**.

## Building

### New project inside another cargo workspace

A project created inside another cargo workspace declares its own
`[workspace]` so it builds standalone, but that can break the outer
workspace. Jackdaw warns and names both options: add the line it prints to
the outer workspace's `Cargo.toml`, for example

```toml
exclude = ["games/my-game"]
```

or delete the `[workspace]` table from the new project's `Cargo.toml` to make
it a member.

### Dependencies do not resolve

`[fail] dependencies` shows cargo's first error. When a new project's crates
do not resolve, the warning names the source they were requested from:
crates.io at a version, a git revision, or a local checkout. An editor built
from git or a checkout scaffolds projects that depend on that same source,
so they need network access to the repository, or that checkout on the same
machine. Once the source is reachable, run `cargo update` if a release is
newer than your cached index, then `jd doctor --project <path>` again.

### SDK is older than the editor

In a source checkout, the editor and its SDK are built by separate commands
and can drift apart. An extension build then warns that the SDK is older
than the editor running it, and may fail to link. Rebuild the SDK with the
command in the warning, or use the SDK from `jd setup`.

### SDK is not usable

`[fail] SDK: ... is not usable` lists what is missing, then a fix. If
`JACKDAW_SDK_DIR` is set, unset it to use the SDK Jackdaw finds for itself;
otherwise run `jd setup`. See
[Where the SDK lives](../reference/configuration.md#where-the-sdk-lives).

### Extension built by an older Jackdaw

The log warns that an extension "exports no `jackdaw_register_types`; it was
built by an older jackdaw". It still loads, but rebuild it with the current
Jackdaw (`jd build` in the extension project, then `jd extension pack`) so its
types reach the editor on every platform.

### "The filename or extension is too long" on Windows

Older versions could fail a build with this error (os error 206). The rustc
wrapper now passes long command lines through a file. If you still see it,
report it with the editor log.

## Logs

- Windows release: `%LOCALAPPDATA%\jackdaw\logs\editor.log`. The previous
  run's log is kept beside it as `editor.previous.log`.
- Linux and macOS: the editor logs to the terminal. Start it from one to keep
  the log:

  ```bash
  jackdaw 2>&1 | tee jackdaw.log
  ```

- An SDK build started from the editor writes `setup.log` in its SDK cache
  directory, `jackdaw/sdk/<version>-<toolchain>/` under the local data
  directory.

## Reporting a bug

Open an issue at <https://github.com/jbuehler23/jackdaw/issues> with:

- the output of `jackdaw --version`,
- your OS and GPU,
- the full output of `jd doctor --project <path>`,
- the editor log,
- the steps that lead to the problem.
