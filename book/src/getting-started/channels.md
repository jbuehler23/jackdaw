# Stable and Bleeding Edge

Jackdaw ships on two channels.

|                  | Stable                          | Bleeding Edge                        |
| ---------------- | ------------------------------- | ------------------------------------ |
| Bevy             | a release, such as 0.20         | the `main` branch, at one commit     |
| Versions         | `0.20.0`, `0.20.1`, ...         | `nightly-2026-10-18`, one per night  |
| Crates           | on crates.io                    | from git, at the nightly's commit    |
| Install          | `jd editor install 0.20`        | `jd editor install bleeding-edge`    |
| Kept             | until you remove it             | 30 days, longer for known-good ones  |

Use Stable for anything you intend to ship. Bleeding Edge is for trying a
Bevy feature before it is released, and for helping find what Bevy's main
branch breaks before a release does.

## Branches

- `release-0.20` carries Jackdaw 0.20.x on Bevy 0.20. Fixes land on `main`
  first and are backported, and each 0.20.x release is tagged there.
- `main` tracks Bevy's `main` branch. Every Bevy dependency is
  `{ git = "https://github.com/bevyengine/bevy", branch = "main" }`, and
  `Cargo.lock` pins the commit. A pull request builds against the commit
  in the lockfile; `cargo update --package bevy` moves it.

When Bevy releases its next minor, `main` moves to it from crates.io, a new
`release-` branch is cut, and `main` goes back to tracking Bevy's `main`.

## Nightlies

Every night the [Nightly workflow](https://github.com/jbuehler23/jackdaw/actions/workflows/nightly.yaml)
moves `main`'s lockfile to Bevy's newest commit, runs the tests and builds
the bundles for every platform. Only a green night is published: as the
GitHub prerelease `nightly-YYYY-MM-DD`, and in the release index `jd`
installs from, with the Bevy and Jackdaw commits it was built from. A
Monday nightly is marked known good and kept for six months. The newest
nightly is never removed, even when later nights fail.

## Bleeding Edge projects

A project created with a nightly records it in `jackdaw.toml`:

```toml
[jackdaw]
version = "0.21.0-dev"
bevy = "main"
channel = "bleeding-edge"
build = "nightly-2026-10-18"
bevy_rev = "85503941e488152711a4695bbe248e9ed9f453fc"
```

and asks for Bevy's main branch in `Cargo.toml`, as Jackdaw's own crates do:

```toml
bevy = { git = "https://github.com/bevyengine/bevy", branch = "main" }
jackdaw_runtime = { git = "https://github.com/jbuehler23/jackdaw", rev = "...", features = [...] }
```

The editor's types and its extension SDK are built from one Bevy commit, so
the project has to build against that commit too. Cargo cannot be given a
`rev` beside the branch Jackdaw's crates name without building a second Bevy,
so the commit lives in `Cargo.lock`: before each build the editor moves the
lockfile to `bevy_rev` if it has drifted. To move a project to a newer
nightly, open it with that nightly; to do it by hand, run
`cargo update --package bevy --precise <commit>`.

`jd open` opens a Bleeding Edge project only in the nightly it records, and
names the `jd editor install` to run when that nightly is not installed.

## Reporting breakage

A red nightly opens an issue labelled
[`bevy-main`](https://github.com/jbuehler23/jackdaw/issues?q=label%3Abevy-main),
naming the Bevy commit and the failing step, and the next green night closes
it. If a published nightly is broken for you, open an issue with that label
and include:

- the nightly (`jackdaw --version` prints it, with the Bevy commit),
- what you did and what happened, with the log if there is one,
- whether the previous nightly, or the latest known-good one, works.

A maintainer can take a broken nightly out of the channel from the Nightly
workflow's `broken` input; `jd editor install bleeding-edge` then picks the
newest one that is not marked broken.
