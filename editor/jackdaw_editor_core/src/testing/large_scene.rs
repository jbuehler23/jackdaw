//! A generated scene about the size of a large hand-built level: hundreds of
//! brushes in groups several levels deep, tens of thousands of faces and a few
//! dozen shared material files. Everything goes through the editor's own
//! operators and save, so the file follows the current format.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw_api_internal::operator::{OperatorResult, OperatorWorldExt as _};
use jackdaw_scene_types::{Brush, PropertyValue};

use crate::entity_ops::EntityTemplate;
use crate::project::{ProjectRoot, create_default_project, load_project_config};

/// The scene file the generator writes, under the project's assets folder.
pub const LARGE_SCENE_FILE: &str = "large_scene.bsn";

const SEED: u64 = 0x6a61_636b_6461_7721;
const DISTRICTS: usize = 10;
const PROPS_PER_DISTRICT: usize = 3;
const BRUSHES_PER_PROP: usize = 10;
const DETAIL_GROUPS: usize = 3;
const BRUSHES_PER_DETAIL: usize = 4;
const LOOSE_BRUSHES: usize = 449;
const POINT_LIGHTS: usize = 5;
const EXTRA_PREFAB_INSTANCES: usize = 4;
const PREFAB_FILE: &str = "prefabs/lantern.bsn";

const MATERIALS: [&str; 43] = [
    "stone",
    "stone_dark",
    "cobble",
    "brick",
    "plaster",
    "timber",
    "wood",
    "wood_light",
    "thatch",
    "roof_red",
    "roof_slate",
    "iron",
    "gold",
    "copper",
    "glass",
    "water",
    "soil",
    "soil_dark",
    "sand",
    "grass",
    "grass_dark",
    "grass_light",
    "moss",
    "leaf",
    "leaf_dark",
    "leaf_light",
    "bark",
    "cloth_red",
    "cloth_blue",
    "cloth_green",
    "cloth_yellow",
    "cloth_purple",
    "cloth_brown",
    "paint_white",
    "paint_black",
    "clay",
    "bone",
    "crystal",
    "ice",
    "glow_lamp",
    "glow_window",
    "glow_crystal",
    "glow_ember",
];

/// What [`write_large_scene`] put in the scene, counted as it went.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LargeScene {
    /// The saved scene file.
    pub path: PathBuf,
    /// Empty nodes that hold others, prefab instances not included.
    pub groups: usize,
    /// Brushes authored in the scene itself, not those its prefab instances bring.
    pub brushes: usize,
    /// Faces over all of [`Self::brushes`].
    pub faces: usize,
    pub point_lights: usize,
    /// Material files written, each used by at least one brush.
    pub materials: usize,
    pub prefab_instances: usize,
    /// Brushes inside the one prefab every instance shares.
    pub prefab_brushes: usize,
}

impl LargeScene {
    /// Every node the saved document holds: groups, brushes, lights, the camera,
    /// the directional light a new scene starts with, and the instances.
    pub fn nodes(&self) -> usize {
        self.groups + self.brushes + self.point_lights + 2 + self.prefab_instances
    }
}

/// Generate the large scene into the project at `project` and save it as
/// [`LARGE_SCENE_FILE`] in its assets folder, with its materials under
/// `materials/` and its prefab under `prefabs/`. The same input gives the same
/// scene, apart from the node ids a save mints.
pub fn write_large_scene(project: &Path) -> Result<LargeScene, String> {
    let config = load_project_config(project).unwrap_or_else(|| create_default_project(project));
    let root = ProjectRoot::new(project, config);
    let assets = root.assets_dir();
    std::fs::create_dir_all(&assets).map_err(|err| format!("{}: {err}", assets.display()))?;

    let mut app = super::editor_test_app_with(Some(&assets), |_| {});
    app.world_mut().insert_resource(root);
    app.world_mut()
        .resource_mut::<NextState<crate::AppState>>()
        .set(crate::AppState::Editor);
    app.update();
    app.update();

    let path = assets.join(LARGE_SCENE_FILE);
    run(
        app.world_mut(),
        "scene.new",
        &[("path", path.to_string_lossy().into_owned().into())],
    )?;
    let materials = create_materials(&mut app)?;
    let mut scene = populate(app.world_mut(), &assets, &materials)?;
    scene.path = path;
    run(app.world_mut(), "scene.save", &[])?;
    app.update();
    Ok(scene)
}

fn run(
    world: &mut World,
    id: &'static str,
    params: &[(&'static str, PropertyValue)],
) -> Result<(), String> {
    let mut call = world.operator(id);
    for (key, value) in params {
        call = call.param(*key, value.clone());
    }
    match call.call() {
        Ok(OperatorResult::Finished) => Ok(()),
        other => Err(format!("{id} did not finish: {other:?}")),
    }
}

/// Write every material file through `material.create` and hand back the
/// handles in [`MATERIALS`] order.
fn create_materials(app: &mut App) -> Result<Vec<Handle<StandardMaterial>>, String> {
    let mut rng = Seeded(SEED ^ 0x6d61_7465_7269_616c);
    for name in MATERIALS {
        let color = format!(
            "{:.3},{:.3},{:.3}",
            rng.range(0.1, 0.95),
            rng.range(0.1, 0.95),
            rng.range(0.1, 0.95)
        );
        let metallic = if matches!(name, "iron" | "gold" | "copper") {
            0.9
        } else {
            0.0
        };
        let mut params = vec![
            ("name", name.to_string().into()),
            ("base_color", color.into()),
            ("roughness", f64::from(rng.range(0.3, 1.0)).into()),
            ("metallic", metallic.into()),
        ];
        if name.starts_with("glow_") {
            params.push(("emissive", "4.0,2.5,1.0".to_string().into()));
        }
        run(app.world_mut(), "material.create", &params)?;
    }
    app.update();

    let world = app.world();
    let index = world.get_resource::<crate::asset_index::AssetIndex>();
    let registry = world.resource::<crate::material_assets::MaterialRegistry>();
    MATERIALS
        .iter()
        .map(|name| {
            let reference = format!(
                "{}/{}",
                crate::material_assets::MATERIALS_DIR,
                crate::material_assets::material_file_name(name)
            );
            crate::material_assets::material_of_reference(index, registry, &reference)
                .ok_or_else(|| format!("{reference} was not indexed after it was written"))
        })
        .collect()
}

/// Build the scene's nodes into the open document.
fn populate(
    world: &mut World,
    assets: &Path,
    materials: &[Handle<StandardMaterial>],
) -> Result<LargeScene, String> {
    let mut rng = Seeded(SEED);
    let picker = MaterialPicker::new(materials.len());
    let mut scene = LargeScene::default();
    let mut used = vec![false; materials.len()];
    let mut brush = |world: &mut World,
                     rng: &mut Seeded,
                     scene: &mut LargeScene,
                     name: &str,
                     at: Vec3,
                     parent: Entity| {
        let mut shape = random_shape(rng);
        let material = picker.pick(rng);
        used[material] = true;
        for face in &mut shape.faces {
            face.material = materials[material].clone();
        }
        scene.brushes += 1;
        scene.faces += shape.faces.len();
        let yaw = rng.range(0.0, std::f32::consts::TAU);
        let name = name.to_string();
        crate::entity_ops::spawn_brush_under(world, shape, Some(parent), move |entity| {
            entity.insert((
                Name::new(name),
                Transform::from_translation(at).with_rotation(Quat::from_rotation_y(yaw)),
            ));
        });
    };

    let prefab = assets.join(PREFAB_FILE);
    scene.prefab_brushes = write_prefab(world, &prefab, &materials[0])?;
    scene.prefab_instances = 1;

    spawn_node(
        world,
        EntityTemplate::Camera3d,
        "Main Camera",
        Transform::from_xyz(60.0, 40.0, 80.0).looking_at(Vec3::ZERO, Vec3::Y),
        None,
    );
    let village = group(world, &mut scene, "Village", Vec3::ZERO, None);

    let mut detail_groups = 0;
    let mut loose_left = LOOSE_BRUSHES;
    for district in 0..DISTRICTS {
        let origin = Vec3::new(
            (district % 5) as f32 * 40.0 - 80.0,
            0.0,
            (district / 5) as f32 * 40.0 - 20.0,
        );
        let district_node = group(
            world,
            &mut scene,
            &format!("district_{district}"),
            origin,
            Some(village),
        );
        for prop in 0..PROPS_PER_DISTRICT {
            let at = Vec3::new(rng.range(-15.0, 15.0), 0.0, rng.range(-15.0, 15.0));
            let kind = PROP_KINDS[(district * PROPS_PER_DISTRICT + prop) % PROP_KINDS.len()];
            let prop_node = group(world, &mut scene, kind.0, at, Some(district_node));
            for part in 0..BRUSHES_PER_PROP {
                let at = Vec3::new(
                    rng.range(-3.0, 3.0),
                    rng.range(0.0, 4.0),
                    rng.range(-3.0, 3.0),
                );
                brush(
                    world,
                    &mut rng,
                    &mut scene,
                    kind.1[part % kind.1.len()],
                    at,
                    prop_node,
                );
            }
            if prop == 0 && district % 3 == 0 && detail_groups < DETAIL_GROUPS {
                detail_groups += 1;
                let detail = group(
                    world,
                    &mut scene,
                    "window",
                    Vec3::new(0.0, 2.0, 2.5),
                    Some(prop_node),
                );
                for part in 0..BRUSHES_PER_DETAIL {
                    let at = Vec3::new(part as f32 * 0.3 - 0.45, 0.0, 0.0);
                    brush(world, &mut rng, &mut scene, "win_bar", at, detail);
                }
            }
            if scene.point_lights < POINT_LIGHTS && prop == 1 {
                scene.point_lights += 1;
                spawn_node(
                    world,
                    EntityTemplate::PointLight,
                    "lamp_light",
                    Transform::from_xyz(0.0, 3.0, 0.0),
                    Some(prop_node),
                );
            }
        }
        let loose = loose_left / (DISTRICTS - district);
        loose_left -= loose;
        for index in 0..loose {
            let at = Vec3::new(rng.range(-18.0, 18.0), 0.0, rng.range(-18.0, 18.0));
            let name = LOOSE_NAMES[index % LOOSE_NAMES.len()];
            brush(world, &mut rng, &mut scene, name, at, district_node);
        }
        if district < EXTRA_PREFAB_INSTANCES {
            let at = Vec3::new(rng.range(-10.0, 10.0), 0.0, rng.range(-10.0, 10.0));
            crate::prefab::operators::spawn_instance_placed(
                world,
                &prefab,
                Transform::from_translation(at),
                Some(district_node),
            );
            scene.prefab_instances += 1;
        }
    }
    if detail_groups != DETAIL_GROUPS {
        return Err(format!(
            "placed {detail_groups} detail groups, not {DETAIL_GROUPS}"
        ));
    }
    scene.materials = used.iter().filter(|used| **used).count();
    Ok(scene)
}

/// A prop's name and the names its parts take in turn.
const PROP_KINDS: [(&str, &[&str]); 5] = [
    (
        "house",
        &["wall", "wall", "roof", "door", "beam", "chimney"],
    ),
    ("tree", &["trunk", "canopy", "canopy", "branch", "root"]),
    ("well", &["rim_stone", "post", "roof", "bucket", "rope"]),
    ("cart", &["bed", "wheel", "wheel", "axle", "barrel"]),
    ("stall", &["counter", "post", "awning", "crate", "basket"]),
];

const LOOSE_NAMES: [&str; 8] = [
    "rock",
    "bush",
    "flower",
    "stem",
    "fence_post",
    "reed",
    "stone",
    "crate",
];

/// Build a small group, save it as the prefab at `path` through the editor's
/// own prefab save, and report how many brushes it holds. The group is
/// replaced in the scene by the first instance of it.
fn write_prefab(
    world: &mut World,
    path: &Path,
    material: &Handle<StandardMaterial>,
) -> Result<usize, String> {
    let lantern = spawn_node(
        world,
        EntityTemplate::Empty,
        "lantern",
        Transform::from_xyz(0.0, 0.0, 30.0),
        None,
    );
    let parts = [
        ("post", Brush::cylinder(0.1, 1.2, 8), 1.2),
        ("cage", Brush::cuboid(0.25, 0.3, 0.25), 2.6),
        ("cap", Brush::pyramid(0.3, 0.3, 0.2), 3.1),
    ];
    let count = parts.len();
    for (name, mut shape, height) in parts {
        for face in &mut shape.faces {
            face.material = material.clone();
        }
        crate::entity_ops::spawn_brush_under(world, shape, Some(lantern), move |entity| {
            entity.insert((Name::new(name), Transform::from_xyz(0.0, height, 0.0)));
        });
    }
    crate::prefab::operators::save_as_prefab_from_selection(world, &[lantern], path);
    if !path.is_file() {
        return Err(format!("the prefab {} was not written", path.display()));
    }
    Ok(count)
}

fn group(
    world: &mut World,
    scene: &mut LargeScene,
    name: &str,
    at: Vec3,
    parent: Option<Entity>,
) -> Entity {
    scene.groups += 1;
    spawn_node(
        world,
        EntityTemplate::Empty,
        name,
        Transform::from_translation(at),
        parent,
    )
}

fn spawn_node(
    world: &mut World,
    template: EntityTemplate,
    name: &str,
    transform: Transform,
    parent: Option<Entity>,
) -> Entity {
    let name = name.to_string();
    crate::entity_ops::spawn_shaped_under(world, template, parent, move |entity| {
        entity.insert((Name::new(name), transform));
    })
}

/// A brush of one of the editor's stock shapes, sized at random. The mix
/// averages about 27 faces a brush.
fn random_shape(rng: &mut Seeded) -> Brush {
    let size = rng.range(0.2, 1.5);
    let height = rng.range(0.2, 2.0);
    match rng.below(100) {
        0..24 => Brush::cuboid(rng.range(0.2, 2.0), height, rng.range(0.2, 2.0)),
        24..44 => Brush::cylinder(size, height, 8 + rng.below(9)),
        44..54 => Brush::cone(size, height, 8 + rng.below(5)),
        54..64 => Brush::frustum(size, size * 0.6, height, 8 + rng.below(5)),
        64..69 => Brush::wedge(size, height, rng.range(0.2, 2.0)),
        69..90 => Brush::ring_sphere(size, 8 + rng.below(3), 6 + rng.below(3)),
        _ => Brush::ico_sphere(size, 1),
    }
}

/// Picks materials with a long tail: the first few cover most brushes and the
/// last are used a handful of times.
struct MaterialPicker {
    cumulative: Vec<f32>,
}

impl MaterialPicker {
    fn new(count: usize) -> Self {
        let mut total = 0.0;
        let cumulative = (0..count)
            .map(|rank| {
                total += 1.0 / (rank as f32 + 1.0);
                total
            })
            .collect();
        Self { cumulative }
    }

    fn pick(&self, rng: &mut Seeded) -> usize {
        let total = self.cumulative.last().copied().unwrap_or(0.0);
        let roll = rng.range(0.0, total);
        self.cumulative
            .iter()
            .position(|edge| roll < *edge)
            .unwrap_or(self.cumulative.len().saturating_sub(1))
    }
}

/// A `SplitMix64` generator, so the scene is the same on every platform and run.
struct Seeded(u64);

impl Seeded {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn range(&mut self, low: f32, high: f32) -> f32 {
        let unit = (self.next() >> 40) as f32 / (1u64 << 24) as f32;
        low + (high - low) * unit
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}
