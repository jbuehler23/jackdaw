//! Engine-agnostic PBR texture-set detection.
//!
//! Artists drop a folder of textures named by convention (`rock_albedo.png`,
//! `rock_normal.png`, `rock_roughness.png`) and expect them grouped into one
//! material. This crate does that grouping as pure string and regex math: it
//! parses each file name into a base name plus a role tag, buckets the files by
//! base name, and assigns each file to a logical [`TextureRole`]. There is no
//! filesystem access and no engine type here; the host walks the directory,
//! hands the path strings to [`group_texture_sets`], and binds the detected
//! [`MaterialSet`]s to its own material type.
//!
//! The roles are deliberately engine-free. The host decides how each role maps
//! onto its material fields and uses [`TextureRole::is_srgb`] to pick the right
//! color space, plus [`MaterialSet::recommended_scalars`] for the scalar
//! defaults a populated set implies.

/// Logical PBR texture role, independent of any engine's material type.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum TextureRole {
    BaseColor,
    Normal,
    MetallicRoughness,
    Emissive,
    Occlusion,
    Depth,
}

impl TextureRole {
    /// Whether textures in this role hold color data (load as sRGB). Base color
    /// and emissive are sRGB; data maps (normal, metallic-roughness, occlusion,
    /// depth) are linear.
    pub fn is_srgb(self) -> bool {
        matches!(self, TextureRole::BaseColor | TextureRole::Emissive)
    }
}

/// A detected PBR material: a base name and the source file path assigned to
/// each role (absent roles are `None`). Paths are whatever strings were passed
/// in (the editor passes absolute filesystem paths).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
#[non_exhaustive]
pub struct MaterialSet {
    pub base_name: String,
    pub base_color: Option<String>,
    pub normal: Option<String>,
    pub metallic_roughness: Option<String>,
    pub emissive: Option<String>,
    pub occlusion: Option<String>,
    pub depth: Option<String>,
    /// Whether the file bound to `metallic_roughness` carries metallic data:
    /// a metallic-tagged file, or a packed `orm`. A roughness-only pack
    /// binds a grayscale map whose blue channel is not metalness, so the
    /// metallic scalar must not scale it up.
    pub metallic_roughness_has_metallic: bool,
}

/// Sensible scalar defaults for a detected set, derived from which roles are
/// present. Mirrors the values an engine should apply when a metallic-roughness
/// or depth texture is bound.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PbrScalars {
    pub metallic: f32,
    pub perceptual_roughness: f32,
    pub parallax_depth_scale: f32,
    pub max_parallax_layer_count: f32,
}

impl MaterialSet {
    /// True when no role is populated (such a set should be discarded).
    pub fn is_empty(&self) -> bool {
        self.base_color.is_none()
            && self.normal.is_none()
            && self.metallic_roughness.is_none()
            && self.emissive.is_none()
            && self.occlusion.is_none()
            && self.depth.is_none()
    }

    /// Scalar defaults implied by the populated roles.
    ///
    /// A metallic-roughness texture multiplies the scalar values, so the
    /// roughness scalar defaults to 1.0 to use the texture as-is (otherwise
    /// 0.5). The metallic scalar only rises to 1.0 when the bound file carries
    /// metallic data ([`MaterialSet::metallic_roughness_has_metallic`]); a
    /// roughness-only map would otherwise read its grayscale blue channel as
    /// metalness. A depth map enables parallax with a gentle scale and layer
    /// cap (otherwise both zero, disabling it).
    pub fn recommended_scalars(&self) -> PbrScalars {
        let has_mr = self.metallic_roughness.is_some();
        let has_depth = self.depth.is_some();
        PbrScalars {
            metallic: if has_mr && self.metallic_roughness_has_metallic {
                1.0
            } else {
                0.0
            },
            perceptual_roughness: if has_mr { 1.0 } else { 0.5 },
            parallax_depth_scale: if has_depth { 0.05 } else { 0.0 },
            max_parallax_layer_count: if has_depth { 32.0 } else { 0.0 },
        }
    }
}

/// The compiled filename pattern. `None` only if the static pattern fails to
/// compile (it does not, in practice).
///
/// The pattern matches `<base><sep><tag><res>.<ext>` case-insensitively, where
/// the separator is `_`, `-`, `.`, or a space. Group 1 captures the base name
/// and group 2 captures the role tag. `<res>` is an optional resolution token
/// (packs commonly suffix resolution onto the tag) of the form `_1k`/`-2K` or
/// a bare pixel count like `_1024`; it is consumed but not captured, so it
/// never becomes part of the base name.
pub fn pbr_filename_regex() -> Option<regex::Regex> {
    let pattern = r"(?i)^(.+?)[_\-\.\s](diffuse|diff|albedo|base|col|color|basecolor|metallic|metalness|metal|mtl|roughness|rough|rgh|normal[_-]gl|normal[_-]dx|normalgl|normaldx|nor[_-]gl|nor[_-]dx|normal|nor|nrm|nrml|norm|orm|emission|emissive|emit|ao|ambient|occlusion|ambientocclusion|displacement|displace|disp|dsp|height|heightmap|alpha|opacity|specularity|specular|spec|spc|gloss|glossy|glossiness|bump|bmp|b|n)(?:[_-](?:\d+k|\d{3,5}))?\.(png|jpg|jpeg|ktx2|bmp|tga|webp)$";
    regex::Regex::new(pattern).ok()
}

/// Classify a filename tag (the captured `<tag>` group, case-insensitive) into a
/// texture role. Returns `None` for an unrecognized tag.
///
/// The `orm` tag is intentionally not classified here; it maps to two roles and
/// is handled inside [`group_texture_sets`]. `-` and `_` are interchangeable
/// inside a tag, so `normal-gl` and `normal_gl` classify the same.
pub fn classify_tag(tag: &str) -> Option<TextureRole> {
    match tag.to_lowercase().replace('-', "_").as_str() {
        "diffuse" | "diff" | "albedo" | "base" | "col" | "color" | "basecolor" | "b" => {
            Some(TextureRole::BaseColor)
        }
        "normalgl" | "normaldx" | "normal_gl" | "normal_dx" | "nor_gl" | "nor_dx" | "nor"
        | "nrm" | "nrml" | "norm" | "bump" | "bmp" | "n" | "normal" => Some(TextureRole::Normal),
        "metallic" | "metalness" | "metal" | "mtl" | "roughness" | "rough" | "rgh" => {
            Some(TextureRole::MetallicRoughness)
        }
        "emission" | "emissive" | "emit" => Some(TextureRole::Emissive),
        "ao" | "ambient" | "occlusion" | "ambientocclusion" => Some(TextureRole::Occlusion),
        "displacement" | "displace" | "disp" | "dsp" | "height" | "heightmap" => {
            Some(TextureRole::Depth)
        }
        _ => None,
    }
}

/// Which normal-map convention a tag names. `None` for a plain normal tag
/// (`normal`, `nor`, ...) that does not commit to either convention.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum NormalConvention {
    Gl,
    Dx,
}

fn normal_convention(tag: &str) -> Option<NormalConvention> {
    match tag.to_lowercase().replace('-', "_").as_str() {
        "normalgl" | "normal_gl" | "nor_gl" => Some(NormalConvention::Gl),
        "normaldx" | "normal_dx" | "nor_dx" => Some(NormalConvention::Dx),
        _ => None,
    }
}

fn is_roughness_tag(tag: &str) -> bool {
    matches!(tag.to_lowercase().as_str(), "roughness" | "rough" | "rgh")
}

fn is_metallic_tag(tag: &str) -> bool {
    matches!(
        tag.to_lowercase().as_str(),
        "metallic" | "metalness" | "metal" | "mtl"
    )
}

/// The normal map that should win the group's normal slot: the first GL- or
/// plain-tagged file in supply order, falling back to the first DX-tagged file
/// only when no GL or plain candidate exists. This makes the GL/DX preference
/// independent of supply order while leaving GL-vs-plain and plain-vs-plain
/// collisions first-seen-wins.
fn preferred_normal(files: &[(String, String)]) -> Option<String> {
    let non_dx = files.iter().find(|(tag, _)| {
        classify_tag(tag) == Some(TextureRole::Normal)
            && normal_convention(tag) != Some(NormalConvention::Dx)
    });
    if let Some((_, path)) = non_dx {
        return Some(path.clone());
    }
    files
        .iter()
        .find(|(tag, _)| normal_convention(tag) == Some(NormalConvention::Dx))
        .map(|(_, path)| path.clone())
}

/// The file that should win the group's metallic-roughness slot: the first
/// roughness-tagged file in supply order, falling back to the first
/// metallic-tagged file only when no roughness candidate exists. This makes
/// the roughness/metallic preference independent of supply order.
fn preferred_metallic_roughness(files: &[(String, String)]) -> Option<String> {
    let roughness = files.iter().find(|(tag, _)| is_roughness_tag(tag));
    if let Some((_, path)) = roughness {
        return Some(path.clone());
    }
    files
        .iter()
        .find(|(tag, _)| is_metallic_tag(tag))
        .map(|(_, path)| path.clone())
}

/// Group a list of file paths into detected material sets.
///
/// Each path's file name is matched against [`pbr_filename_regex`]; matches are
/// grouped by the captured base name (lowercased). Within a group, each file is
/// assigned to the role from [`classify_tag`], applying these rules: a
/// roughness tag always wins the metallic-roughness slot over a metallic tag,
/// regardless of supply order; a GL-convention normal always wins the normal
/// slot over a DX-convention one, regardless of supply order, with plain
/// normal tags and GL tags otherwise competing first-seen-wins; an `orm` file
/// fills the occlusion role when still empty and the metallic-roughness role
/// only when no explicit metallic or roughness tag is present in the group;
/// the first file seen for any other role wins. Empty sets are dropped. The
/// result is sorted by base name.
pub fn group_texture_sets(paths: &[String]) -> Vec<MaterialSet> {
    let Some(re) = pbr_filename_regex() else {
        return Vec::new();
    };

    // Preserve the per-base file order while bucketing, so "first seen wins"
    // matches the order paths were supplied in.
    let mut order: Vec<String> = Vec::new();
    let mut groups: std::collections::HashMap<String, Vec<(String, String)>> =
        std::collections::HashMap::new();

    for path in paths {
        let file_name = file_name_of(path);
        let Some(caps) = re.captures(&file_name) else {
            continue;
        };
        let base_name = caps[1].to_lowercase();
        let tag = caps[2].to_string();
        if !groups.contains_key(&base_name) {
            order.push(base_name.clone());
        }
        groups
            .entry(base_name)
            .or_default()
            .push((tag, path.clone()));
    }

    let mut results: Vec<MaterialSet> = Vec::new();
    for base_name in order {
        let files = &groups[&base_name];
        let mut set = MaterialSet {
            base_name: base_name.clone(),
            ..MaterialSet::default()
        };

        // Normal and metallic-roughness are resolved separately below so their
        // preferred tag wins regardless of where it falls in supply order;
        // this pass handles every other role plus the orm fallback.
        for (tag, path) in files {
            let tag_lower = tag.to_lowercase();
            if tag_lower == "orm" {
                if set.metallic_roughness.is_none() {
                    set.metallic_roughness = Some(path.clone());
                }
                if set.occlusion.is_none() {
                    set.occlusion = Some(path.clone());
                }
                continue;
            }

            let Some(role) = classify_tag(&tag_lower) else {
                continue;
            };
            let slot = match role {
                TextureRole::BaseColor => &mut set.base_color,
                TextureRole::Normal | TextureRole::MetallicRoughness => continue,
                TextureRole::Emissive => &mut set.emissive,
                TextureRole::Occlusion => &mut set.occlusion,
                TextureRole::Depth => &mut set.depth,
            };
            if slot.is_none() {
                *slot = Some(path.clone());
            }
        }

        set.normal = preferred_normal(files);
        // An explicit metallic or roughness tag always outranks an orm fallback.
        if let Some(mr) = preferred_metallic_roughness(files) {
            // Roughness outranks metallic, so the winner is metallic-tagged
            // only when the group holds no roughness file at all.
            set.metallic_roughness_has_metallic =
                !files.iter().any(|(tag, _)| is_roughness_tag(tag));
            set.metallic_roughness = Some(mr);
        } else {
            // Only an orm file can have filled the slot in the pass above, and
            // orm packs metalness in its blue channel.
            set.metallic_roughness_has_metallic = set.metallic_roughness.is_some();
        }

        if set.is_empty() {
            continue;
        }
        results.push(set);
    }

    results.sort_by(|a, b| a.base_name.cmp(&b.base_name));
    results
}

/// The final path component, treating both `/` and `\` as separators. Pure
/// string math, so a backslash path from any host splits the same way.
fn file_name_of(path: &str) -> String {
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_tag_maps_each_role() {
        assert_eq!(classify_tag("albedo"), Some(TextureRole::BaseColor));
        assert_eq!(classify_tag("Albedo"), Some(TextureRole::BaseColor));
        assert_eq!(classify_tag("normal"), Some(TextureRole::Normal));
        assert_eq!(
            classify_tag("roughness"),
            Some(TextureRole::MetallicRoughness)
        );
        assert_eq!(
            classify_tag("metallic"),
            Some(TextureRole::MetallicRoughness)
        );
        assert_eq!(classify_tag("emissive"), Some(TextureRole::Emissive));
        assert_eq!(classify_tag("ao"), Some(TextureRole::Occlusion));
        assert_eq!(classify_tag("height"), Some(TextureRole::Depth));
    }

    #[test]
    fn classify_tag_rejects_junk() {
        assert_eq!(classify_tag("readme"), None);
        assert_eq!(classify_tag(""), None);
        // `orm` is handled by grouping, not by classify_tag.
        assert_eq!(classify_tag("orm"), None);
    }

    #[test]
    fn groups_three_role_set() {
        let paths = vec![
            "/m/rock_albedo.png".to_string(),
            "/m/rock_normal.png".to_string(),
            "/m/rock_roughness.png".to_string(),
        ];
        let sets = group_texture_sets(&paths);
        assert_eq!(sets.len(), 1);
        let s = &sets[0];
        assert_eq!(s.base_name, "rock");
        assert_eq!(s.base_color.as_deref(), Some("/m/rock_albedo.png"));
        assert_eq!(s.normal.as_deref(), Some("/m/rock_normal.png"));
        assert_eq!(
            s.metallic_roughness.as_deref(),
            Some("/m/rock_roughness.png")
        );
        assert_eq!(s.emissive, None);
        assert_eq!(s.occlusion, None);
        assert_eq!(s.depth, None);
    }

    #[test]
    fn orm_fills_both_roles() {
        let paths = vec!["/m/x_orm.png".to_string()];
        let sets = group_texture_sets(&paths);
        assert_eq!(sets.len(), 1);
        let s = &sets[0];
        assert_eq!(s.metallic_roughness.as_deref(), Some("/m/x_orm.png"));
        assert_eq!(s.occlusion.as_deref(), Some("/m/x_orm.png"));
    }

    #[test]
    fn orm_does_not_overwrite_explicit_metallic_roughness() {
        let paths = vec!["/m/x_metallic.png".to_string(), "/m/x_orm.png".to_string()];
        let sets = group_texture_sets(&paths);
        assert_eq!(sets.len(), 1);
        let s = &sets[0];
        // Explicit metallic map wins the MR slot; ORM still fills occlusion.
        assert_eq!(s.metallic_roughness.as_deref(), Some("/m/x_metallic.png"));
        assert_eq!(s.occlusion.as_deref(), Some("/m/x_orm.png"));
    }

    #[test]
    fn metallic_roughness_collapse_roughness_wins() {
        let paths = vec![
            "/m/x_metallic.png".to_string(),
            "/m/x_roughness.png".to_string(),
        ];
        let sets = group_texture_sets(&paths);
        assert_eq!(sets.len(), 1);
        let s = &sets[0];
        // Both collapse into one slot; roughness wins over metallic.
        assert_eq!(s.metallic_roughness.as_deref(), Some("/m/x_roughness.png"));
        assert_eq!(s.occlusion, None);
    }

    #[test]
    fn separate_ao_populates_occlusion_only() {
        let paths = vec!["/m/x_ao.png".to_string()];
        let sets = group_texture_sets(&paths);
        assert_eq!(sets.len(), 1);
        let s = &sets[0];
        assert_eq!(s.occlusion.as_deref(), Some("/m/x_ao.png"));
        assert_eq!(s.metallic_roughness, None);
    }

    #[test]
    fn non_matching_files_yield_empty() {
        let paths = vec!["/m/readme.txt".to_string(), "/m/notes.md".to_string()];
        let sets = group_texture_sets(&paths);
        assert!(sets.is_empty());
    }

    #[test]
    fn matching_base_with_only_unknown_tag_drops() {
        // `alpha` is in the regex but not classified into any role, so the set
        // ends up empty and is dropped.
        let paths = vec!["/m/x_alpha.png".to_string()];
        let sets = group_texture_sets(&paths);
        assert!(sets.is_empty());
    }

    #[test]
    fn results_sorted_by_base_name() {
        let paths = vec![
            "/m/zeta_albedo.png".to_string(),
            "/m/alpha_albedo.png".to_string(),
        ];
        let sets = group_texture_sets(&paths);
        assert_eq!(sets.len(), 2);
        assert_eq!(sets[0].base_name, "alpha");
        assert_eq!(sets[1].base_name, "zeta");
    }

    #[test]
    fn recommended_scalars_reflect_present_roles() {
        let with_mr = MaterialSet {
            metallic_roughness: Some("/m/x_metallic.png".to_string()),
            metallic_roughness_has_metallic: true,
            ..MaterialSet::default()
        };
        let s = with_mr.recommended_scalars();
        assert_eq!(s.metallic, 1.0);
        assert_eq!(s.perceptual_roughness, 1.0);
        assert_eq!(s.parallax_depth_scale, 0.0);
        assert_eq!(s.max_parallax_layer_count, 0.0);

        let with_depth = MaterialSet {
            depth: Some("/m/x_height.png".to_string()),
            ..MaterialSet::default()
        };
        let s = with_depth.recommended_scalars();
        assert_eq!(s.metallic, 0.0);
        assert_eq!(s.perceptual_roughness, 0.5);
        assert_eq!(s.parallax_depth_scale, 0.05);
        assert_eq!(s.max_parallax_layer_count, 32.0);
    }

    #[test]
    fn roughness_only_pack_keeps_metallic_scalar_at_zero() {
        let sets = group_texture_sets(&[
            "/m/grass_05_basecolor.png".to_string(),
            "/m/grass_05_roughness.png".to_string(),
        ]);
        assert_eq!(sets.len(), 1);
        assert!(!sets[0].metallic_roughness_has_metallic);
        let s = sets[0].recommended_scalars();
        assert_eq!(s.metallic, 0.0);
        assert_eq!(s.perceptual_roughness, 1.0);
    }

    #[test]
    fn metallic_tagged_pack_raises_metallic_scalar() {
        let sets = group_texture_sets(&["/m/steel_metallic.png".to_string()]);
        assert_eq!(sets.len(), 1);
        assert!(sets[0].metallic_roughness_has_metallic);
        assert_eq!(sets[0].recommended_scalars().metallic, 1.0);
    }

    #[test]
    fn orm_pack_raises_metallic_scalar() {
        let sets = group_texture_sets(&["/m/crate_orm.png".to_string()]);
        assert_eq!(sets.len(), 1);
        assert!(sets[0].metallic_roughness_has_metallic);
        assert_eq!(sets[0].recommended_scalars().metallic, 1.0);
    }

    #[test]
    fn roughness_beating_metallic_leaves_metallic_scalar_at_zero() {
        // The roughness map wins the single slot, so no metallic data is bound.
        let sets = group_texture_sets(&[
            "/m/x_metallic.png".to_string(),
            "/m/x_roughness.png".to_string(),
        ]);
        assert_eq!(sets.len(), 1);
        assert!(!sets[0].metallic_roughness_has_metallic);
        assert_eq!(sets[0].recommended_scalars().metallic, 0.0);
    }

    #[test]
    fn is_srgb_only_for_color_roles() {
        assert!(TextureRole::BaseColor.is_srgb());
        assert!(TextureRole::Emissive.is_srgb());
        assert!(!TextureRole::Normal.is_srgb());
        assert!(!TextureRole::MetallicRoughness.is_srgb());
        assert!(!TextureRole::Occlusion.is_srgb());
        assert!(!TextureRole::Depth.is_srgb());
    }

    #[test]
    fn separators_all_match() {
        for path in [
            "/m/rock_albedo.png",
            "/m/rock-albedo.png",
            "/m/rock.albedo.png",
            "/m/rock albedo.png",
        ] {
            let sets = group_texture_sets(&[path.to_string()]);
            assert_eq!(sets.len(), 1, "separator in {path} should match");
            assert_eq!(sets[0].base_name, "rock");
            assert_eq!(sets[0].base_color.as_deref(), Some(path));
        }
    }

    #[test]
    fn backslash_paths_split_to_file_name() {
        let paths = vec![r"C:\assets\rock_albedo.png".to_string()];
        let sets = group_texture_sets(&paths);
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].base_name, "rock");
    }

    #[test]
    fn resolution_suffixed_packs_detect_two_full_sets() {
        let paths = vec![
            "/tex/grass_05_basecolor_1k.png".to_string(),
            "/tex/grass_05_normal_gl_1k.png".to_string(),
            "/tex/grass_05_normal_dx_1k.png".to_string(),
            "/tex/grass_05_roughness_1k.png".to_string(),
            "/tex/cliff_rocks_07_basecolor_1k.png".to_string(),
            "/tex/cliff_rocks_07_ambientocclusion_1k.png".to_string(),
            "/tex/cliff_rocks_07_height_1k.png".to_string(),
            "/tex/cliff_rocks_07_metallic_1k.png".to_string(),
            "/tex/cliff_rocks_07_normal_dx_1k.png".to_string(),
            "/tex/cliff_rocks_07_normal_gl_1k.png".to_string(),
            "/tex/cliff_rocks_07_roughness_1k.png".to_string(),
        ];
        let sets = group_texture_sets(&paths);
        assert_eq!(sets.len(), 2);

        // Sorted by base name: "cliff_rocks_07" precedes "grass_05".
        let cliff = &sets[0];
        assert_eq!(cliff.base_name, "cliff_rocks_07");
        assert_eq!(
            cliff.base_color.as_deref(),
            Some("/tex/cliff_rocks_07_basecolor_1k.png")
        );
        assert_eq!(
            cliff.normal.as_deref(),
            Some("/tex/cliff_rocks_07_normal_gl_1k.png")
        );
        assert_eq!(
            cliff.metallic_roughness.as_deref(),
            Some("/tex/cliff_rocks_07_roughness_1k.png")
        );
        assert_eq!(
            cliff.occlusion.as_deref(),
            Some("/tex/cliff_rocks_07_ambientocclusion_1k.png")
        );
        assert_eq!(
            cliff.depth.as_deref(),
            Some("/tex/cliff_rocks_07_height_1k.png")
        );

        let grass = &sets[1];
        assert_eq!(grass.base_name, "grass_05");
        assert_eq!(
            grass.base_color.as_deref(),
            Some("/tex/grass_05_basecolor_1k.png")
        );
        assert_eq!(
            grass.normal.as_deref(),
            Some("/tex/grass_05_normal_gl_1k.png")
        );
        assert_eq!(
            grass.metallic_roughness.as_deref(),
            Some("/tex/grass_05_roughness_1k.png")
        );
        assert_eq!(grass.occlusion, None);
        assert_eq!(grass.depth, None);
    }

    #[test]
    fn gl_normal_wins_regardless_of_supply_order() {
        let gl = "/m/x_normal_gl.png".to_string();
        let dx = "/m/x_normal_dx.png".to_string();

        let forward = group_texture_sets(&[gl.clone(), dx.clone()]);
        let reversed = group_texture_sets(&[dx, gl]);

        assert_eq!(forward, reversed);
        assert_eq!(forward[0].normal.as_deref(), Some("/m/x_normal_gl.png"));
    }

    #[test]
    fn dx_normal_fills_slot_only_when_no_gl_present() {
        let paths = vec!["/m/x_normal_dx.png".to_string()];
        let sets = group_texture_sets(&paths);
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].normal.as_deref(), Some("/m/x_normal_dx.png"));
    }

    #[test]
    fn short_normal_convention_tags_classify_as_normal() {
        assert_eq!(classify_tag("nor_gl"), Some(TextureRole::Normal));
        assert_eq!(classify_tag("nor-dx"), Some(TextureRole::Normal));
        assert_eq!(classify_tag("normaldx"), Some(TextureRole::Normal));
    }

    #[test]
    fn roughness_wins_regardless_of_supply_order() {
        let metallic = "/m/x_metallic.png".to_string();
        let roughness = "/m/x_roughness.png".to_string();

        let forward = group_texture_sets(&[metallic.clone(), roughness.clone()]);
        let reversed = group_texture_sets(&[roughness, metallic]);

        assert_eq!(forward, reversed);
        assert_eq!(
            forward[0].metallic_roughness.as_deref(),
            Some("/m/x_roughness.png")
        );
    }

    #[test]
    fn resolution_suffix_variants_are_stripped_from_base_name() {
        for path in [
            "/m/rock_basecolor_1k.png",
            "/m/rock_basecolor-2K.png",
            "/m/rock_basecolor_4k.png",
            "/m/rock_basecolor_8k.png",
            "/m/rock_basecolor_1024.png",
            "/m/rock_basecolor_2048.png",
            "/m/rock_basecolor_512.png",
        ] {
            let sets = group_texture_sets(&[path.to_string()]);
            assert_eq!(sets.len(), 1, "{path} should match");
            assert_eq!(sets[0].base_name, "rock", "{path} base name");
            assert_eq!(sets[0].base_color.as_deref(), Some(path));
        }
    }

    #[test]
    fn resolution_digits_without_role_tag_do_not_match() {
        let paths = vec!["/m/rock_2048.png".to_string()];
        let sets = group_texture_sets(&paths);
        assert!(sets.is_empty());
    }

    #[test]
    fn short_base_color_tag_still_detects_without_resolution() {
        let paths = vec!["/m/brick_b.png".to_string()];
        let sets = group_texture_sets(&paths);
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].base_color.as_deref(), Some("/m/brick_b.png"));
    }
}
