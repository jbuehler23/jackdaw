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
pub struct MaterialSet {
    pub base_name: String,
    pub base_color: Option<String>,
    pub normal: Option<String>,
    pub metallic_roughness: Option<String>,
    pub emissive: Option<String>,
    pub occlusion: Option<String>,
    pub depth: Option<String>,
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
    /// A metallic-roughness texture multiplies the scalar values, so both
    /// scalars default to 1.0 to use the texture as-is (otherwise 0.0 metallic,
    /// 0.5 roughness). A depth map enables parallax with a gentle scale and
    /// layer cap (otherwise both zero, disabling it).
    pub fn recommended_scalars(&self) -> PbrScalars {
        let has_mr = self.metallic_roughness.is_some();
        let has_depth = self.depth.is_some();
        PbrScalars {
            metallic: if has_mr { 1.0 } else { 0.0 },
            perceptual_roughness: if has_mr { 1.0 } else { 0.5 },
            parallax_depth_scale: if has_depth { 0.05 } else { 0.0 },
            max_parallax_layer_count: if has_depth { 32.0 } else { 0.0 },
        }
    }
}

/// The compiled filename pattern. `None` only if the static pattern fails to
/// compile (it does not, in practice).
///
/// The pattern matches `<base><sep><tag>.<ext>` case-insensitively, where the
/// separator is `_`, `-`, `.`, or a space. Group 1 captures the base name and
/// group 2 captures the role tag.
pub fn pbr_filename_regex() -> Option<regex::Regex> {
    let pattern = r"(?i)^(.+?)[_\-\.\s](diffuse|diff|albedo|base|col|color|basecolor|metallic|metalness|metal|mtl|roughness|rough|rgh|normal|normaldx|normalgl|nor|nrm|nrml|norm|orm|emission|emissive|emit|ao|ambient|occlusion|ambientocclusion|displacement|displace|disp|dsp|height|heightmap|alpha|opacity|specularity|specular|spec|spc|gloss|glossy|glossiness|bump|bmp|b|n)\.(png|jpg|jpeg|ktx2|bmp|tga|webp)$";
    regex::Regex::new(pattern).ok()
}

/// Classify a filename tag (the captured `<tag>` group, case-insensitive) into a
/// texture role. Returns `None` for an unrecognized tag.
///
/// The `orm` tag is intentionally not classified here; it maps to two roles and
/// is handled inside [`group_texture_sets`].
pub fn classify_tag(tag: &str) -> Option<TextureRole> {
    match tag.to_lowercase().as_str() {
        "diffuse" | "diff" | "albedo" | "base" | "col" | "color" | "basecolor" | "b" => {
            Some(TextureRole::BaseColor)
        }
        "normalgl" | "nor" | "nrm" | "nrml" | "norm" | "bump" | "bmp" | "n" | "normal" => {
            Some(TextureRole::Normal)
        }
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

/// Group a list of file paths into detected material sets.
///
/// Each path's file name is matched against [`pbr_filename_regex`]; matches are
/// grouped by the captured base name (lowercased). Within a group, each file is
/// assigned to the role from [`classify_tag`], applying these rules: an `orm`
/// file fills both the metallic-roughness and occlusion roles when each is still
/// empty; metallic and roughness tags share the metallic-roughness role (the
/// first seen wins); the first file seen for any role wins. Empty sets are
/// dropped. The result is sorted by base name.
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

        for (tag, path) in files {
            let tag_lower = tag.to_lowercase();
            if tag_lower == "orm" {
                // ORM packs occlusion, roughness, and metallic into one image,
                // so it fills both the metallic-roughness and occlusion roles,
                // but only where each is still empty (an explicit map wins).
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
                TextureRole::Normal => &mut set.normal,
                TextureRole::MetallicRoughness => &mut set.metallic_roughness,
                TextureRole::Emissive => &mut set.emissive,
                TextureRole::Occlusion => &mut set.occlusion,
                TextureRole::Depth => &mut set.depth,
            };
            if slot.is_none() {
                *slot = Some(path.clone());
            }
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
    fn metallic_roughness_collapse_first_wins() {
        let paths = vec![
            "/m/x_metallic.png".to_string(),
            "/m/x_roughness.png".to_string(),
        ];
        let sets = group_texture_sets(&paths);
        assert_eq!(sets.len(), 1);
        let s = &sets[0];
        // Both collapse into one slot; the first seen (metallic) wins.
        assert_eq!(s.metallic_roughness.as_deref(), Some("/m/x_metallic.png"));
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
            metallic_roughness: Some("/m/x_roughness.png".to_string()),
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
}
