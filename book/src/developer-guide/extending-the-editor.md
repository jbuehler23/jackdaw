# Extending the editor

Jackdaw has two deliberate extension seams.

## Custom standalone editors

`jackdaw_editor` exposes the same Bevy plugin group used by the official GUI:

```rust
use bevy::prelude::*;
use jackdaw_editor::prelude::*;

fn main() -> AppExit {
    App::new()
        .add_plugins(DefaultPlugins.set(editor_window_plugin()))
        .add_plugins((EnhancedInputPlugin, PhysicsPlugins::default()))
        .add_plugins(JackdawEditorPlugins::default())
        .run()
}
```

Use normal `PluginGroup` controls to disable or replace editor plugins and add
your own. This is unrestricted compile-time Rust composition and remains the
right choice for deeply customized editor distributions.

## Runtime extensions

Marketplace extensions use the focused `jackdaw_extension` crate:

```rust
use jackdaw_extension::prelude::*;

#[derive(Default)]
pub struct MyTool;

impl JackdawExtension for MyTool {
    fn id(&self) -> String { "example.my-tool".into() }
    fn label(&self) -> String { "My Tool".into() }

    fn register(&self, registrar: &mut ExtensionRegistrar<'_>) {
        // register operators, panels, menus, keymaps, and host-owned state
    }
}
```

Everything installed through `ExtensionRegistrar` is owned by that extension.
Disable, update, and uninstall remove those registrations immediately.
Superseded native libraries remain safely mapped but unreachable until the
process exits.

Runtime extensions deliberately cannot install extension-owned Bevy component
metadata or reflected Rust types. Use a custom editor build when that level of
access is required.

## Signed bundles

Create one publisher key, once:

```bash
jd extension keygen
```

That writes `publisher-key.pk8` into the Jackdaw data directory and refuses to
overwrite an existing one, because publishing an update under a new key makes
every user repeat the trust decision. Pass a path to keep it elsewhere.

Then, in the extension project, build and pack:

```bash
jd build
jd extension pack
jd extension verify my-tool-0.1.0-x86_64-unknown-linux-gnu.jdext
```

`pack` reads the bundle's identity, version, publisher, license, and homepage
from the project's `Cargo.toml`:

```toml
[package]
name = "my-tool"
version = "0.1.0"
authors = ["Example Studio <hello@example.com>"]
license = "MIT OR Apache-2.0"
repository = "https://example.com/my-tool"

[package.metadata.jackdaw]
label = "My Tool"
```

Any of those can be overridden with `--id`, `--label`, `--version`,
`--publisher`, `--license`, `--homepage`, `--key`, `--out`, and `--library`.

Build and pack separately on Linux, Windows, and macOS. A bundle records the
target triple and the SDK ABI string it was built against, and installs only
into a Jackdaw that matches both, so publish one bundle per target per Jackdaw
release.

Users install signed `.jdext` bundles from **Extensions** or with:

```bash
jd extension install my-tool.jdext
jd extension list
jd extension disable example.my-tool
jd extension enable example.my-tool
jd extension uninstall example.my-tool
```

The manifest and signature are checked before native code is loaded. Bundles
must match the exact Jackdaw SDK ABI and target. Trusting a publisher is an
explicit confirmation because native extensions run with the user's full
permissions. Updates are staged by version and activated atomically; a failed
activation leaves the previous version available for recovery. Inter-extension
dependencies are not supported by this bundle format.

## Distribution

Jackdaw does not host a registry. It provides the pieces an external one
needs: a signed bundle format, a compatibility key, and installation
straight from a URL.

Publish `.jdext` files wherever you like, and users install them with:

```bash
jd extension install https://example.com/my-tool-0.1.0-x86_64-unknown-linux-gnu.jdext
```

A URL and a local path go through the same gate. The signature, the library
checksum, the ABI and target match, and the publisher trust prompt all run on
the fetched bytes exactly as they do on a file, so a remote install is never
less checked than a local one. Plain `http://` is refused.

### Serving the right artifact

A bundle only installs into a Jackdaw whose compatibility key matches, so a
marketplace has to know the client's before it offers a download. Ask it:

```bash
$ jd extension abi --json
{"sdk_abi":"jackdaw-0.19.0-bevy-0.19-rustc 1.90.0","target":"x86_64-unknown-linux-gnu",
 "jackdaw":"0.19.0","bevy":"0.19"}
```

`sdk_abi` covers the Jackdaw version, the Bevy minor, and the exact rustc that
built the SDK; `target` is the platform triple. Key your catalogue on both.
In practice that means one bundle per target per Jackdaw release, rebuilt and
re-signed when Jackdaw updates. `jd extension verify` reports what a given
bundle claims, without installing it.
