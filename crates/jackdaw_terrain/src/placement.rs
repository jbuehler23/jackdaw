//! Scatter placements stored on the terrain rather than as entities.
//!
//! A [`crate::scatter::Placement`] is what the kernel produces; a
//! [`ScatterPlacement`] is what a terrain keeps. The difference is
//! ownership: the kernel's placement is a value in a caller's list, while
//! a stored one belongs to a region and is addressed by that region's
//! coordinate and its index within it.
//!
//! Two side tables make a placement small. The [`ScatterPalette`] holds
//! each asset once, and the group table holds each stamp identity once, so
//! a placement is four u16-or-f32 fields and two indices rather than two
//! strings. Both tables are append-only within a document: an index is
//! what every placement refers to, so an entry is emptied rather than
//! removed.

use bevy_math::Vec3;

/// Extensions a palette entry may name.
const MODEL_EXTENSIONS: [&str; 2] = [".gltf", ".glb"];

/// Most rows either side table may hold: a placement names a row by `u16`,
/// so an index past this one could not be stored.
pub const MAX_SCATTER_TABLE: usize = u16::MAX as usize + 1;

/// Longest a stamp identity may be. A key is an editor-facing name, not a
/// payload, and an unbounded one would be written into every sidecar that
/// carried a placement of it.
pub const MAX_SCATTER_GROUP_LEN: usize = 128;

/// Why a palette entry could not name an asset.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScatterAssetError {
    /// The path is empty.
    Empty,
    /// The path is absolute, carries a platform prefix, or holds a
    /// component that is not a plain name, so it does not address a file
    /// beneath the assets directory.
    NotAssetsRelative,
    /// The path does not end in `.gltf` or `.glb`.
    NotAModel,
    /// The palette already holds every index a placement could name.
    TableFull,
}

impl core::fmt::Display for ScatterAssetError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Empty => write!(f, "scatter palette entry is empty"),
            Self::NotAssetsRelative => write!(
                f,
                "scatter palette entry must be a forward-slash path relative to the assets directory"
            ),
            Self::NotAModel => write!(f, "scatter palette entry must end in .gltf or .glb"),
            Self::TableFull => write!(
                f,
                "scatter palette already holds {} assets, which is every index a placement can name",
                MAX_SCATTER_TABLE
            ),
        }
    }
}

impl core::error::Error for ScatterAssetError {}

/// Why a stamp identity could not be stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScatterGroupError {
    /// The empty key names nothing, so no placement could be found by it.
    Empty,
    /// Longer than [`MAX_SCATTER_GROUP_LEN`].
    TooLong,
    /// A control character, which no editor field can round-trip.
    InvalidCharacter,
    /// The group table already holds every index a placement could name.
    TableFull,
}

impl core::fmt::Display for ScatterGroupError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Empty => write!(f, "scatter group key is empty"),
            Self::TooLong => write!(
                f,
                "scatter group key is longer than {MAX_SCATTER_GROUP_LEN} bytes"
            ),
            Self::InvalidCharacter => {
                write!(f, "scatter group key holds a control character")
            }
            Self::TableFull => write!(
                f,
                "scatter group table already holds {MAX_SCATTER_TABLE} keys, which is every index a placement can name"
            ),
        }
    }
}

impl core::error::Error for ScatterGroupError {}

/// Validate a palette entry's asset reference.
///
/// Stricter than the host platform, and for the same reason the sidecar's
/// other path fields are: a document is read on machines other than the
/// one that wrote it, so a backslash, a drive prefix and a `..` component
/// are rejected everywhere rather than wherever they happen to escape.
///
/// The extension is required because the renderer resolves an entry
/// through the glTF loader and has nothing to fall back on for a path
/// that loader will not accept.
pub fn validate_scatter_asset(path: &str) -> Result<(), ScatterAssetError> {
    if path.is_empty() {
        return Err(ScatterAssetError::Empty);
    }
    if path.contains('\\') || path.starts_with('/') || path.contains(':') {
        return Err(ScatterAssetError::NotAssetsRelative);
    }
    if path
        .split('/')
        .any(|part| matches!(part, "" | "." | "..") || part.contains('\0'))
    {
        return Err(ScatterAssetError::NotAssetsRelative);
    }
    let lowered = path.to_ascii_lowercase();
    if !MODEL_EXTENSIONS.iter().any(|ext| lowered.ends_with(ext)) {
        return Err(ScatterAssetError::NotAModel);
    }
    Ok(())
}

/// Validate a stamp identity.
pub fn validate_scatter_group(key: &str) -> Result<(), ScatterGroupError> {
    if key.is_empty() {
        return Err(ScatterGroupError::Empty);
    }
    if key.len() > MAX_SCATTER_GROUP_LEN {
        return Err(ScatterGroupError::TooLong);
    }
    if key.chars().any(char::is_control) {
        return Err(ScatterGroupError::InvalidCharacter);
    }
    Ok(())
}

/// One asset a terrain's placements may name, with what the renderer and
/// the navmesh bake need to know about it.
#[derive(Clone, Debug, PartialEq)]
pub struct ScatterPaletteEntry {
    /// Path to a `.gltf` or `.glb`, relative to the assets directory.
    /// Empty for a tombstone: an index held open by a removed entry.
    pub asset: String,
    /// Whether a placement of this asset blocks an agent, and so
    /// contributes geometry to a navmesh bake. Ground cover is false; a
    /// tree is true.
    pub obstacle: bool,
    /// Distance in world units past which placements of this asset stop
    /// drawing. Zero draws at every distance.
    pub cull_distance: f32,
}

impl ScatterPaletteEntry {
    /// An entry that blocks agents and draws at every distance.
    pub fn new(asset: impl Into<String>) -> Self {
        Self {
            asset: asset.into(),
            obstacle: true,
            cull_distance: 0.0,
        }
    }

    /// An index held open by a removed entry: placements still name it, so
    /// it keeps its position and draws nothing.
    pub fn tombstone() -> Self {
        Self {
            asset: String::new(),
            obstacle: false,
            cull_distance: 0.0,
        }
    }

    /// Whether this index holds no asset.
    pub fn is_tombstone(&self) -> bool {
        self.asset.is_empty()
    }

    /// Clamp `cull_distance` into the range the renderer compares against:
    /// a non-finite or negative value reads as "no cutoff", which is what
    /// a caller that never set one meant.
    pub(crate) fn sanitize(&mut self) {
        if !self.cull_distance.is_finite() || self.cull_distance < 0.0 {
            self.cull_distance = 0.0;
        }
    }
}

/// The assets and stamp identities a document's placements refer to.
///
/// Both tables are indexed by the `u16` a placement carries, so an index
/// never changes meaning while placements naming it exist.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScatterPalette {
    /// Assets in index order.
    pub assets: Vec<ScatterPaletteEntry>,
    /// Stamp identities in index order. Empty for a tombstone.
    pub groups: Vec<String>,
}

impl ScatterPalette {
    /// Whether no placement could name anything: both tables empty.
    pub fn is_empty(&self) -> bool {
        self.assets.is_empty() && self.groups.is_empty()
    }

    /// Index of `asset`, appending an entry when it is not there yet.
    ///
    /// An existing entry is returned as it stands, so re-running a stamp
    /// over the same assets reuses the rows it wrote last time rather than
    /// growing the table.
    pub fn intern_asset(&mut self, asset: &str) -> Result<u16, ScatterAssetError> {
        if let Some(at) = self.assets.iter().position(|e| e.asset == asset) {
            return Ok(at as u16);
        }
        if self.assets.len() >= MAX_SCATTER_TABLE {
            return Err(ScatterAssetError::TableFull);
        }
        self.assets.push(ScatterPaletteEntry::new(asset));
        Ok((self.assets.len() - 1) as u16)
    }

    /// Index of `key`, reviving a tombstoned row or appending one when the
    /// key is not there yet.
    ///
    /// A tombstoned row is free to take because the only thing that
    /// tombstones one takes that group's placements with it, so no
    /// placement names the index by the time it is handed out again.
    pub fn intern_group(&mut self, key: &str) -> Result<u16, ScatterGroupError> {
        if let Some(at) = self.groups.iter().position(|g| g == key) {
            return Ok(at as u16);
        }
        if let Some(at) = self.groups.iter().position(String::is_empty) {
            self.groups[at] = key.to_string();
            return Ok(at as u16);
        }
        if self.groups.len() >= MAX_SCATTER_TABLE {
            return Err(ScatterGroupError::TableFull);
        }
        self.groups.push(key.to_string());
        Ok((self.groups.len() - 1) as u16)
    }

    /// Index of `key`, without adding it.
    pub fn group_index(&self, key: &str) -> Option<u16> {
        self.groups
            .iter()
            .position(|g| g == key)
            .map(|at| at as u16)
    }

    /// The asset at `index`, or `None` for an index past the table or a
    /// tombstone.
    pub fn asset(&self, index: u16) -> Option<&ScatterPaletteEntry> {
        self.assets
            .get(index as usize)
            .filter(|entry| !entry.is_tombstone())
    }

    /// The stamp identity at `index`, or `None` for an index past the
    /// table or a tombstone.
    pub fn group(&self, index: u16) -> Option<&str> {
        self.groups
            .get(index as usize)
            .map(String::as_str)
            .filter(|key| !key.is_empty())
    }
}

/// One instance a terrain carries, in the space of the region holding it.
///
/// Position is stored against the region's minimum corner rather than the
/// terrain's origin: that is the only anchor that stays put when the
/// terrain grows, and it keeps the floats small on a document that reaches
/// far from its origin.
///
/// Rotation is a yaw alone. A placement stands upright; anything tilted,
/// bent or hand-adjusted is an entity, not a stored placement.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScatterPlacement {
    /// Index into [`ScatterPalette::groups`].
    pub group: u16,
    /// Index into [`ScatterPalette::assets`].
    pub asset: u16,
    /// Offset from the region's minimum corner, in world units.
    pub x: f32,
    /// Height in the terrain's local space.
    pub y: f32,
    /// Offset from the region's minimum corner, in world units.
    pub z: f32,
    /// Rotation about Y, in radians.
    pub yaw: f32,
    /// Uniform scale.
    pub scale: f32,
}

impl ScatterPlacement {
    /// Offset from the region's minimum corner, as a vector.
    pub fn offset(&self) -> Vec3 {
        Vec3::new(self.x, self.y, self.z)
    }

    /// Clamp the floats into a range the renderer and the bake can use.
    ///
    /// A non-finite coordinate reaching the render world becomes a NaN
    /// bounding volume, which no frustum test rejects and every batch then
    /// draws.
    pub(crate) fn sanitize(&mut self) {
        for value in [&mut self.x, &mut self.y, &mut self.z, &mut self.yaw] {
            if !value.is_finite() {
                *value = 0.0;
            }
        }
        if !self.scale.is_finite() || self.scale <= 0.0 {
            self.scale = 1.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_palette_entry_names_a_model_below_the_assets_directory() {
        assert!(validate_scatter_asset("models/kit/tree.gltf").is_ok());
        assert!(validate_scatter_asset("tree.glb").is_ok());
        assert!(validate_scatter_asset("models/Tree.GLB").is_ok());
    }

    #[test]
    fn a_palette_entry_refuses_a_path_that_leaves_the_assets_directory() {
        for path in [
            "/abs/tree.gltf",
            "../tree.gltf",
            "models/../../tree.gltf",
            "models\\tree.gltf",
            "C:/models/tree.gltf",
            "models//tree.gltf",
        ] {
            assert_eq!(
                validate_scatter_asset(path),
                Err(ScatterAssetError::NotAssetsRelative),
                "{path}"
            );
        }
    }

    #[test]
    fn a_palette_entry_refuses_a_file_the_gltf_loader_would_not_take() {
        assert_eq!(validate_scatter_asset(""), Err(ScatterAssetError::Empty));
        assert_eq!(
            validate_scatter_asset("models/tree.obj"),
            Err(ScatterAssetError::NotAModel)
        );
        assert_eq!(
            validate_scatter_asset("models/tree"),
            Err(ScatterAssetError::NotAModel)
        );
    }

    #[test]
    fn a_group_key_refuses_what_no_field_could_show() {
        assert!(validate_scatter_group("Ashwood deep").is_ok());
        assert_eq!(validate_scatter_group(""), Err(ScatterGroupError::Empty));
        assert_eq!(
            validate_scatter_group("bad\nkey"),
            Err(ScatterGroupError::InvalidCharacter)
        );
        assert_eq!(
            validate_scatter_group(&"k".repeat(MAX_SCATTER_GROUP_LEN + 1)),
            Err(ScatterGroupError::TooLong)
        );
    }

    #[test]
    fn interning_returns_the_same_index_for_the_same_string() {
        let mut palette = ScatterPalette::default();
        assert_eq!(palette.intern_asset("a.gltf"), Ok(0));
        assert_eq!(palette.intern_asset("b.gltf"), Ok(1));
        assert_eq!(palette.intern_asset("a.gltf"), Ok(0));
        assert_eq!(palette.intern_group("one"), Ok(0));
        assert_eq!(palette.intern_group("one"), Ok(0));
        assert_eq!(palette.group_index("two"), None);
    }

    #[test]
    fn interning_a_key_again_takes_the_row_a_tombstone_left_open() {
        let mut palette = ScatterPalette::default();
        palette.intern_group("one").expect("the table is empty");
        palette.intern_group("two").expect("the table has room");
        palette.groups[0].clear();
        assert_eq!(palette.intern_group("three"), Ok(0));
        assert_eq!(palette.groups.len(), 2);
    }

    #[test]
    fn a_table_with_no_index_left_refuses_a_new_entry() {
        let mut palette = ScatterPalette {
            groups: (0..MAX_SCATTER_TABLE).map(|at| format!("g{at}")).collect(),
            assets: (0..MAX_SCATTER_TABLE)
                .map(|at| ScatterPaletteEntry::new(format!("a{at}.glb")))
                .collect(),
        };
        assert_eq!(
            palette.intern_group("one more"),
            Err(ScatterGroupError::TableFull)
        );
        assert_eq!(
            palette.intern_asset("one-more.glb"),
            Err(ScatterAssetError::TableFull)
        );
        assert_eq!(palette.group_index("g0"), Some(0));
    }

    #[test]
    fn a_tombstoned_index_resolves_to_nothing_but_keeps_its_position() {
        let mut palette = ScatterPalette::default();
        palette.intern_asset("a.gltf").expect("the table is empty");
        palette.intern_asset("b.gltf").expect("the table has room");
        palette.assets[0] = ScatterPaletteEntry::tombstone();
        assert!(palette.asset(0).is_none());
        assert_eq!(palette.asset(1).map(|e| e.asset.as_str()), Some("b.gltf"));
    }

    #[test]
    fn a_placement_with_an_unusable_float_sanitizes_to_a_drawable_one() {
        let mut placement = ScatterPlacement {
            group: 0,
            asset: 0,
            x: f32::NAN,
            y: 1.0,
            z: f32::INFINITY,
            yaw: f32::NAN,
            scale: 0.0,
        };
        placement.sanitize();
        assert_eq!(placement.offset(), Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(placement.yaw, 0.0);
        assert_eq!(placement.scale, 1.0);
    }
}
