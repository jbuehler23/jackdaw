//! Materials from values and the project palette: `material.create` with a
//! preset and fields, `material.set` checked before it writes, palette names
//! in every material parameter, and `palette.apply` swapping a whole look as
//! one undo step.

use crate::util;

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw::commands::CommandHistory;
use jackdaw::material_palette::{MaterialPalettes, PALETTE_FILE};
use jackdaw::material_params::PRESETS;
use jackdaw::project::ProjectRoot;
use jackdaw::remote::server::{batch_handler, call_operator_handler, status_handler};
use jackdaw_scene_types::Brush;
use serde_json::{Value, json};

/// An editor on a fresh project with a materials folder and one image.
fn project_app(root: &Path) -> App {
    if !root.join("jackdaw.toml").exists() {
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/definition_project/jackdaw.toml"),
            root.join("jackdaw.toml"),
        )
        .expect("the manifest copies");
        std::fs::create_dir_all(root.join("assets/materials")).expect("a materials folder");
        std::fs::create_dir_all(root.join("assets/textures")).expect("a textures folder");
        std::fs::write(root.join("assets/textures/plank.png"), b"not decoded here")
            .expect("an image file");
    }
    let mut app = util::editor_test_app();
    app.world_mut().insert_resource(ProjectRoot {
        root: root.to_path_buf(),
        config: default(),
    });
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
    app.update();
    app
}

fn call(app: &mut App, id: &str, params: Value) -> Value {
    let answer = app
        .world_mut()
        .run_system_cached_with(
            call_operator_handler,
            Some(json!({ "id": id, "params": params })),
        )
        .expect("the handler ran")
        .unwrap_or_else(|err| panic!("{id} refused: {}", err.message));
    for _ in 0..3 {
        app.update();
    }
    answer
}

fn batch(app: &mut App, params: Value) -> Result<Value, bevy::remote::BrpError> {
    let answer = app
        .world_mut()
        .run_system_cached_with(batch_handler, Some(params))
        .expect("the handler ran");
    for _ in 0..3 {
        app.update();
    }
    answer
}

#[track_caller]
fn changed(answer: &Value) {
    assert_eq!(answer["outcome"], "changed", "{answer}");
}

#[track_caller]
fn finished(answer: &Value) {
    assert_eq!(answer["result"], "finished", "{answer}");
}

fn history_len(app: &App) -> usize {
    app.world().resource::<CommandHistory>().undo_stack.len()
}

fn undo(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.undo(world));
    app.update();
}

fn redo(app: &mut App) {
    app.world_mut()
        .resource_scope(|world, mut history: Mut<CommandHistory>| history.redo(world));
    app.update();
}

fn handle_at(app: &App, path: &str) -> Handle<StandardMaterial> {
    app.world()
        .resource::<jackdaw::asset_index::AssetIndex>()
        .get(Path::new(path))
        .and_then(|entry| entry.value.handle().cloned())
        .unwrap_or_else(|| panic!("the project holds {path}"))
        .typed::<StandardMaterial>()
}

fn material(app: &App, path: &str) -> StandardMaterial {
    app.world()
        .resource::<Assets<StandardMaterial>>()
        .get(&handle_at(app, path))
        .cloned()
        .expect("the material is loaded")
}

fn texture_path(material: &StandardMaterial) -> Option<PathBuf> {
    material
        .base_color_texture
        .as_ref()
        .and_then(|image| image.path())
        .map(|path| path.path().to_path_buf())
}

/// The values a material is made of, comparable across two materials.
fn values(material: &StandardMaterial) -> String {
    format!(
        "{:?} {} {} {} {:?} {:?} {:?} {:?}",
        material.base_color,
        material.perceptual_roughness,
        material.metallic,
        material.reflectance,
        material.emissive,
        material.alpha_mode,
        material.uv_transform,
        texture_path(material),
    )
}

fn face_materials(app: &App, entity: Entity) -> Vec<AssetId<StandardMaterial>> {
    app.world()
        .get::<Brush>(entity)
        .expect("a brush")
        .faces
        .iter()
        .map(|face| face.material.id())
        .collect()
}

fn cube(app: &mut App, name: &str, material: &str) -> Entity {
    let answer = call(
        app,
        "entity.add.cube",
        json!({ "name": name, "material": material }),
    );
    changed(&answer);
    let bits = answer["entities"][0].as_u64().expect("the cube's id");
    Entity::from_bits(bits)
}

fn the_fields() -> Value {
    json!({
        "base_color": "#336699",
        "roughness": 0.7,
        "metallic": 0.2,
        "emissive": "0.5,0.1,0",
        "texture": "textures/plank.png",
        "uv_scale": "2,3",
    })
}

#[test]
fn creating_with_fields_equals_creating_then_setting_them() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut app = project_app(tmp.path());

    let mut params = the_fields();
    params["name"] = json!("given");
    let answer = call(&mut app, "material.create", params);
    changed(&answer);
    assert_eq!(
        answer["reports"],
        json!(["given"]),
        "it hands back its name"
    );

    changed(&call(
        &mut app,
        "material.create",
        json!({ "name": "later" }),
    ));
    changed(&call(
        &mut app,
        "material.set",
        json!({ "material": "materials/later.bsn", "fields": the_fields() }),
    ));

    let given = material(&app, "materials/given.bsn");
    let later = material(&app, "materials/later.bsn");
    assert_eq!(values(&given), values(&later));
    assert_eq!(given.perceptual_roughness, 0.7);
    assert_eq!(
        given.uv_transform,
        bevy::math::Affine2::from_scale(Vec2::new(2.0, 3.0))
    );
    assert_eq!(
        texture_path(&given),
        Some(PathBuf::from("textures/plank.png"))
    );

    let file = |name: &str| {
        std::fs::read_to_string(tmp.path().join(format!("assets/materials/{name}.bsn")))
            .expect("the material is saved")
    };
    assert_eq!(
        file("given").replace("given", "NAME"),
        file("later").replace("later", "NAME"),
        "both files hold the same values"
    );
}

#[test]
fn a_preset_names_the_material_and_fields_override_it() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut app = project_app(tmp.path());
    assert_eq!(PRESETS.len(), 10);

    let answer = call(&mut app, "material.create", json!({ "preset": "stone" }));
    assert_eq!(answer["reports"], json!(["stone"]), "{answer}");
    let answer = call(
        &mut app,
        "material.create",
        json!({ "preset": "stone", "roughness": 0.3 }),
    );
    assert_eq!(answer["reports"], json!(["stone_2"]), "{answer}");

    let stone = material(&app, "materials/stone.bsn");
    let rougher = material(&app, "materials/stone_2.bsn");
    assert_eq!(stone.perceptual_roughness, 0.9);
    assert_eq!(rougher.perceptual_roughness, 0.3);
    assert_eq!(stone.base_color, rougher.base_color);

    let glass = call(&mut app, "material.create", json!({ "preset": "glass" }));
    changed(&glass);
    assert_eq!(
        material(&app, "materials/glass.bsn").alpha_mode,
        AlphaMode::Blend
    );

    for refused in [
        json!({ "preset": "marble" }),
        json!({ "name": "stone" }),
        json!({ "name": "has space" }),
        json!({ "roughness": 2.0 }),
        json!({ "texture": "textures/missing.png" }),
    ] {
        let answer = call(&mut app, "material.create", refused.clone());
        assert_eq!(answer["outcome"], "invalid", "{refused}: {answer}");
    }
}

#[test]
fn a_bad_field_or_value_leaves_the_material_unchanged() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut app = project_app(tmp.path());
    changed(&call(
        &mut app,
        "material.create",
        json!({ "name": "clay", "roughness": 0.6 }),
    ));
    let before = values(&material(&app, "materials/clay.bsn"));
    let file_before =
        std::fs::read_to_string(tmp.path().join("assets/materials/clay.bsn")).expect("saved");
    let depth = history_len(&app);

    for refused in [
        json!({ "field": "roughness", "value": 1.5 }),
        json!({ "field": "shininess", "value": 0.5 }),
        json!({ "field": "base_color", "value": "#zzzzzz" }),
        json!({ "field": "uv_scale", "value": "0" }),
        json!({ "field": "texture", "value": "textures/nothing.png" }),
        json!({ "fields": { "metallic": 1.0, "roughness": -1 } }),
        json!({ "fields": { "metallic": 1.0, "glow": 2 } }),
        json!({ "fields": { "metallic": 1.0 }, "field": "roughness", "value": 0.1 }),
        json!({}),
    ] {
        let mut params = refused.clone();
        params["material"] = json!("materials/clay.bsn");
        let answer = call(&mut app, "material.set", params);
        assert_eq!(answer["outcome"], "invalid", "{refused}: {answer}");
        assert!(!answer["warnings"].as_array().is_none_or(Vec::is_empty));
    }
    let answer = call(
        &mut app,
        "material.set",
        json!({ "material": "nowhere", "field": "roughness", "value": 0.1 }),
    );
    assert_eq!(answer["outcome"], "invalid", "{answer}");

    assert_eq!(values(&material(&app, "materials/clay.bsn")), before);
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("assets/materials/clay.bsn")).expect("saved"),
        file_before
    );
    assert_eq!(history_len(&app), depth, "nothing was recorded");
}

#[test]
fn material_set_is_one_undo_step_in_either_form() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut app = project_app(tmp.path());
    changed(&call(
        &mut app,
        "material.create",
        json!({ "name": "clay", "roughness": 0.6, "palette": "wall" }),
    ));
    let original = values(&material(&app, "materials/clay.bsn"));

    let depth = history_len(&app);
    changed(&call(
        &mut app,
        "material.set",
        json!({ "material": "wall", "field": "roughness", "value": "0.25" }),
    ));
    assert_eq!(
        material(&app, "materials/clay.bsn").perceptual_roughness,
        0.25
    );
    assert_eq!(history_len(&app), depth + 1);
    undo(&mut app);
    assert_eq!(values(&material(&app, "materials/clay.bsn")), original);

    changed(&call(
        &mut app,
        "material.set",
        json!({ "material": "wall", "fields": { "metallic": 1, "base_color": [1, 0, 0] } }),
    ));
    assert_eq!(history_len(&app), depth + 1, "several fields, one step");
    let set = material(&app, "materials/clay.bsn");
    assert_eq!(set.metallic, 1.0);
    assert_eq!(set.base_color, Color::srgb(1.0, 0.0, 0.0));
    let on_disk =
        std::fs::read_to_string(tmp.path().join("assets/materials/clay.bsn")).expect("saved");
    undo(&mut app);
    assert_eq!(values(&material(&app, "materials/clay.bsn")), original);
    redo(&mut app);
    assert_eq!(values(&material(&app, "materials/clay.bsn")), values(&set));
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("assets/materials/clay.bsn")).expect("saved"),
        on_disk,
        "the file follows the edit"
    );
}

#[test]
fn palette_names_resolve_in_material_apply_and_entity_add() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut app = project_app(tmp.path());
    finished(&call(&mut app, "scene.new", json!({})));
    changed(&call(
        &mut app,
        "material.create",
        json!({ "preset": "thatch", "palette": "roof" }),
    ));
    changed(&call(
        &mut app,
        "material.create",
        json!({ "preset": "timber" }),
    ));
    changed(&call(
        &mut app,
        "palette.set",
        json!({ "name": "beam", "material": "materials/timber.bsn" }),
    ));
    let thatch = handle_at(&app, "materials/thatch.bsn").id();
    let timber = handle_at(&app, "materials/timber.bsn").id();

    let hut = cube(&mut app, "Hut", "roof");
    assert!(face_materials(&app, hut).iter().all(|id| *id == thatch));

    finished(&call(
        &mut app,
        "material.apply",
        json!({ "material": "beam", "entities": "Hut" }),
    ));
    assert!(face_materials(&app, hut).iter().all(|id| *id == timber));

    let answer = call(
        &mut app,
        "entity.add.cube",
        json!({ "name": "Shed", "material": "chimney" }),
    );
    assert_eq!(
        answer["outcome"], "invalid",
        "an unbound name names nothing"
    );
}

#[test]
fn the_palette_survives_a_save_and_reopen() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let scene = {
        let mut app = project_app(tmp.path());
        finished(&call(&mut app, "scene.new", json!({})));
        changed(&call(
            &mut app,
            "material.create",
            json!({ "preset": "shingle", "palette": "roof" }),
        ));
        changed(&call(
            &mut app,
            "palette.set",
            json!({ "palette": "winter", "name": "roof", "material": "materials/shingle.bsn" }),
        ));
        cube(&mut app, "Hut", "roof");
        let scene = tmp.path().join("assets/village.bsn");
        let answer = call(
            &mut app,
            "scene.save",
            json!({ "path": scene.to_string_lossy() }),
        );
        assert_eq!(answer["result"], "finished", "{answer}");
        scene
    };
    assert!(tmp.path().join(PALETTE_FILE).is_file());

    let mut app = project_app(tmp.path());
    let palettes = app.world().resource::<MaterialPalettes>().clone();
    assert_eq!(palettes.active, "default");
    assert_eq!(palettes.bound("roof"), Some("materials/shingle.bsn"));
    assert!(palettes.palettes.contains_key("winter"));

    let answer = call(
        &mut app,
        "scene.open",
        json!({ "path": scene.to_string_lossy() }),
    );
    assert_eq!(answer["result"], "finished", "{answer}");
    let shed = cube(&mut app, "Shed", "roof");
    let shingle = handle_at(&app, "materials/shingle.bsn").id();
    assert!(face_materials(&app, shed).iter().all(|id| *id == shingle));
}

/// The village's ten materials come from one plain batch, later batches name
/// them by palette name, and one call swaps the whole look.
#[test]
fn ten_presets_in_one_batch_then_one_call_swaps_the_look() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut app = project_app(tmp.path());
    finished(&call(&mut app, "scene.new", json!({})));

    let creates: Vec<Value> = PRESETS
        .iter()
        .map(|preset| {
            json!({ "id": "material.create",
                    "params": { "preset": preset.name, "palette": preset.name } })
        })
        .collect();
    let refused = batch(
        &mut app,
        json!({ "atomic": true, "calls": creates.clone() }),
    )
    .expect_err("a batch that must roll back refuses a disk write");
    assert!(
        refused.message.contains("material.create"),
        "{}",
        refused.message
    );

    let answer = batch(&mut app, json!({ "calls": creates })).expect("a plain batch runs");
    for (index, preset) in PRESETS.iter().enumerate() {
        assert_eq!(
            answer["calls"][index]["reports"],
            json!([preset.name]),
            "{answer}"
        );
        assert!(
            tmp.path()
                .join(format!("assets/materials/{}.bsn", preset.name))
                .is_file()
        );
    }

    let answer = batch(
        &mut app,
        json!({ "calls": [
            { "id": "entity.add.cube", "params": { "name": "Wall", "material": "plaster" } },
            { "id": "entity.add.cube", "params": { "name": "Roof", "material": "thatch" } },
            { "id": "entity.add.cube", "params": { "name": "Lamp", "material": "metal" } },
            { "id": "material.create", "params": { "name": "brick", "base_color": "#a0522d" } },
            { "id": "entity.add.cube", "params": { "name": "Kiln", "material": "materials/brick.bsn" } },
        ] }),
    )
    .expect("a plain batch runs");
    let id = |n: usize| Entity::from_bits(answer["calls"][n]["entities"][0].as_u64().expect("id"));
    let (wall, roof, lamp, kiln) = (id(0), id(1), id(2), id(4));

    let mut winter = Vec::new();
    for (slot, preset, color) in [
        ("plaster", "plaster", "#f4f4f8"),
        ("thatch", "thatch", "#e8eef2"),
    ] {
        let name = format!("{slot}_winter");
        winter.push(json!({ "id": "material.create",
            "params": { "name": name, "preset": preset, "base_color": color } }));
        winter.push(json!({ "id": "palette.set",
            "params": { "palette": "winter", "name": slot, "material": format!("materials/{name}.bsn") } }));
    }
    batch(&mut app, json!({ "calls": winter })).expect("a plain batch runs");

    let before = |app: &App| [wall, roof, lamp, kiln].map(|entity| face_materials(app, entity)[0]);
    let original = before(&app);
    let depth = history_len(&app);

    let answer = call(&mut app, "palette.apply", json!({ "palette": "winter" }));
    changed(&answer);
    assert_eq!(history_len(&app), depth + 1, "one undo step");
    let warnings = answer["warnings"].to_string();
    assert!(
        warnings.contains("metal"),
        "unbound names are reported: {answer}"
    );

    let plaster_winter = handle_at(&app, "materials/plaster_winter.bsn").id();
    let thatch_winter = handle_at(&app, "materials/thatch_winter.bsn").id();
    assert!(
        face_materials(&app, wall)
            .iter()
            .all(|id| *id == plaster_winter)
    );
    assert!(
        face_materials(&app, roof)
            .iter()
            .all(|id| *id == thatch_winter)
    );
    assert_eq!(
        face_materials(&app, lamp)[0],
        original[2],
        "winter binds no metal"
    );
    assert_eq!(
        face_materials(&app, kiln)[0],
        original[3],
        "outside the palette"
    );
    assert_eq!(app.world().resource::<MaterialPalettes>().active, "winter");
    let on_disk = std::fs::read_to_string(tmp.path().join(PALETTE_FILE)).expect("palettes");
    assert!(on_disk.contains("\"active\": \"winter\""), "{on_disk}");

    let answer = call(
        &mut app,
        "entity.add.cube",
        json!({ "name": "Barn", "material": "thatch" }),
    );
    let barn = Entity::from_bits(answer["entities"][0].as_u64().expect("id"));
    assert_eq!(
        face_materials(&app, barn)[0],
        thatch_winter,
        "names follow the active palette"
    );
    undo(&mut app);

    undo(&mut app);
    assert_eq!(before(&app), original, "undo takes the whole look back");
    assert_eq!(app.world().resource::<MaterialPalettes>().active, "default");
    redo(&mut app);
    assert_eq!(face_materials(&app, wall)[0], plaster_winter);

    let answer = call(&mut app, "palette.apply", json!({ "palette": "winter" }));
    assert_eq!(answer["outcome"], "unchanged", "{answer}");
    let answer = call(&mut app, "palette.apply", json!({ "palette": "summer" }));
    assert_eq!(answer["outcome"], "invalid", "{answer}");
}

#[test]
fn an_alpha_wins_over_the_colour_whatever_order_the_fields_come_in() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut app = project_app(tmp.path());
    changed(&call(
        &mut app,
        "material.create",
        json!({ "name": "pane" }),
    ));
    changed(&call(
        &mut app,
        "material.set",
        json!({ "material": "materials/pane.bsn",
                "fields": { "base_color": "#336699", "alpha": 0.3 } }),
    ));
    let pane = material(&app, "materials/pane.bsn");
    assert_eq!(pane.base_color.alpha(), 0.3);
    assert_eq!(pane.alpha_mode, AlphaMode::Blend);

    let answer = call(
        &mut app,
        "material.create",
        json!({ "preset": "glass", "base_color": "#a0d0ff" }),
    );
    changed(&answer);
    let glass = material(&app, "materials/glass.bsn");
    assert_eq!(glass.base_color.alpha(), 0.3, "the preset's alpha stays");
    assert_eq!(glass.alpha_mode, AlphaMode::Blend);
    assert_eq!(
        glass.base_color.with_alpha(1.0),
        Color::from(Srgba::hex("#a0d0ff").expect("hex"))
    );

    changed(&call(
        &mut app,
        "material.create",
        json!({ "name": "veil", "base_color": "#ffffff", "alpha": 0.5 }),
    ));
    assert_eq!(material(&app, "materials/veil.bsn").base_color.alpha(), 0.5);
}

#[test]
fn undo_of_material_set_keeps_later_edits_to_other_fields() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut app = project_app(tmp.path());
    changed(&call(
        &mut app,
        "material.create",
        json!({ "name": "clay", "roughness": 0.6 }),
    ));
    changed(&call(
        &mut app,
        "material.set",
        json!({ "material": "materials/clay.bsn", "field": "roughness", "value": 0.2 }),
    ));
    let handle = handle_at(&app, "materials/clay.bsn");
    if let Some(mut clay) = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .get_mut(&handle)
    {
        clay.metallic = 0.9;
    }
    undo(&mut app);
    let clay = material(&app, "materials/clay.bsn");
    assert_eq!(clay.perceptual_roughness, 0.6, "the call's field goes back");
    assert_eq!(clay.metallic, 0.9, "a later edit to another field survives");

    let depth = history_len(&app);
    let answer = call(
        &mut app,
        "material.set",
        json!({ "material": "materials/clay.bsn", "fields": { "roughness": 0.6, "metallic": 0.9 } }),
    );
    assert_eq!(answer["outcome"], "unchanged", "{answer}");
    assert_eq!(history_len(&app), depth, "nothing to undo");
}

#[test]
fn a_new_material_binds_its_own_file_even_when_the_name_is_taken_in_the_palette() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut app = project_app(tmp.path());
    changed(&call(
        &mut app,
        "material.create",
        json!({ "preset": "plaster", "palette": "wall" }),
    ));
    changed(&call(
        &mut app,
        "material.create",
        json!({ "name": "wall", "preset": "stone", "palette": "wall" }),
    ));
    assert_eq!(
        app.world().resource::<MaterialPalettes>().bound("wall"),
        Some("materials/wall.bsn")
    );
}

#[test]
fn a_renamed_material_keeps_its_palette_names_and_a_stale_one_is_skipped() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut app = project_app(tmp.path());
    finished(&call(&mut app, "scene.new", json!({})));
    changed(&call(
        &mut app,
        "material.create",
        json!({ "name": "clay", "palette": "wall" }),
    ));
    changed(&call(
        &mut app,
        "material.create",
        json!({ "preset": "thatch", "palette": "roof" }),
    ));
    finished(&call(
        &mut app,
        "material.save",
        json!({ "material": "materials/clay.bsn", "name": "loam" }),
    ));
    assert_eq!(
        app.world().resource::<MaterialPalettes>().bound("wall"),
        Some("materials/loam.bsn"),
        "the binding follows the rename"
    );

    let wall = cube(&mut app, "Wall", "wall");
    let roof = cube(&mut app, "Roof", "roof");
    changed(&call(
        &mut app,
        "material.create",
        json!({ "name": "snow" }),
    ));
    changed(&call(
        &mut app,
        "palette.set",
        json!({ "palette": "winter", "name": "roof", "material": "materials/snow.bsn" }),
    ));
    app.world_mut()
        .resource_mut::<MaterialPalettes>()
        .palettes
        .get_mut("winter")
        .expect("winter")
        .insert("wall".into(), "materials/gone.bsn".into());
    let wall_before = face_materials(&app, wall)[0];

    let answer = call(&mut app, "palette.apply", json!({ "palette": "winter" }));
    changed(&answer);
    assert!(answer["warnings"].to_string().contains("wall"), "{answer}");
    let snow = handle_at(&app, "materials/snow.bsn").id();
    assert_eq!(face_materials(&app, roof)[0], snow);
    assert_eq!(face_materials(&app, wall)[0], wall_before);
}

#[test]
fn an_unreadable_palette_file_is_left_alone() {
    let tmp = tempfile::tempdir().expect("tempdir");
    drop(project_app(tmp.path()));
    let file = tmp.path().join(PALETTE_FILE);
    std::fs::write(&file, "{ not json").expect("write");
    let mut app = project_app(tmp.path());
    changed(&call(
        &mut app,
        "material.create",
        json!({ "name": "clay" }),
    ));

    let answer = call(
        &mut app,
        "palette.set",
        json!({ "name": "wall", "material": "materials/clay.bsn" }),
    );
    assert_eq!(answer["outcome"], "invalid", "{answer}");
    assert!(
        answer["warnings"].to_string().contains("could not be read"),
        "{answer}"
    );
    assert_eq!(std::fs::read_to_string(&file).expect("read"), "{ not json");
    let status = app
        .world_mut()
        .run_system_cached_with(status_handler, None)
        .expect("ran")
        .expect("status");
    assert!(!status["palette"]["error"].is_null(), "{status}");

    std::fs::write(&file, "{}").expect("write");
    changed(&call(
        &mut app,
        "palette.set",
        json!({ "name": "wall", "material": "materials/clay.bsn" }),
    ));
    let status = app
        .world_mut()
        .run_system_cached_with(status_handler, None)
        .expect("ran")
        .expect("status");
    assert_eq!(
        status["palette"]["palettes"]["default"]["wall"], "materials/clay.bsn",
        "{status}"
    );
}

#[cfg(unix)]
#[test]
fn a_palette_switch_that_cannot_be_written_recolours_nothing() {
    use std::os::unix::fs::PermissionsExt as _;
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut app = project_app(tmp.path());
    finished(&call(&mut app, "scene.new", json!({})));
    changed(&call(
        &mut app,
        "material.create",
        json!({ "preset": "thatch", "palette": "roof" }),
    ));
    changed(&call(
        &mut app,
        "material.create",
        json!({ "name": "snow" }),
    ));
    changed(&call(
        &mut app,
        "palette.set",
        json!({ "palette": "winter", "name": "roof", "material": "materials/snow.bsn" }),
    ));
    let roof = cube(&mut app, "Roof", "roof");
    let before = face_materials(&app, roof);
    let depth = history_len(&app);
    let file = tmp.path().join(PALETTE_FILE);
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).expect("chmod");

    let answer = call(&mut app, "palette.apply", json!({ "palette": "winter" }));
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).expect("chmod");
    assert_eq!(answer["outcome"], "invalid", "{answer}");
    assert_eq!(face_materials(&app, roof), before);
    assert_eq!(history_len(&app), depth);
    assert_eq!(app.world().resource::<MaterialPalettes>().active, "default");
}
