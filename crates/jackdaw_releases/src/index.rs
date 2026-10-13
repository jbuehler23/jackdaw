//! The signed release index, `releases.json`, and each release's
//! `modules.json`.

use ed25519_dalek::VerifyingKey;
use serde::{Deserialize, Serialize};

use crate::signing::{SignatureError, verify_detached};
use crate::version;

/// The index layout this build reads and writes.
pub const SCHEMA: u32 = 1;

/// The `kind` of a `releases.json`.
pub const RELEASES_KIND: &str = "releases";

/// The `kind` of a `modules.json`.
pub const MODULES_KIND: &str = "modules";

/// How long Bleeding Edge builds stay in the index when it names no period.
pub const DEFAULT_RETAIN_DAYS: u32 = 30;

/// How long a nightly picked as known good stays in the index.
pub const KNOWN_GOOD_RETAIN_DAYS: u32 = 183;

/// Why an index or modules list was not accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexError {
    Signature(SignatureError),
    Malformed(String),
    /// Written for a newer layout than [`SCHEMA`].
    NewerSchema(u32),
    /// A signed file of another kind, such as a modules list served as the
    /// release index.
    WrongKind {
        expected: &'static str,
    },
}

impl std::fmt::Display for IndexError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Signature(error) => write!(f, "release index signature: {error}"),
            Self::Malformed(error) => write!(f, "release index does not parse: {error}"),
            Self::NewerSchema(schema) => write!(
                f,
                "release index uses schema {schema}, newer than this build reads; update jd"
            ),
            Self::WrongKind { expected } => write!(f, "expected a `{expected}` file"),
        }
    }
}

impl std::error::Error for IndexError {}

/// Which update stream a release belongs to.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Channel {
    /// Released versions, one line per Bevy minor.
    #[serde(rename = "stable")]
    Stable,
    /// Nightly builds against Bevy's main branch.
    #[serde(rename = "bleeding-edge")]
    BleedingEdge,
}

impl Channel {
    /// The name used in the index, `jackdaw.toml` and the command line.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::BleedingEdge => "bleeding-edge",
        }
    }

    /// Read a channel name, accepting `nightly` for Bleeding Edge.
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "stable" => Some(Self::Stable),
            "bleeding-edge" | "nightly" => Some(Self::BleedingEdge),
            _ => None,
        }
    }
}

/// Support state of a stable line.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LineStatus {
    Current,
    Previous,
    Unsupported,
}

/// The newest release of one Bevy minor on the Stable channel.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct StableLine {
    /// The Bevy minor, e.g. `0.20`.
    pub bevy: String,
    /// The newest release on this line.
    pub latest: String,
    pub status: LineStatus,
}

/// The Stable channel: one entry per Bevy minor, newest first.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct StableChannel {
    #[serde(default)]
    pub lines: Vec<StableLine>,
}

/// The Bleeding Edge channel.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct BleedingEdgeChannel {
    /// The newest nightly that is not marked broken.
    #[serde(default)]
    pub latest: Option<String>,
    /// How many days of nightlies the index keeps.
    #[serde(default = "default_retain_days")]
    pub retain_days: u32,
}

impl Default for BleedingEdgeChannel {
    fn default() -> Self {
        Self {
            latest: None,
            retain_days: DEFAULT_RETAIN_DAYS,
        }
    }
}

fn default_retain_days() -> u32 {
    DEFAULT_RETAIN_DAYS
}

/// Both channels' summaries.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Channels {
    #[serde(default)]
    pub stable: StableChannel,
    #[serde(default, rename = "bleeding-edge")]
    pub bleeding_edge: BleedingEdgeChannel,
}

/// One downloadable editor bundle.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Artifact {
    /// The Rust target triple the bundle runs on.
    pub target: String,
    pub url: String,
    /// Lowercase hex SHA-256 of the downloaded file.
    pub sha256: String,
    /// Download size in bytes.
    pub size: u64,
    /// Unpacked size in bytes, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installed_size: Option<u64>,
    /// Build provenance attestation, when published.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attestation: Option<String>,
}

/// One editor release.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// `0.20.1` on Stable, `nightly-YYYY-MM-DD` on Bleeding Edge.
    pub version: String,
    pub channel: Channel,
    /// The Bevy minor it targets, or `main` for a nightly.
    pub bevy: String,
    /// The Bevy commit a nightly was built against.
    #[serde(default)]
    pub bevy_rev: Option<String>,
    /// The jackdaw commit a nightly was built from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jackdaw_rev: Option<String>,
    /// Release date, `YYYY-MM-DD`.
    pub date: String,
    #[serde(default)]
    pub rustc: Option<String>,
    /// The SDK identity extensions are built against.
    #[serde(default)]
    pub sdk_id: Option<String>,
    #[serde(default)]
    pub notes_url: Option<String>,
    /// Withdrawn: never chosen by a channel or line, only by exact version.
    #[serde(default)]
    pub yanked: bool,
    /// A nightly whose checks failed; never the channel's latest.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub broken: bool,
    /// A nightly picked as a known-good weekly build, kept longer.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub known_good: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub known_issues: Vec<String>,
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
    /// URL of this release's signed `modules.json`.
    #[serde(default)]
    pub modules_index: Option<String>,
}

impl Release {
    /// The bundle for `target`, if this release ships one.
    pub fn artifact_for(&self, target: &str) -> Option<&Artifact> {
        self.artifacts
            .iter()
            .find(|artifact| artifact.target == target)
    }

    /// Whether a channel or line query may pick this release.
    fn selectable(&self) -> bool {
        !self.yanked && !self.broken
    }
}

/// The whole `releases.json`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ReleaseIndex {
    pub schema: u32,
    /// Always [`RELEASES_KIND`].
    pub kind: String,
    /// Raised by one on every publish, so a client can refuse an index
    /// older than one it already saw.
    #[serde(default)]
    pub serial: u64,
    /// When the index was written, RFC 3339.
    #[serde(default)]
    pub generated: Option<String>,
    #[serde(default)]
    pub channels: Channels,
    #[serde(default)]
    pub releases: Vec<Release>,
}

impl Default for ReleaseIndex {
    fn default() -> Self {
        Self {
            schema: SCHEMA,
            kind: RELEASES_KIND.to_string(),
            serial: 0,
            generated: None,
            channels: Channels::default(),
            releases: Vec::new(),
        }
    }
}

impl ReleaseIndex {
    /// Parse `bytes` after checking `signature` over them with `key`.
    pub fn from_signed(
        bytes: &[u8],
        signature: &[u8],
        key: &VerifyingKey,
    ) -> Result<Self, IndexError> {
        verify_detached(key, bytes, signature).map_err(IndexError::Signature)?;
        Self::parse(bytes)
    }

    /// Parse an index without checking a signature, for one this process
    /// is about to sign.
    pub fn parse(bytes: &[u8]) -> Result<Self, IndexError> {
        parse_schema_checked(bytes, RELEASES_KIND)
    }

    /// The release a version, line or channel name selects.
    ///
    /// `stable` is the current line's latest release and `bleeding-edge`
    /// (or `nightly`) the newest healthy nightly. A line such as `0.20`
    /// picks its newest release. An exact version is taken as is, unless it
    /// was yanked.
    pub fn resolve(&self, query: &str) -> Result<&Release, String> {
        let query = query.strip_prefix('v').unwrap_or(query);
        match Channel::parse(query) {
            Some(Channel::Stable) => self.latest_stable(),
            Some(Channel::BleedingEdge) => self.latest_nightly(),
            None if version::is_line(query) => self
                .newest(|release| {
                    release.channel == Channel::Stable
                        && version::line_of(&release.version).as_deref() == Some(query)
                })
                .ok_or_else(|| format!("no release of the {query} line is published")),
            None => {
                let release = self
                    .find(query)
                    .ok_or_else(|| format!("no release `{query}` is published"))?;
                if release.yanked {
                    return Err(format!(
                        "{query} was withdrawn; install its line's newest release instead"
                    ));
                }
                Ok(release)
            }
        }
    }

    /// The release with exactly this version.
    pub fn find(&self, version: &str) -> Option<&Release> {
        self.releases
            .iter()
            .find(|release| release.version == version)
    }

    fn latest_stable(&self) -> Result<&Release, String> {
        let current = self
            .channels
            .stable
            .lines
            .iter()
            .find(|line| line.status == LineStatus::Current)
            .and_then(|line| self.find(&line.latest))
            .filter(|release| release.selectable());
        current
            .or_else(|| self.newest(|release| release.channel == Channel::Stable))
            .ok_or_else(|| "no stable release is published".to_string())
    }

    fn latest_nightly(&self) -> Result<&Release, String> {
        self.channels
            .bleeding_edge
            .latest
            .as_deref()
            .and_then(|name| self.find(name))
            .filter(|release| release.selectable())
            .or_else(|| self.newest(|release| release.channel == Channel::BleedingEdge))
            .ok_or_else(|| "no Bleeding Edge build is published".to_string())
    }

    fn newest(&self, include: impl Fn(&Release) -> bool) -> Option<&Release> {
        self.releases
            .iter()
            .filter(|release| release.selectable() && include(release))
            .max_by(|a, b| version::compare(&a.version, &b.version))
    }

    /// Add or replace `release`, drop nightlies past retention as of
    /// `today` (days since 1970-01-01), and recompute the channel
    /// summaries.
    pub fn add_release(&mut self, release: Release, today: i64) {
        self.releases
            .retain(|known| known.version != release.version);
        self.releases.push(release);
        let retain = i64::from(self.channels.bleeding_edge.retain_days);
        self.releases.retain(|release| {
            let Some(day) = version::nightly_day(&release.version) else {
                return true;
            };
            let limit = if release.known_good {
                i64::from(KNOWN_GOOD_RETAIN_DAYS)
            } else {
                retain
            };
            today - day <= limit
        });
        self.releases
            .sort_by(|a, b| version::compare(&b.version, &a.version));
        self.refresh_channels();
    }

    /// Recompute [`Channels`] from the releases.
    pub fn refresh_channels(&mut self) {
        let mut lines: Vec<StableLine> = Vec::new();
        for release in &self.releases {
            if release.channel != Channel::Stable || !release.selectable() {
                continue;
            }
            match lines.iter_mut().find(|line| line.bevy == release.bevy) {
                Some(line) => {
                    if version::compare(&release.version, &line.latest).is_gt() {
                        line.latest.clone_from(&release.version);
                    }
                }
                None => lines.push(StableLine {
                    bevy: release.bevy.clone(),
                    latest: release.version.clone(),
                    status: LineStatus::Unsupported,
                }),
            }
        }
        lines.sort_by(|a, b| version::compare(&b.latest, &a.latest));
        for (rank, line) in lines.iter_mut().enumerate() {
            line.status = match rank {
                0 => LineStatus::Current,
                1 => LineStatus::Previous,
                _ => LineStatus::Unsupported,
            };
        }
        self.channels.stable.lines = lines;
        self.channels.bleeding_edge.latest = self
            .newest(|release| release.channel == Channel::BleedingEdge)
            .map(|release| release.version.clone());
    }
}

/// One module build in a release's `modules.json`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ModuleRelease {
    /// Reverse-DNS id, e.g. `jackdaw.physics`.
    pub id: String,
    pub label: String,
    pub version: String,
    #[serde(default)]
    pub requires_restart: bool,
    #[serde(default)]
    pub depends: Vec<String>,
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
}

/// A release's `modules.json`: the optional editor modules built with it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ModulesIndex {
    pub schema: u32,
    /// Always [`MODULES_KIND`].
    pub kind: String,
    /// The editor version the modules were built with.
    pub editor: String,
    #[serde(default)]
    pub sdk_id: Option<String>,
    #[serde(default)]
    pub modules: Vec<ModuleRelease>,
}

impl ModulesIndex {
    /// Parse `bytes` after checking `signature` over them with `key`.
    pub fn from_signed(
        bytes: &[u8],
        signature: &[u8],
        key: &VerifyingKey,
    ) -> Result<Self, IndexError> {
        verify_detached(key, bytes, signature).map_err(IndexError::Signature)?;
        parse_schema_checked(bytes, MODULES_KIND)
    }

    /// The module a short name such as `physics` or a full id names.
    pub fn module(&self, name: &str) -> Option<&ModuleRelease> {
        let id = module_id(name);
        self.modules.iter().find(|module| module.id == id)
    }
}

/// The full id of an official module named by its short name.
pub fn module_id(name: &str) -> String {
    if name.contains('.') {
        name.to_string()
    } else {
        format!("jackdaw.{name}")
    }
}

fn parse_schema_checked<T: serde::de::DeserializeOwned>(
    bytes: &[u8],
    kind: &'static str,
) -> Result<T, IndexError> {
    #[derive(Deserialize)]
    struct Header {
        schema: u32,
        #[serde(default)]
        kind: Option<String>,
    }
    let header: Header =
        serde_json::from_slice(bytes).map_err(|error| IndexError::Malformed(error.to_string()))?;
    if header.schema > SCHEMA {
        return Err(IndexError::NewerSchema(header.schema));
    }
    if header.kind.as_deref() != Some(kind) {
        return Err(IndexError::WrongKind { expected: kind });
    }
    serde_json::from_slice(bytes).map_err(|error| IndexError::Malformed(error.to_string()))
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::SigningKey;

    use super::*;
    use crate::signing::sign_detached;

    fn release(version: &str, bevy: &str) -> Release {
        let channel = if version::is_nightly(version) {
            Channel::BleedingEdge
        } else {
            Channel::Stable
        };
        Release {
            version: version.to_string(),
            channel,
            bevy: bevy.to_string(),
            bevy_rev: None,
            jackdaw_rev: None,
            date: version
                .strip_prefix("nightly-")
                .unwrap_or("2026-10-01")
                .to_string(),
            rustc: None,
            sdk_id: None,
            notes_url: None,
            yanked: false,
            broken: false,
            known_good: false,
            known_issues: Vec::new(),
            artifacts: vec![Artifact {
                target: "x86_64-unknown-linux-gnu".into(),
                url: format!("https://example.com/{version}.tar.zst"),
                sha256: "00".repeat(32),
                size: 1,
                installed_size: None,
                attestation: None,
            }],
            modules_index: None,
        }
    }

    fn index(releases: &[Release]) -> ReleaseIndex {
        let mut index = ReleaseIndex::default();
        for release in releases {
            index.add_release(release.clone(), 0);
        }
        index
    }

    #[test]
    fn the_documented_index_example_parses() {
        let text = r#"{
          "schema": 1,
          "kind": "releases",
          "serial": 12,
          "generated": "2026-10-11T06:00:00Z",
          "channels": {
            "stable": { "lines": [
              { "bevy": "0.20", "latest": "0.20.1", "status": "current" },
              { "bevy": "0.19", "latest": "0.19.0", "status": "previous" }
            ] },
            "bleeding-edge": { "latest": "nightly-2026-10-11", "retain_days": 30 }
          },
          "releases": [
            { "version": "0.20.1", "channel": "stable", "bevy": "0.20", "bevy_rev": null,
              "date": "2026-10-20", "rustc": "1.91.0", "sdk_id": "jackdaw-0.20.1",
              "notes_url": "https://example.com/notes", "yanked": false,
              "artifacts": [ { "target": "x86_64-unknown-linux-gnu",
                "url": "https://example.com/a.tar.zst", "sha256": "ab", "size": 186000000,
                "installed_size": 540000000 } ],
              "modules_index": "https://example.com/modules.json" },
            { "version": "nightly-2026-10-11", "channel": "bleeding-edge", "bevy": "main",
              "bevy_rev": "9be004d", "jackdaw_rev": "b513241", "date": "2026-10-11",
              "ci": { "status": "green" }, "known_issues": ["Terrain brush crashes on Metal"],
              "artifacts": [] }
          ]
        }"#;
        let index = ReleaseIndex::parse(text.as_bytes()).unwrap();
        assert_eq!(index.releases.len(), 2);
        assert_eq!(index.resolve("stable").unwrap().version, "0.20.1");
        assert_eq!(
            index.resolve("bleeding-edge").unwrap().known_issues,
            ["Terrain brush crashes on Metal"]
        );
        assert_eq!(
            index.releases[0].artifacts[0].installed_size,
            Some(540_000_000)
        );
    }

    #[test]
    fn a_newer_schema_is_reported_rather_than_misread() {
        assert_eq!(
            ReleaseIndex::parse(br#"{"schema": 2, "releases": "changed"}"#),
            Err(IndexError::NewerSchema(2))
        );
    }

    #[test]
    fn a_modules_list_is_not_accepted_as_a_release_index() {
        let modules = br#"{"schema": 1, "kind": "modules", "editor": "0.20.1"}"#;
        assert_eq!(
            ReleaseIndex::parse(modules),
            Err(IndexError::WrongKind {
                expected: RELEASES_KIND
            })
        );
        assert_eq!(
            ReleaseIndex::parse(br#"{"schema": 1}"#),
            Err(IndexError::WrongKind {
                expected: RELEASES_KIND
            })
        );
    }

    #[test]
    fn a_signed_index_is_only_accepted_with_its_signature() {
        let key = SigningKey::from_bytes(&[3; 32]);
        let bytes = serde_json::to_vec(&index(&[release("0.20.1", "0.20")])).unwrap();
        let signature = sign_detached(&key, &bytes);
        let public = key.verifying_key();
        assert!(ReleaseIndex::from_signed(&bytes, &signature, &public).is_ok());

        let mut altered = bytes.clone();
        let position = altered.iter().position(|&byte| byte == b'1').unwrap();
        altered[position] = b'2';
        assert!(matches!(
            ReleaseIndex::from_signed(&altered, &signature, &public),
            Err(IndexError::Signature(_))
        ));
    }

    #[test]
    fn line_queries_pick_the_newest_patch_and_skip_yanked() {
        let mut yanked = release("0.20.3", "0.20");
        yanked.yanked = true;
        let index = index(&[
            release("0.20.1", "0.20"),
            release("0.20.2", "0.20"),
            yanked,
            release("0.19.4", "0.19"),
        ]);
        assert_eq!(index.resolve("0.20").unwrap().version, "0.20.2");
        assert_eq!(index.resolve("0.19").unwrap().version, "0.19.4");
        assert_eq!(index.resolve("v0.20.1").unwrap().version, "0.20.1");
        assert!(index.resolve("0.20.3").unwrap_err().contains("withdrawn"));
        assert!(index.resolve("0.18").is_err());
    }

    #[test]
    fn stable_is_the_current_line_and_nightly_skips_broken_builds() {
        let mut broken = release("nightly-2026-10-12", "main");
        broken.broken = true;
        let index = index(&[
            release("0.19.4", "0.19"),
            release("0.20.0", "0.20"),
            release("nightly-2026-10-11", "main"),
            broken,
        ]);
        assert_eq!(index.resolve("stable").unwrap().version, "0.20.0");
        assert_eq!(
            index.resolve("bleeding-edge").unwrap().version,
            "nightly-2026-10-11"
        );
        assert_eq!(
            index.channels.bleeding_edge.latest.as_deref(),
            Some("nightly-2026-10-11")
        );
        let statuses: Vec<_> = index
            .channels
            .stable
            .lines
            .iter()
            .map(|line| (line.bevy.as_str(), line.status))
            .collect();
        assert_eq!(
            statuses,
            [
                ("0.20", LineStatus::Current),
                ("0.19", LineStatus::Previous)
            ]
        );
    }

    #[test]
    fn adding_a_release_prunes_old_nightlies_but_keeps_known_good_ones() {
        let day = |date: &str| version::parse_date(date).unwrap();
        let mut known_good = release("nightly-2026-08-01", "main");
        known_good.known_good = true;
        let mut index = ReleaseIndex::default();
        for release in [
            release("0.20.0", "0.20"),
            release("nightly-2026-08-02", "main"),
            known_good,
            release("nightly-2026-09-20", "main"),
        ] {
            index.add_release(release, day("2026-09-20"));
        }
        index.add_release(release("nightly-2026-10-11", "main"), day("2026-10-11"));
        let versions: Vec<_> = index
            .releases
            .iter()
            .map(|release| release.version.as_str())
            .collect();
        assert_eq!(
            versions,
            [
                "nightly-2026-10-11",
                "nightly-2026-09-20",
                "nightly-2026-08-01",
                "0.20.0"
            ]
        );
    }

    #[test]
    fn readding_a_version_replaces_it() {
        let mut index = index(&[release("0.20.0", "0.20")]);
        let mut fixed = release("0.20.0", "0.20");
        fixed.notes_url = Some("https://example.com".into());
        index.add_release(fixed, 0);
        assert_eq!(index.releases.len(), 1);
        assert!(index.releases[0].notes_url.is_some());
    }

    #[test]
    fn modules_are_found_by_short_name_or_id() {
        let modules: ModulesIndex = serde_json::from_str(
            r#"{"schema":1,"kind":"modules","editor":"0.20.1","modules":[
                {"id":"jackdaw.physics","label":"Physics","version":"0.20.1","artifacts":[]}
            ]}"#,
        )
        .unwrap();
        assert!(modules.module("physics").is_some());
        assert!(modules.module("jackdaw.physics").is_some());
        assert!(modules.module("navmesh").is_none());
    }
}
