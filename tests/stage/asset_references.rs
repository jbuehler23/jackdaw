//! References between assets are paths.
//!
//! A scene, a prefab or an asset that points at another asset spells the path
//! of the file it points at. The bare names written before paths still resolve
//! while projects catch up, and a reference the editor cannot resolve at all is
//! kept rather than blanked.

use crate::util;

use std::path::{Path, PathBuf};

use bevy::asset::{Asset, Assets};
use bevy::prelude::*;
use jackdaw::asset_index::{AssetIndex, AssetValue};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_scene_types::PropertyValue;

/// An asset of the project's own with a handle field, which is what a game's
/// outfits, mobs and items look like.
#[derive(Asset, Reflect, Clone, Default)]
#[reflect(Default)]
struct OutfitDef {
    material: Handle<StandardMaterial>,
}

/// A component holding one material, so a scene can spell a reference without
/// a brush's geometry in the way.
#[derive(Component, Reflect, Clone, Default)]
#[reflect(Component, Default)]
struct Painted {
    material: Handle<StandardMaterial>,
}

fn editor_with_outfits() -> (App, tempfile::TempDir) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let mut app = util::editor_test_app();
    app.init_asset::<OutfitDef>();
    app.register_asset_reflect::<OutfitDef>();
    app.register_type::<OutfitDef>();
    app.register_type::<Painted>();
    app.world_mut()
        .insert_resource(jackdaw::project::ProjectRoot {
            root: tmp.path().to_path_buf(),
            config: default(),
        });
    app.world_mut()
        .resource_mut::<AssetKinds>()
        .register(AssetKind::extension(
            "outfit",
            "Outfit",
            OutfitDef::type_path(),
        ));
    app.world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    app.update();
    (app, tmp)
}

#[track_caller]
fn call(app: &mut App, id: &'static str, params: &[(&'static str, PropertyValue)]) {
    let mut call = app.world_mut().operator(id).settings(CallOperatorSettings {
        execution_context: ExecutionContext::Invoke,
        creates_history_entry: true,
    });
    for (key, value) in params {
        call = call.param(*key, value.clone());
    }
    let result = call.call().expect("the operator dispatched");
    assert_eq!(result, OperatorResult::Finished, "{id} did not finish");
    app.update();
}

/// A material of the project's own, filed where the caller asks for it, as the
/// index holds it once the walk has read the file back.
fn file_material(app: &mut App, relative: &str) -> Handle<StandardMaterial> {
    let handle = app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            metallic: 0.5,
            ..default()
        });
    let root = app
        .world()
        .resource::<jackdaw::project::ProjectRoot>()
        .root
        .clone();
    let path = root.join("assets").join(relative);
    std::fs::create_dir_all(path.parent().expect("a parent")).expect("the directory is made");
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.split('.').next())
        .expect("a name");
    jackdaw::definition_assets::write_asset_file(
        app.world(),
        name,
        &AssetValue::Handle(handle.clone().untyped()),
        &path,
    )
    .expect("the material file is written");
    jackdaw::asset_index::rescan_asset_index(app.world_mut());
    app.update();
    app.world()
        .resource::<AssetIndex>()
        .get(Path::new(relative))
        .and_then(|entry| entry.value.handle())
        .expect("the walk indexed the material")
        .clone()
        .typed::<StandardMaterial>()
}

/// Write a scene naming one material, open it, and emit it again.
fn round_trip_scene(app: &mut App, tmp: &tempfile::TempDir, reference: &str) -> String {
    let scene = tmp.path().join("assets/zone.bsn");
    std::fs::create_dir_all(scene.parent().expect("a parent")).expect("the directory is made");
    std::fs::write(
        &scene,
        format!("{} {{ material: \"{reference}\" }}\n", Painted::type_path()),
    )
    .expect("the scene is written");

    jackdaw::scene_io::load_scene_from_file(app.world_mut(), &scene);
    app.update();
    jackdaw::scene_io::emit_bsn_scene_with_inline_assets(
        app.world_mut(),
        scene.parent().expect("a parent"),
    )
}

fn painted_material(app: &mut App) -> Handle<StandardMaterial> {
    let world = app.world_mut();
    let mut query = world.query::<&Painted>();
    query
        .iter(world)
        .next()
        .expect("the scene spawned its painted entity")
        .material
        .clone()
}

#[test]
fn a_scene_naming_a_material_by_name_saves_it_as_a_path() {
    let (mut app, tmp) = editor_with_outfits();
    let filed = file_material(&mut app, "materials/slate.material.bsn");

    let text = round_trip_scene(&mut app, &tmp, "@slate");

    assert_eq!(
        painted_material(&mut app),
        filed,
        "the name resolves to the file the index holds"
    );
    assert!(
        text.contains("materials/slate.material.bsn"),
        "the save spells the path:\n{text}"
    );
    assert!(!text.contains("@slate"), "got:\n{text}");
}

#[test]
fn a_scene_referencing_an_asset_outside_the_materials_folder_round_trips() {
    let (mut app, tmp) = editor_with_outfits();
    let filed = file_material(&mut app, "content/ground/slate.bsn");

    let text = round_trip_scene(&mut app, &tmp, "content/ground/slate.bsn");

    assert_eq!(
        painted_material(&mut app),
        filed,
        "a path outside the materials folder names the file it points at"
    );
    assert!(
        text.contains("content/ground/slate.bsn"),
        "the save spells the same path:\n{text}"
    );
}

#[test]
fn a_name_two_files_share_is_kept_rather_than_pointed_at_either() {
    let (mut app, tmp) = editor_with_outfits();
    let filed = file_material(&mut app, "materials/slate.material.bsn");
    let other = file_material(&mut app, "content/slate.material.bsn");
    assert_ne!(filed, other, "two files, two materials");

    let text = round_trip_scene(&mut app, &tmp, "@slate");

    let painted = painted_material(&mut app);
    assert!(
        painted != filed && painted != other,
        "an ambiguous name names neither file"
    );
    assert!(
        text.contains("@slate"),
        "and the save keeps what the file said:\n{text}"
    );
}

#[test]
fn a_reference_the_editor_cannot_resolve_survives_the_save() {
    let (mut app, tmp) = editor_with_outfits();

    let text = round_trip_scene(&mut app, &tmp, "@ghost");

    assert!(
        text.contains("@ghost"),
        "what the file said is kept rather than blanked:\n{text}"
    );
}

#[test]
fn asset_set_takes_a_material_path_and_still_takes_a_bare_name() {
    let (mut app, _tmp) = editor_with_outfits();
    let filed = file_material(&mut app, "materials/slate.material.bsn");

    call(
        &mut app,
        "asset.new",
        &[("type", "outfit".into()), ("name", "ranger".into())],
    );
    call(
        &mut app,
        "asset.set",
        &[
            ("field", "material".into()),
            ("value", "materials/slate.material.bsn".into()),
        ],
    );
    assert_eq!(
        open_outfit(&app).material,
        filed,
        "a path assigns the handle the index loaded"
    );

    call(
        &mut app,
        "asset.set",
        &[
            ("field", "material".into()),
            ("value", String::new().into()),
        ],
    );
    call(
        &mut app,
        "asset.set",
        &[("field", "material".into()), ("value", "slate".into())],
    );
    assert_eq!(
        open_outfit(&app).material,
        filed,
        "and a bare name still resolves to the same file"
    );

    call(&mut app, "asset.save", &[]);
    let written = std::fs::read_to_string(outfit_path(&app, "ranger.bsn")).expect("the file reads");
    assert!(
        written.contains("materials/slate.material.bsn"),
        "the asset file spells the path:\n{written}"
    );
}

#[test]
fn undo_takes_a_material_field_back_to_the_path_it_held() {
    let (mut app, _tmp) = editor_with_outfits();
    let slate = file_material(&mut app, "materials/slate.material.bsn");
    let moss = file_material(&mut app, "materials/moss.material.bsn");

    call(
        &mut app,
        "asset.new",
        &[("type", "outfit".into()), ("name", "ranger".into())],
    );
    call(
        &mut app,
        "asset.set",
        &[
            ("field", "material".into()),
            ("value", "materials/slate.material.bsn".into()),
        ],
    );
    call(
        &mut app,
        "asset.set",
        &[
            ("field", "material".into()),
            ("value", "materials/moss.material.bsn".into()),
        ],
    );
    assert_eq!(open_outfit(&app).material, moss);

    app.world_mut().resource_scope(
        |world, mut history: Mut<jackdaw_commands::CommandHistory>| {
            history.undo(world);
        },
    );

    assert_eq!(
        open_outfit(&app).material,
        slate,
        "undo takes the field back to the path it held before"
    );
}

#[test]
fn material_apply_takes_the_path_of_a_material_file() {
    use jackdaw::brush::{BrushEditMode, BrushSelection, EditMode};
    use jackdaw::selection::Selection;
    use jackdaw_scene_types::Brush;

    let (mut app, _tmp) = editor_with_outfits();
    let filed = file_material(&mut app, "materials/slate.material.bsn");
    let brush = app
        .world_mut()
        .spawn((
            Name::new("Wall"),
            Brush::cuboid(0.5, 0.5, 0.5),
            Transform::default(),
            Visibility::default(),
        ))
        .id();
    app.world_mut().resource_mut::<Selection>().entities = vec![brush];
    *app.world_mut().resource_mut::<EditMode>() = EditMode::BrushEdit(BrushEditMode::Face);
    {
        let mut selection = app.world_mut().resource_mut::<BrushSelection>();
        selection.active_brush = Some(brush);
        selection.sub_mut(brush).faces = vec![0];
    }
    app.update();

    call(
        &mut app,
        "material.apply",
        &[("material", "materials/slate.material.bsn".into())],
    );

    assert_eq!(
        app.world().get::<Brush>(brush).expect("the brush").faces[0]
            .material
            .clone(),
        filed,
        "the face draws the file the path names"
    );
}

#[test]
fn a_reference_resolves_to_the_material_it_names_after_that_file_turns_binary() {
    let (mut app, tmp) = editor_with_outfits();
    file_material(&mut app, "materials/slate.material.bsn");
    let filed = tmp.path().join("assets/materials/slate.material.bsn");

    call(
        &mut app,
        "file.convert_to_binary",
        &[("path", filed.to_string_lossy().into_owned().into())],
    );
    jackdaw::asset_index::rescan_asset_index(app.world_mut());
    app.update();

    assert!(!filed.exists(), "the text file gave way to its twin");
    assert!(
        tmp.path()
            .join("assets/materials/slate.material.bsb")
            .exists()
    );
    assert!(
        round_trip_scene(&mut app, &tmp, "materials/slate.material.bsn")
            .contains("materials/slate.material.bsn"),
        "the reference the scene was written with still names the asset it found"
    );
}

#[test]
fn converting_a_document_a_tab_has_open_is_refused() {
    let (mut app, tmp) = editor_with_outfits();
    file_material(&mut app, "materials/slate.material.bsn");
    round_trip_scene(&mut app, &tmp, "materials/slate.material.bsn");
    let open = tmp.path().join("assets/zone.bsn");
    app.world_mut()
        .resource_mut::<jackdaw::scenes::Scenes>()
        .push_tab(jackdaw::scenes::SceneTab {
            path: Some(open.clone()),
            display_name: "zone".to_string(),
            dirty: false,
            kind: jackdaw::scenes::TabKind::Scene,
            content: jackdaw::scenes::TabContent::Scene(None),
            view_state: jackdaw::scenes::ViewState::default(),
            history: jackdaw::commands::CommandHistory::default(),
            terrain_data_store: jackdaw::terrain::TerrainDataStore::default(),
            navmesh: jackdaw::terrain::navmesh_bake::TabNavmesh::default(),
            history_depth_at_last_check: 0,
            refusal: None,
        });

    let result = app
        .world_mut()
        .operator("file.convert_to_binary")
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: true,
        })
        .param("path", open.to_string_lossy().into_owned())
        .call()
        .expect("the operator dispatched");
    app.update();

    assert_eq!(result, OperatorResult::Cancelled);
    assert!(
        open.exists(),
        "the file the tab is written back to is still there"
    );
    assert!(!tmp.path().join("assets/zone.bsb").exists());
}

fn open_outfit(app: &App) -> OutfitDef {
    let value =
        jackdaw::definition_assets::open_definition_value(app.world()).expect("an asset is open");
    let handle = value.handle().expect("a compiled asset").clone();
    app.world()
        .resource::<Assets<OutfitDef>>()
        .get(&handle.typed::<OutfitDef>())
        .expect("the asset is in its store")
        .clone()
}

fn outfit_path(app: &App, relative: &str) -> PathBuf {
    let root = &app.world().resource::<jackdaw::project::ProjectRoot>().root;
    root.join("assets").join(Path::new(relative))
}
