use std::collections::HashMap;
use std::path::Path;

use bevy::prelude::*;
use regex::Regex;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Reflect, Serialize, Deserialize)]
pub struct MaterialDefinition {
    pub name: String,
    pub base_color_texture: Option<String>,
    pub normal_map_texture: Option<String>,
    pub metallic_roughness_texture: Option<String>,
    pub emissive_texture: Option<String>,
    pub occlusion_texture: Option<String>,
    pub depth_texture: Option<String>,
    #[serde(default = "default_base_color")]
    pub base_color: [f32; 4],
    #[serde(default)]
    pub metallic: f32,
    #[serde(default = "default_half")]
    pub perceptual_roughness: f32,
    #[serde(default = "default_half")]
    pub reflectance: f32,
    #[serde(default)]
    pub emissive_intensity: f32,
    #[serde(default)]
    pub double_sided: bool,
    #[serde(default)]
    pub flip_normal_map_y: bool,
}

fn default_base_color() -> [f32; 4] {
    [1.0, 1.0, 1.0, 1.0]
}

fn default_half() -> f32 {
    0.5
}

impl Default for MaterialDefinition {
    fn default() -> Self {
        Self {
            name: String::new(),
            base_color_texture: None,
            normal_map_texture: None,
            metallic_roughness_texture: None,
            emissive_texture: None,
            occlusion_texture: None,
            depth_texture: None,
            base_color: [1.0, 1.0, 1.0, 1.0],
            metallic: 0.0,
            perceptual_roughness: 0.5,
            reflectance: 0.5,
            emissive_intensity: 0.0,
            double_sided: false,
            flip_normal_map_y: false,
        }
    }
}

#[derive(Resource, Default)]
pub struct MaterialLibrary {
    pub materials: Vec<MaterialDefinition>,
}

impl MaterialLibrary {
    pub fn get_by_name(&self, name: &str) -> Option<&MaterialDefinition> {
        self.materials.iter().find(|m| m.name == name)
    }

    pub fn get_by_name_mut(&mut self, name: &str) -> Option<&mut MaterialDefinition> {
        self.materials.iter_mut().find(|m| m.name == name)
    }

    pub fn add(&mut self, def: MaterialDefinition) {
        self.materials.push(def);
    }

    pub fn remove_by_name(&mut self, name: &str) {
        self.materials.retain(|m| m.name != name);
    }

    pub fn upsert(&mut self, def: MaterialDefinition) {
        if let Some(existing) = self.materials.iter_mut().find(|m| m.name == def.name) {
            *existing = def;
        } else {
            self.materials.push(def);
        }
    }
}

pub struct MaterialDefCacheEntry {
    pub material: Handle<StandardMaterial>,
    pub preview_image: Option<Handle<Image>>,
    /// Base color texture handle for use as a fallback thumbnail.
    pub base_color_image: Option<Handle<Image>>,
}

#[derive(Resource, Default)]
pub struct MaterialDefinitionCache {
    pub entries: HashMap<String, MaterialDefCacheEntry>,
}

/// Compiled PBR filename regex pattern.
pub fn pbr_filename_regex() -> Option<Regex> {
    let pattern = r"(?i)^(.+?)[_\-\.\s](diffuse|diff|albedo|base|col|color|basecolor|metallic|metalness|metal|mtl|roughness|rough|rgh|normal|nor|nrm|nrml|norm|orm|emission|emissive|emit|ao|ambient|occlusion|displacement|displace|disp|dsp|height|heightmap|alpha|opacity|specularity|specular|spec|spc|gloss|glossy|glossiness|bump|bmp|b|n)\.(png|jpg|jpeg|ktx2|bmp|tga|webp)$";
    Regex::new(pattern).ok()
}

/// Parse a texture filename into `(base_name, tag)` using the PBR naming convention.
pub fn parse_texture_filename(filename: &str, re: &Regex) -> Option<(String, String)> {
    let caps = re.captures(filename)?;
    Some((caps[1].to_string(), caps[2].to_string()))
}

/// Build a `MaterialDefinition` from a name and a list of `(tag, asset_path)` slots.
pub fn build_material_from_slots(name: String, slots: &[(String, String)]) -> MaterialDefinition {
    let mut def = MaterialDefinition {
        name,
        ..Default::default()
    };

    for (tag, asset_path) in slots {
        let tag_lower = tag.to_lowercase();
        match tag_lower.as_str() {
            "diffuse" | "diff" | "albedo" | "base" | "col" | "color" | "basecolor" | "b" => {
                def.base_color_texture = Some(asset_path.clone());
            }
            "normal" | "nor" | "nrm" | "nrml" | "norm" | "bump" | "bmp" | "n" => {
                def.normal_map_texture = Some(asset_path.clone());
            }
            "metallic" | "metalness" | "metal" | "mtl" | "roughness" | "rough" | "rgh"
            | "orm" => {
                def.metallic_roughness_texture = Some(asset_path.clone());
            }
            "specular" | "specularity" | "spec" | "spc" => {
                if def.metallic_roughness_texture.is_none() {
                    def.metallic_roughness_texture = Some(asset_path.clone());
                }
            }
            "emission" | "emissive" | "emit" => {
                def.emissive_texture = Some(asset_path.clone());
            }
            "ao" | "ambient" | "occlusion" => {
                def.occlusion_texture = Some(asset_path.clone());
            }
            "displacement" | "displace" | "disp" | "dsp" | "height" | "heightmap" => {
                def.depth_texture = Some(asset_path.clone());
            }
            "gloss" | "glossy" | "glossiness" => {
                if def.metallic_roughness_texture.is_none() {
                    def.metallic_roughness_texture = Some(asset_path.clone());
                }
            }
            _ => {}
        }
    }

    def
}

/// Returns true if the KTX2 file is NOT a simple 2D texture (cubemap or array texture).
pub fn is_ktx2_non_2d(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    use std::io::Read;
    let mut header = [0u8; 40];
    if file.read_exact(&mut header).is_err() {
        return false;
    }
    let layer_count = u32::from_le_bytes([header[32], header[33], header[34], header[35]]);
    let face_count = u32::from_le_bytes([header[36], header[37], header[38], header[39]]);
    layer_count > 1 || face_count > 1
}

/// Scan a directory for PBR texture sets and auto-detect material definitions.
pub fn detect_material_sets(dir: &Path, asset_root: &Path) -> Vec<MaterialDefinition> {
    let pattern = r"(?i)^(.+?)[_\-\.\s](diffuse|diff|albedo|base|col|color|basecolor|metallic|metalness|metal|mtl|roughness|rough|rgh|normal|nor|nrm|nrml|norm|orm|emission|emissive|emit|ao|ambient|occlusion|displacement|displace|disp|dsp|height|heightmap|alpha|opacity|specularity|specular|spec|spc|gloss|glossy|glossiness|bump|bmp|b|n)\.(png|jpg|jpeg|ktx2|bmp|tga|webp)$";

    let re = match Regex::new(pattern) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let mut groups: HashMap<String, Vec<(String, String)>> = HashMap::new();

    scan_dir_recursive(dir, asset_root, &re, &mut groups);

    let mut results = Vec::new();
    for (base_name, slots) in &groups {
        let mut def = MaterialDefinition {
            name: base_name.clone(),
            ..Default::default()
        };

        for (tag, asset_path) in slots {
            let tag_lower = tag.to_lowercase();
            match tag_lower.as_str() {
                "diffuse" | "diff" | "albedo" | "base" | "col" | "color" | "basecolor" | "b" => {
                    def.base_color_texture = Some(asset_path.clone());
                }
                "normal" | "nor" | "nrm" | "nrml" | "norm" | "bump" | "bmp" | "n" => {
                    def.normal_map_texture = Some(asset_path.clone());
                }
                "metallic" | "metalness" | "metal" | "mtl" | "roughness" | "rough" | "rgh"
                | "orm" => {
                    def.metallic_roughness_texture = Some(asset_path.clone());
                }
                "specular" | "specularity" | "spec" | "spc" => {
                    if def.metallic_roughness_texture.is_none() {
                        def.metallic_roughness_texture = Some(asset_path.clone());
                    }
                }
                "emission" | "emissive" | "emit" => {
                    def.emissive_texture = Some(asset_path.clone());
                }
                "ao" | "ambient" | "occlusion" => {
                    def.occlusion_texture = Some(asset_path.clone());
                }
                "displacement" | "displace" | "disp" | "dsp" | "height" | "heightmap" => {
                    def.depth_texture = Some(asset_path.clone());
                }
                "gloss" | "glossy" | "glossiness" => {
                    // Store as roughness slot (inversion deferred to v2)
                    if def.metallic_roughness_texture.is_none() {
                        def.metallic_roughness_texture = Some(asset_path.clone());
                    }
                }
                // alpha, opacity — noted but not directly mapped in v1
                _ => {}
            }
        }

        // Only include if at least one texture slot is populated
        if def.base_color_texture.is_some()
            || def.normal_map_texture.is_some()
            || def.metallic_roughness_texture.is_some()
            || def.emissive_texture.is_some()
            || def.occlusion_texture.is_some()
            || def.depth_texture.is_some()
        {
            results.push(def);
        }
    }

    results.sort_by(|a, b| a.name.cmp(&b.name));
    results
}

fn scan_dir_recursive(
    dir: &Path,
    asset_root: &Path,
    re: &Regex,
    groups: &mut HashMap<String, Vec<(String, String)>>,
) {
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in read_dir.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_dir_recursive(&path, asset_root, re, groups);
        } else {
            let file_name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();

            // Skip non-2D KTX2 files (cubemaps, array textures)
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("ktx2"))
                && is_ktx2_non_2d(&path)
            {
                continue;
            }

            if let Some(caps) = re.captures(&file_name) {
                let base_name = caps[1].to_string();
                let tag = caps[2].to_string();

                let asset_path = path
                    .strip_prefix(asset_root)
                    .map(|r| r.to_string_lossy().replace('\\', "/"))
                    .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"));

                groups
                    .entry(base_name.to_lowercase())
                    .or_default()
                    .push((tag, asset_path));

                // Use original case for display name
                groups
                    .entry(base_name.to_lowercase())
                    .and_modify(|_| {})
                    .or_default();
            }
        }
    }
}
