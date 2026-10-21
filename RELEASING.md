# Releasing

Releases are cut from the `release-0.<minor>` branch for that Bevy minor.
Commands below assume a checkout of that branch and `gh` signed in to
`jbuehler23/jackdaw`.

## One-time setup

The release workflow signs `releases.json` on the `index` release, and `jd`
and the launcher only accept an index signed by the key built into them.

1. Generate the signing key (an Ed25519 PKCS#8 key, base64 on one line) and
   print its public half:

   ```sh
   openssl genpkey -algorithm ed25519 -outform DER | openssl base64 -A > index-key.b64
   JACKDAW_INDEX_SIGNING_KEY="$(cat index-key.b64)" \
     cargo run -q -p jackdaw_releases --features index-tool --bin jackdaw-release-index -- public-key
   ```

2. Store it as the repository secret, then keep `index-key.b64` somewhere
   offline and delete the local copy:

   ```sh
   gh secret set JACKDAW_INDEX_SIGNING_KEY --repo jbuehler23/jackdaw < index-key.b64
   ```

3. Paste the 64 hex digits into `OFFICIAL_INDEX_KEY` in
   `crates/jackdaw_releases/src/signing.rs`:

   ```rust
   pub const OFFICIAL_INDEX_KEY: Option<&str> = Some("<64 hex digits>");
   ```

   Merge this into the release branch and into `main` before tagging the
   first release that should verify the index. Builds without it refuse
   every index.

4. Create the `index` release once. It must never become the latest
   release, because the install scripts download from `releases/latest`:

   ```sh
   gh release create index --repo jbuehler23/jackdaw --target main \
     --title "Release index" --notes "Signed list of releases read by jd." \
     --prerelease --latest=false
   ```

   The first final release writes `releases.json` and `releases.json.sig`
   to it.

## Each release

1. Open a PR into the release branch that sets the workspace version, every
   `version = "..."` on a jackdaw path dependency, and the lockfiles
   (`Cargo.lock`, `xtask/Cargo.lock`, `integrations/*/Cargo.lock`) to the new
   version, such as `0.20.0-rc.1`. Before merging it, check the packages
   and the publish order:

   ```sh
   cargo xtask package target/package --check
   cargo xtask publish --dry-run
   ```

2. Tag the release candidate from the merged branch:

   ```sh
   git tag v0.20.0-rc.1 origin/release-0.20
   git push origin v0.20.0-rc.1
   ```

   `release.yaml` runs the heavy tier, checks that scaffolded projects
   resolve against the packaged crates, builds and smoke-tests the bundles
   for Linux, macOS and Windows, tests `install.sh` and `install.ps1`,
   publishes the crates with `cargo xtask publish` (dependency order, skips
   versions already on crates.io, waits out rate limits; re-run the job if
   it stops), checks that a scaffolded project resolves from crates.io, and
   creates a GitHub prerelease with the archives, install scripts and
   attestations. `regression.yaml` and `onboarding.yaml` also run on
   release candidate tags. Release candidates are not added to the index.

   A tag that does not match the workspace version builds bundles but
   publishes nothing.

3. Post the tester checklist with the prerelease link. Fixes go into the release
   branch by PR, and into `main` too when they apply there. Repeat from
   step 1 with the next `-rc.N`.

4. For the final release, set the version to `0.20.0` the same way and tag
   `v0.20.0`. The final tag builds bundles with one codegen unit and thin
   LTO, skips the heavy tier, regression and onboarding (the last release
   candidate passed them), publishes the crates, marks the GitHub release
   as latest, and the `Release index` job adds it to `releases.json` and
   signs it.

5. Check the index. The `Release index` job warns and leaves the index
   unchanged when the secret or the `index` release is missing, so look
   for warnings in the run, then:

   ```sh
   gh release view index --repo jbuehler23/jackdaw
   jd editor refresh
   jd editor list --available
   ```

   `jd` from the new release should list it without warnings.
