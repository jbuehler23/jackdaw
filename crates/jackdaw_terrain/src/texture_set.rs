//! The texture set a terrain's splat material paints with: an ordered list
//! of texture entries, addressed by the base/overlay ids packed into a
//! [`crate::control::Control`] word.
//!
//! Derived, never authored. A terrain persists an ordered list of material
//! names (see [`crate::sidecar::TerrainMaterialSlot`]); the editor resolves
//! each one into an entry here and hands the result to the array builder.
//! Nothing outside the editor writes this type.
//!
//! Entry order is the id space: entry 0 is texture id 0, which is what an
//! unpainted cell (control word 0) draws. A set is capped at
//! [`MAX_TEXTURES`] entries, half of what the 5-bit id field can address.
//!
//! Plain data with no engine types in it, so it compiles in a
//! `--no-default-features` build alongside the sidecar and the mesher.

/// Most entries a texture set may declare.
///
/// The control word's id fields are 5 bits (0..=31); sets are capped at 16
/// so the array-layer budget has headroom to grow without a format change.
pub const MAX_TEXTURES: usize = 16;

/// Texture repeats per world unit a slot uses when it does not say.
///
/// One repeat per 10 world units, the conventional default for terrain
/// splatting: a 1 m grass texture tiling every metre reads as noise at
/// any distance.
pub const DEFAULT_UV_SCALE: f32 = 0.1;

/// Smallest `uv_scale` a slot may declare. Zero or negative collapses or
/// mirrors the texture.
pub const MIN_UV_SCALE: f32 = 0.001;
/// Largest `uv_scale` a slot may declare. Past this a texture aliases to
/// hash noise at any viewing distance.
pub const MAX_UV_SCALE: f32 = 100.0;

/// Detiling strength a slot uses when it does not say: off.
///
/// At 0 the shader samples the UVs the slot's scale alone puts it at.
pub const DEFAULT_DETILE: f32 = 0.0;

/// Strongest detiling a slot may declare. Past a full turn of rotation
/// and a whole tile of offset there is nothing further to break up.
pub const MAX_DETILE: f32 = 1.0;

/// One texture id in a set, resolved from the material the terrain slot
/// names.
///
/// Paths are project-relative, resolved by the asset system that loads the
/// material. Albedo is sampled as sRGB; normal and height are linear data.
#[derive(Clone, Debug, PartialEq)]
pub struct TextureSetEntry {
    /// Material name this entry was resolved from. Errors name this
    /// rather than only a file.
    pub material: String,
    /// Albedo path, or `None` when the material has no base colour texture
    /// or could not be resolved at all. A slot with no albedo still holds
    /// its id and draws the fallback layer: dropping it would renumber
    /// every id painted after it.
    pub albedo: Option<String>,
    /// Tangent-space normal map, or `None` to shade this entry flat.
    pub normal: Option<String>,
    /// The normal map is authored with green pointing down. The array
    /// builder flips the green channel so the shader sees one convention.
    pub flip_normal_y: bool,
    /// Grayscale height map read from the red channel, or `None`. Height
    /// is what sharpens a blend into an interlocking transition rather
    /// than a cross-fade; an entry without one blends on even terms.
    pub height: Option<String>,
    /// Texture repeats per world unit. Lives on the terrain slot, not on
    /// the material: one material tiles differently per surface.
    pub uv_scale: f32,
    /// How hard to break up this entry's repetition, `0..1`. 0 is off.
    pub detile: f32,
}

impl TextureSetEntry {
    /// An id held open by a removed material: it draws the fallback and
    /// nothing else in the set moves. See
    /// [`crate::sidecar::TerrainMaterialSlot::tombstone`].
    ///
    /// Carries the default UV scale rather than the tombstone's own zero:
    /// nothing samples it, but the shader still divides by whatever
    /// reaches its uniform.
    pub fn vacant() -> Self {
        Self::unresolved(String::new(), DEFAULT_UV_SCALE)
    }

    /// Whether this entry has no material behind it at all: a vacated id,
    /// as opposed to one whose material failed to resolve.
    pub fn is_vacant(&self) -> bool {
        self.material.is_empty()
    }

    /// An entry with only an albedo, at the default UV scale.
    pub fn new(material: impl Into<String>, albedo: impl Into<String>) -> Self {
        Self {
            material: material.into(),
            albedo: Some(albedo.into()),
            normal: None,
            flip_normal_y: false,
            height: None,
            uv_scale: DEFAULT_UV_SCALE,
            detile: DEFAULT_DETILE,
        }
    }

    /// An entry whose material resolved to nothing: it keeps its id and
    /// draws the fallback layer.
    pub fn unresolved(material: impl Into<String>, uv_scale: f32) -> Self {
        Self {
            material: material.into(),
            albedo: None,
            normal: None,
            flip_normal_y: false,
            height: None,
            uv_scale,
            detile: DEFAULT_DETILE,
        }
    }

    /// Albedo, normal and height paths, skipping the ones this entry does
    /// not have.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        [
            self.albedo.as_ref(),
            self.normal.as_ref(),
            self.height.as_ref(),
        ]
        .into_iter()
        .flatten()
        .map(String::as_str)
    }
}

impl Default for TextureSetEntry {
    fn default() -> Self {
        Self::unresolved(String::new(), DEFAULT_UV_SCALE)
    }
}

/// A terrain's texture set, in id order.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextureSet {
    /// Entries in id order. Index is the texture id a control word names.
    pub entries: Vec<TextureSetEntry>,
}

/// Why a texture set could not be used.
///
/// Every variant names the material it concerns, so a set of sixteen can
/// be narrowed to the one at fault.
#[derive(Clone, Debug, PartialEq)]
pub enum TextureSetError {
    /// A set with no entries has no texture 0, so nothing can draw.
    Empty,
    /// More entries than [`MAX_TEXTURES`].
    TooManyEntries(usize),
    /// A path was absolute or contained `..`.
    UnsafePath {
        entry: usize,
        material: String,
        path: String,
    },
    /// A `uv_scale` outside [`MIN_UV_SCALE`]..=[`MAX_UV_SCALE`], or NaN.
    BadUvScale {
        entry: usize,
        material: String,
        uv_scale: f32,
    },
    /// A detiling strength outside `0..=`[`MAX_DETILE`], or NaN.
    BadDetile {
        entry: usize,
        material: String,
        detile: f32,
    },
    /// Two slots name the same material. A slot is one texture id, so
    /// this is one material using two ids and two array layers. Vacated
    /// ids are exempt: they name nothing.
    DuplicateMaterial {
        entry: usize,
        material: String,
        first: usize,
    },
    /// Texture arrays require every layer to share one size, so one
    /// mismatched file makes the whole set unusable.
    MismatchedSize {
        entry: usize,
        material: String,
        path: String,
        found: (u32, u32),
        expected: (u32, u32),
    },
}

impl core::fmt::Display for TextureSetError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Empty => write!(f, "this terrain has no materials; add at least one"),
            Self::TooManyEntries(n) => {
                write!(
                    f,
                    "this terrain has {n} materials; the limit is {MAX_TEXTURES}"
                )
            }
            Self::UnsafePath {
                entry,
                material,
                path,
            } => write!(
                f,
                "material '{material}' (slot {entry}) uses path '{path}'; \
                 it must be a relative path inside the project"
            ),
            Self::BadUvScale {
                entry,
                material,
                uv_scale,
            } => write!(
                f,
                "material '{material}' (slot {entry}) has uv_scale {uv_scale}, outside \
                 {MIN_UV_SCALE}..={MAX_UV_SCALE}"
            ),
            Self::BadDetile {
                entry,
                material,
                detile,
            } => write!(
                f,
                "material '{material}' (slot {entry}) has detiling {detile}, outside \
                 0..={MAX_DETILE}"
            ),
            Self::DuplicateMaterial {
                entry,
                material,
                first,
            } => write!(
                f,
                "slots {first} and {entry} both use material '{material}'; \
                 each slot is one texture id and needs its own material"
            ),
            Self::MismatchedSize {
                entry,
                material,
                path,
                found,
                expected,
            } => write!(
                f,
                "material '{material}' (slot {entry}) texture '{path}' is {}x{}, but this \
                 terrain's textures are {}x{}; every texture must be the same size",
                found.0, found.1, expected.0, expected.1
            ),
        }
    }
}

impl core::error::Error for TextureSetError {}

impl TextureSet {
    /// Check everything that can be checked without opening the texture
    /// files: entry count, paths, UV scales, repeated materials.
    ///
    /// Size agreement needs the decoded images and is checked by
    /// [`check_layer_sizes`] once they load.
    pub fn validate(&self) -> Result<(), TextureSetError> {
        if self.entries.is_empty() {
            return Err(TextureSetError::Empty);
        }
        if self.entries.len() > MAX_TEXTURES {
            return Err(TextureSetError::TooManyEntries(self.entries.len()));
        }
        for (entry, e) in self.entries.iter().enumerate() {
            for path in e.paths() {
                if crate::sidecar::validate_asset_ref(path).is_err() {
                    return Err(TextureSetError::UnsafePath {
                        entry,
                        material: e.material.clone(),
                        path: path.to_string(),
                    });
                }
            }
            if !e.uv_scale.is_finite() || !(MIN_UV_SCALE..=MAX_UV_SCALE).contains(&e.uv_scale) {
                return Err(TextureSetError::BadUvScale {
                    entry,
                    material: e.material.clone(),
                    uv_scale: e.uv_scale,
                });
            }
            if !e.detile.is_finite() || !(0.0..=MAX_DETILE).contains(&e.detile) {
                return Err(TextureSetError::BadDetile {
                    entry,
                    material: e.material.clone(),
                    detile: e.detile,
                });
            }
            // Vacated ids share the empty name by construction, so only
            // real materials can collide.
            if e.is_vacant() {
                continue;
            }
            if let Some(first) = self.entries[..entry]
                .iter()
                .position(|other| other.material == e.material)
            {
                return Err(TextureSetError::DuplicateMaterial {
                    entry,
                    material: e.material.clone(),
                    first,
                });
            }
        }
        Ok(())
    }

    /// Number of texture ids this set defines.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Per-id UV scales, padded to [`MAX_TEXTURES`] with
    /// [`DEFAULT_UV_SCALE`] so a control word naming an id past the end of
    /// the set still produces a finite UV rather than a divide by zero.
    pub fn uv_scales(&self) -> [f32; MAX_TEXTURES] {
        let mut scales = [DEFAULT_UV_SCALE; MAX_TEXTURES];
        for (slot, entry) in scales.iter_mut().zip(&self.entries) {
            *slot = entry.uv_scale;
        }
        scales
    }

    /// Per-id detiling strengths, padded with [`DEFAULT_DETILE`] so an id
    /// past the end of the set draws with detiling off.
    pub fn detile_strengths(&self) -> [f32; MAX_TEXTURES] {
        let mut strengths = [DEFAULT_DETILE; MAX_TEXTURES];
        for (slot, entry) in strengths.iter_mut().zip(&self.entries) {
            *slot = entry.detile;
        }
        strengths
    }
}

/// Agree the layer sizes of a texture array, or name the first file that
/// disagrees.
///
/// The first layer sets the expected size, so the error points at the odd
/// one out rather than at layer 0. Callers pass the sizes they decoded;
/// nothing here opens a file.
pub fn check_layer_sizes(
    layers: &[(usize, &str, &str, (u32, u32))],
) -> Result<(u32, u32), TextureSetError> {
    let Some((_, _, _, expected)) = layers.first().copied() else {
        return Err(TextureSetError::Empty);
    };
    for (entry, material, path, found) in layers.iter().copied() {
        if found != expected {
            return Err(TextureSetError::MismatchedSize {
                entry,
                material: material.to_string(),
                path: path.to_string(),
                found,
                expected,
            });
        }
    }
    Ok(expected)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(entries: Vec<TextureSetEntry>) -> TextureSet {
        TextureSet { entries }
    }

    #[test]
    fn a_minimal_single_entry_set_validates() {
        assert_eq!(
            set(vec![TextureSetEntry::new("grass", "a.png")]).validate(),
            Ok(())
        );
    }

    #[test]
    fn an_empty_set_is_rejected() {
        assert_eq!(set(vec![]).validate(), Err(TextureSetError::Empty));
    }

    #[test]
    fn a_set_at_the_entry_limit_validates_and_one_past_it_does_not() {
        let at_limit: Vec<_> = (0..MAX_TEXTURES)
            .map(|i| TextureSetEntry::new(format!("m{i}"), format!("t{i}.png")))
            .collect();
        assert_eq!(set(at_limit).validate(), Ok(()));

        let over: Vec<_> = (0..MAX_TEXTURES + 1)
            .map(|i| TextureSetEntry::new(format!("m{i}"), format!("t{i}.png")))
            .collect();
        assert_eq!(
            set(over).validate(),
            Err(TextureSetError::TooManyEntries(MAX_TEXTURES + 1))
        );
    }

    /// A material with no file behind it keeps its id and validates: the
    /// ids painted after it must not shift because one material went
    /// missing.
    #[test]
    fn a_slot_whose_material_resolved_to_nothing_still_validates() {
        let built = set(vec![
            TextureSetEntry::new("grass", "grass.png"),
            TextureSetEntry::unresolved("gone", DEFAULT_UV_SCALE),
            TextureSetEntry::new("rock", "rock.png"),
        ]);
        assert_eq!(built.validate(), Ok(()));
        assert_eq!(built.entries[1].paths().count(), 0);
        assert_eq!(built.len(), 3, "the id space keeps its shape");
    }

    #[test]
    fn traversal_in_any_of_an_entrys_three_paths_is_rejected() {
        for build in [
            |p: &str| TextureSetEntry::new("bad", p),
            |p: &str| TextureSetEntry {
                normal: Some(p.to_string()),
                ..TextureSetEntry::new("bad", "ok.png")
            },
            |p: &str| TextureSetEntry {
                height: Some(p.to_string()),
                ..TextureSetEntry::new("bad", "ok.png")
            },
        ] {
            let built = set(vec![
                TextureSetEntry::new("first", "first.png"),
                build("../secret.png"),
            ]);
            assert_eq!(
                built.validate(),
                Err(TextureSetError::UnsafePath {
                    entry: 1,
                    material: "bad".to_string(),
                    path: "../secret.png".to_string(),
                })
            );
        }
    }

    #[test]
    fn an_absolute_path_is_rejected() {
        assert!(matches!(
            set(vec![TextureSetEntry::new("bad", "/etc/passwd")]).validate(),
            Err(TextureSetError::UnsafePath { entry: 0, .. })
        ));
    }

    #[test]
    fn uv_scale_is_rejected_outside_its_range_and_for_nan() {
        for bad in [0.0, -1.0, MAX_UV_SCALE * 2.0, f32::NAN, f32::INFINITY] {
            let built = set(vec![TextureSetEntry {
                uv_scale: bad,
                ..TextureSetEntry::new("grass", "a.png")
            }]);
            assert!(
                matches!(
                    built.validate(),
                    Err(TextureSetError::BadUvScale { entry: 0, .. })
                ),
                "uv_scale {bad} should be rejected"
            );
        }
    }

    #[test]
    fn uv_scale_is_accepted_at_both_ends_of_its_range() {
        for good in [MIN_UV_SCALE, DEFAULT_UV_SCALE, MAX_UV_SCALE] {
            let built = set(vec![TextureSetEntry {
                uv_scale: good,
                ..TextureSetEntry::new("grass", "a.png")
            }]);
            assert_eq!(built.validate(), Ok(()), "uv_scale {good} should pass");
        }
    }

    /// Two slots on one material waste an id and an array layer.
    #[test]
    fn two_slots_naming_the_same_material_are_rejected_and_both_are_named() {
        let built = set(vec![
            TextureSetEntry::new("grass", "grass.png"),
            TextureSetEntry::new("rock", "rock.png"),
            TextureSetEntry::new("grass", "grass.png"),
        ]);
        let err = built.validate().expect_err("repeated material");
        assert_eq!(
            err,
            TextureSetError::DuplicateMaterial {
                entry: 2,
                material: "grass".to_string(),
                first: 0,
            }
        );
        assert!(err.to_string().contains("grass"), "{err}");
    }

    /// Several ids can be vacant at once, and the empty name they share
    /// is not two materials colliding.
    #[test]
    fn vacated_ids_do_not_collide_with_each_other() {
        let built = set(vec![
            TextureSetEntry::new("grass", "grass.png"),
            TextureSetEntry::vacant(),
            TextureSetEntry::vacant(),
            TextureSetEntry::new("sand", "sand.png"),
        ]);
        assert_eq!(built.validate(), Ok(()));
        assert!(built.entries[1].is_vacant());
        assert!(!built.entries[3].is_vacant());
        assert_eq!(
            built.uv_scales()[1],
            DEFAULT_UV_SCALE,
            "a vacated id still needs a UV scale the shader can divide by",
        );
    }

    /// Two different materials may share a texture file: identity is the
    /// material, not the image it happens to bind.
    #[test]
    fn two_materials_may_share_a_texture_file() {
        let built = set(vec![
            TextureSetEntry {
                height: Some("shared_h.png".into()),
                ..TextureSetEntry::new("grass", "shared.png")
            },
            TextureSetEntry {
                height: Some("shared_h.png".into()),
                ..TextureSetEntry::new("meadow", "shared.png")
            },
        ]);
        assert_eq!(built.validate(), Ok(()));
    }

    #[test]
    fn uv_scales_pads_to_the_id_space_with_the_default() {
        let built = set(vec![
            TextureSetEntry {
                uv_scale: 0.5,
                ..TextureSetEntry::new("a", "a.png")
            },
            TextureSetEntry {
                uv_scale: 2.0,
                ..TextureSetEntry::new("b", "b.png")
            },
        ]);
        let scales = built.uv_scales();
        assert_eq!(scales[0], 0.5);
        assert_eq!(scales[1], 2.0);
        assert!(scales[2..].iter().all(|s| *s == DEFAULT_UV_SCALE));
    }

    #[test]
    fn an_entrys_paths_skip_the_maps_it_does_not_have() {
        let bare = TextureSetEntry::new("a", "a.png");
        assert_eq!(bare.paths().collect::<Vec<_>>(), vec!["a.png"]);
        let full = TextureSetEntry {
            normal: Some("n.png".into()),
            height: Some("h.png".into()),
            ..TextureSetEntry::new("a", "a.png")
        };
        assert_eq!(
            full.paths().collect::<Vec<_>>(),
            vec!["a.png", "n.png", "h.png"]
        );
    }

    #[test]
    fn matching_layer_sizes_report_the_shared_size() {
        let layers = [
            (0, "grass", "a.png", (256, 256)),
            (1, "rock", "b.png", (256, 256)),
        ];
        assert_eq!(check_layer_sizes(&layers), Ok((256, 256)));
    }

    #[test]
    fn a_mismatched_layer_names_the_material_not_the_first_entry() {
        let layers = [
            (0, "grass", "a.png", (256, 256)),
            (1, "rock", "b.png", (256, 256)),
            (2, "sand", "odd.png", (512, 256)),
        ];
        let err = check_layer_sizes(&layers).expect_err("mismatch");
        assert_eq!(
            err,
            TextureSetError::MismatchedSize {
                entry: 2,
                material: "sand".to_string(),
                path: "odd.png".to_string(),
                found: (512, 256),
                expected: (256, 256),
            }
        );
        let message = err.to_string();
        assert!(message.contains("sand"), "{message}");
        assert!(message.contains("odd.png"), "{message}");
        assert!(message.contains("512x256"), "{message}");
        assert!(message.contains("256x256"), "{message}");
    }
}
