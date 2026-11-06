//! A field edit writes that field: it leaves the assets the rest of the
//! component holds alone, on the live entity and in the saved scene, through
//! undo and redo.

use crate::util;
use crate::util::OperatorResultExt as _;

use bevy::prelude::*;
use jackdaw::migrate_dialog::request_open_with_conversion;
use jackdaw::pie_projection::PieProjection;
use jackdaw::project::{ProjectConfig, ProjectRoot};
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::{CallOperatorSettings, ExecutionContext};
use jackdaw_bsn::{
    BsnPatch, BsnSceneAssets, BsnValue, SceneBsnAst, get_bsn_field, holds_no_asset, is_placeholder,
};
use jackdaw_scene_types::types::Brush;

const BRUSH: &str = "jackdaw_scene_types::types::Brush";
const SPRITE: &str = "bevy_sprite::sprite::Sprite";

const SOIL: &str = "materials/soil.bsn";
const MOSS: &str = "materials/moss.bsn";
const SAVED: [&str; 6] = [SOIL, MOSS, MOSS, SOIL, SOIL, MOSS];

/// A component with nothing to do with the brush, for the operators that add
/// and remove one on the entity that holds it.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Marked {
    weight: f32,
}

fn material_file(roughness: f32) -> String {
    format!(
        "// jackdaw asset bevy_pbr::pbr_material::StandardMaterial\n\
         #material\n\
         bevy_pbr::pbr_material::StandardMaterial {{\n    \
         perceptual_roughness: {roughness:?},\n}}\n"
    )
}

fn scene_text() -> String {
    scene_text_naming(SAVED)
}

fn scene_text_naming(materials: [&str; 6]) -> String {
    let normals = [
        (1.0, 0.0, 0.0),
        (-1.0, 0.0, 0.0),
        (0.0, 1.0, 0.0),
        (0.0, -1.0, 0.0),
        (0.0, 0.0, 1.0),
        (0.0, 0.0, -1.0),
    ];
    let faces: String = normals
        .iter()
        .zip(materials)
        .map(|((x, y, z), material)| {
            format!(
                "        jackdaw_geometry::BrushFaceData {{\n            \
                 plane: jackdaw_geometry::BrushPlane {{\n                \
                 normal: glam::Vec3 {{ x: {x:?}, y: {y:?}, z: {z:?} }},\n                \
                 distance: 1.0,\n            }},\n            \
                 material: \"{material}\",\n        }},\n"
            )
        })
        .collect();
    format!(
        "#Block\n\
         bevy_transform::components::transform::Transform\n\
         jackdaw_scene_types::types::Brush {{\n    faces: [\n{faces}    ],\n}}\n"
    )
}

struct Opened {
    app: App,
    dir: tempfile::TempDir,
    brush: Entity,
}

fn open_scene() -> Opened {
    open_scene_naming(SAVED)
}

/// A scene whose brush spells `materials`, face by face, in the editor state.
fn open_scene_naming(materials: [&str; 6]) -> Opened {
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(&dir, &scene_text_naming(materials));
    open_project(dir)
}

fn write_project(dir: &tempfile::TempDir, scene: &str) {
    let assets = dir.path().join("assets");
    std::fs::create_dir_all(assets.join("materials")).expect("the folder is made");
    std::fs::write(assets.join(SOIL), material_file(0.25)).expect("a material");
    std::fs::write(assets.join(MOSS), material_file(0.75)).expect("a material");
    std::fs::write(assets.join("scene.bsn"), scene).expect("the scene is written");
}

/// Open the project's scene in a new editor, in the editor state.
fn open_project(dir: tempfile::TempDir) -> Opened {
    let mut opened = open_project_before_sync(dir);
    opened
        .app
        .world_mut()
        .resource_mut::<NextState<jackdaw::AppState>>()
        .set(jackdaw::AppState::Editor);
    opened.app.update();
    opened.app.update();
    opened
}

/// The scene just opened. The sync of live brushes into the document runs in
/// the editor state, so this document still names what the file names.
fn open_scene_before_sync() -> Opened {
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(&dir, &scene_text());
    open_project_before_sync(dir)
}

fn open_project_before_sync(dir: tempfile::TempDir) -> Opened {
    let assets = dir.path().join("assets");
    let mut app = util::editor_test_app_reading(&assets);
    app.insert_resource(ProjectRoot::new(
        dir.path().to_path_buf(),
        ProjectConfig::default(),
    ));
    request_open_with_conversion(app.world_mut(), &assets.join("scene.bsn"));
    util::settle_scene_load(&mut app);
    app.update();
    let brush = app
        .world_mut()
        .query_filtered::<Entity, With<Brush>>()
        .iter(app.world())
        .next()
        .expect("the brush spawned");
    Opened { app, dir, brush }
}

impl Opened {
    /// The brushes in the scene now.
    fn brushes(&mut self) -> Vec<Entity> {
        self.app
            .world_mut()
            .query_filtered::<Entity, With<Brush>>()
            .iter(self.app.world())
            .collect()
    }

    /// Point at the one brush the scene holds, after a rebuild replaced it.
    fn refind_brush(&mut self) {
        let brushes = self.brushes();
        assert_eq!(brushes.len(), 1, "one brush in the scene");
        self.brush = brushes[0];
    }

    fn select_brush(&mut self) {
        let brush = self.brush;
        self.app.world_mut().resource_mut::<Selection>().entities = vec![brush];
    }

    /// Run a selection operator as the keybind runs it, as one history entry.
    fn invoke(&mut self, id: &str) {
        self.app
            .world_mut()
            .operator(id.to_string())
            .settings(CallOperatorSettings {
                execution_context: ExecutionContext::Invoke,
                creates_history_entry: true,
            })
            .call()
            .expect("the operator dispatches")
            .assert_finished();
        self.app.update();
    }

    fn component_op(&mut self, id: &str, type_path: &str) {
        self.select_brush();
        let brush = self.brush;
        self.app
            .world_mut()
            .operator(id.to_string())
            .param("entity", brush)
            .param("type_path", type_path.to_string())
            .call()
            .expect("the component operator dispatches")
            .assert_finished();
        self.app.update();
    }

    /// What the live document holds for a face's material.
    fn document_material(&self, face: usize) -> Option<BsnValue> {
        let ast = self.app.world().resource::<SceneBsnAst>();
        let node = ast
            .ast_for(self.brush)
            .expect("the brush is in the document");
        get_bsn_field(ast, node, BRUSH, &format!("faces[{face}].material"))
    }

    /// Change the live brush directly, as a mutator outside the commands does.
    fn mutate_brush(&mut self, edit: impl FnOnce(&mut Brush)) {
        let mut brush = self
            .app
            .world_mut()
            .get_mut::<Brush>(self.brush)
            .expect("the brush is live");
        edit(&mut brush);
        self.app.update();
        self.app.update();
    }

    fn set(&mut self, type_path: &str, target: Entity, field: &str, value: &str) {
        self.app
            .world_mut()
            .operator("component.set")
            .param("entity", target)
            .param("type_path", type_path.to_string())
            .param("field", field.to_string())
            .param("value", value.to_string())
            .call()
            .expect("component.set dispatches")
            .assert_finished();
        self.app.update();
    }

    fn set_face(&mut self, field: &str, value: &str) {
        self.set(BRUSH, self.brush, field, value);
    }

    fn history(&mut self, id: &str) {
        self.app
            .world_mut()
            .operator(id.to_string())
            .call()
            .expect("the history operator dispatches")
            .assert_finished();
        self.app.update();
    }

    fn live(&self) -> Brush {
        self.app
            .world()
            .get::<Brush>(self.brush)
            .expect("the brush is live")
            .clone()
    }

    fn handles(&self) -> Vec<Handle<StandardMaterial>> {
        self.live().faces.into_iter().map(|f| f.material).collect()
    }

    fn save(&mut self) -> String {
        let path = self.dir.path().join("assets/scene.bsn");
        self.app
            .world_mut()
            .operator("scene.save")
            .param("path", path.to_string_lossy().into_owned())
            .call()
            .expect("scene.save dispatches")
            .assert_finished();
        self.app.update();
        std::fs::read_to_string(path).expect("the saved scene")
    }

    /// A save, then the materials it wrote on faces, face by face.
    fn saved_materials(&mut self) -> Vec<String> {
        materials_in(&self.save())
    }
}

fn materials_in(text: &str) -> Vec<String> {
    text.split("material: \"")
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .map(str::to_string)
        .collect()
}

fn assert_kept(opened: &mut Opened, handles: &[Handle<StandardMaterial>], moment: &str) {
    assert_eq!(opened.handles(), handles, "live handles {moment}");
    let saved = opened.saved_materials();
    assert_eq!(saved.len(), SAVED.len(), "{moment}: saved faces {saved:?}");
    for (written, expected) in saved.iter().zip(SAVED) {
        assert!(
            written.ends_with(expected.rsplit('/').next().unwrap_or(expected)),
            "{moment}: a face saved {written:?}, not {expected:?}, in {saved:?}"
        );
    }
    let stored = opened.app.world().resource::<Assets<StandardMaterial>>();
    assert!(
        handles.iter().all(|handle| stored.contains(handle)),
        "{moment}: every face's material is still in the store"
    );
}

#[test]
fn a_uv_scale_edit_keeps_every_material_through_undo_and_redo() {
    let mut opened = open_scene();
    let handles = opened.handles();
    assert!(
        handles.iter().all(|handle| *handle != Handle::default()),
        "the scene opened with its materials"
    );
    let before = opened.live().faces[0].uv_scale;

    opened.set_face("faces[0].uv_scale", "[2.0,3.0]");
    assert_eq!(opened.live().faces[0].uv_scale, Vec2::new(2.0, 3.0));
    assert_kept(&mut opened, &handles, "after the set");
    let saved = opened.save();
    assert!(
        saved.contains("x: 2.0") && saved.contains("y: 3.0"),
        "the edited value is saved:\n{saved}"
    );

    opened.history("history.undo");
    assert_eq!(opened.live().faces[0].uv_scale, before);
    assert_kept(&mut opened, &handles, "after undo");
    let saved = opened.save();
    assert!(
        !saved.contains("x: 2.0") && !saved.contains("y: 3.0"),
        "undo takes the edited value out of the save:\n{saved}"
    );

    opened.history("history.redo");
    assert_eq!(opened.live().faces[0].uv_scale, Vec2::new(2.0, 3.0));
    assert_kept(&mut opened, &handles, "after redo");
    let saved = opened.save();
    assert!(
        saved.contains("x: 2.0") && saved.contains("y: 3.0"),
        "redo puts the edited value back in the save:\n{saved}"
    );
}

#[test]
fn a_uv_rotation_edit_keeps_every_material_through_undo_and_redo() {
    let mut opened = open_scene();
    let handles = opened.handles();
    opened.set_face("faces[1].uv_rotation", "0.5");
    assert_eq!(opened.live().faces[1].uv_rotation, 0.5);
    assert_kept(&mut opened, &handles, "after the set");
    assert!(opened.save().contains("uv_rotation: 0.5"));

    opened.history("history.undo");
    assert_eq!(opened.live().faces[1].uv_rotation, 0.0);
    assert_kept(&mut opened, &handles, "after undo");
    assert!(!opened.save().contains("uv_rotation: 0.5"));

    opened.history("history.redo");
    assert_eq!(opened.live().faces[1].uv_rotation, 0.5);
    assert_kept(&mut opened, &handles, "after redo");
}

#[test]
fn a_second_edit_of_the_same_brush_keeps_every_material() {
    let mut opened = open_scene();
    let handles = opened.handles();
    opened.set_face("faces[0].uv_scale", "[2.0,3.0]");
    opened.set_face("faces[1].uv_offset", "[0.5,0.25]");
    assert_kept(&mut opened, &handles, "after two edits");
    opened.history("history.undo");
    opened.history("history.undo");
    assert_kept(&mut opened, &handles, "after undoing both");
    opened.history("history.redo");
    opened.history("history.redo");
    assert_kept(&mut opened, &handles, "after redoing both");
}

#[test]
fn setting_a_field_to_what_it_holds_keeps_every_material() {
    let mut opened = open_scene();
    let handles = opened.handles();
    opened.set_face("faces[0].uv_scale", "[2.0,3.0]");
    opened.set_face("faces[0].uv_scale", "[2.0,3.0]");
    assert_kept(&mut opened, &handles, "after setting the same value");
}

#[test]
fn undoing_a_material_edit_puts_the_previous_material_back() {
    let mut opened = open_scene();
    let handles = opened.handles();
    opened.set_face("faces[1].uv_rotation", "0.5");

    let stone = {
        let world = opened.app.world_mut();
        let stone = world
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let mut assets = world.get_resource_or_init::<BsnSceneAssets>();
        assets
            .0
            .insert("#Stone".to_string(), stone.clone().untyped());
        stone
    };
    opened.set_face("faces[0].material", "\"#Stone\"");
    let mut edited = handles.clone();
    edited[0] = stone;
    assert_eq!(opened.handles(), edited, "the edit lands on its face alone");

    opened.history("history.undo");
    assert_kept(&mut opened, &handles, "after undoing the material");

    opened.history("history.redo");
    assert_eq!(opened.handles(), edited, "redo puts the edit back");
    assert_eq!(opened.saved_materials().len(), SAVED.len());
}

/// A list written whole is what the faces hold from then on: an empty one has
/// no faces, and undo brings back the list with its materials.
#[test]
fn writing_the_face_list_whole_replaces_it_and_undo_brings_every_material_back() {
    let mut opened = open_scene();
    let handles = opened.handles();
    opened.set_face("faces", "[]");
    assert!(opened.live().faces.is_empty(), "the list took the value");
    assert_eq!(opened.saved_materials(), Vec::<String>::new());
    opened.history("history.undo");
    assert_kept(&mut opened, &handles, "after undo");
}

/// A whole component write that empties the brush undoes back to its materials.
#[test]
fn undoing_a_whole_component_write_puts_the_materials_back() {
    let mut opened = open_scene();
    let handles = opened.handles();
    opened.set_face("", "{\"faces\":[]}");
    assert!(
        opened.live().faces.is_empty(),
        "the component took the value"
    );
    opened.history("history.undo");
    assert_kept(&mut opened, &handles, "after undo");
    opened.history("history.redo");
    assert!(opened.live().faces.is_empty(), "redo empties it again");
}

/// A field the document does not hold yet undoes to what the entity held, even
/// when the type's default has no such field: before a first frame syncs the
/// live brush, the opened document spells only what the file spells.
#[test]
fn undoing_a_field_the_document_never_spelled_restores_the_live_value() {
    let mut opened = open_scene_before_sync();
    let handles = opened.handles();
    opened.set_face("faces[0].uv_scale", "[2.0,3.0]");
    opened.history("history.undo");
    assert_eq!(opened.live().faces[0].uv_scale, Vec2::ZERO);
    assert_eq!(opened.handles(), handles);
}

#[test]
fn a_sprite_colour_edit_keeps_its_image_through_undo_and_redo() {
    let mut opened = open_scene();
    let image = opened
        .app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::default());
    let sprite = {
        let world = opened.app.world_mut();
        let entity = world.spawn(Sprite::from_image(image.clone())).id();
        jackdaw::scene_io::register_entity_in_ast(world, entity);
        entity
    };
    opened.app.update();
    let held = |opened: &Opened| {
        opened
            .app
            .world()
            .get::<Sprite>(sprite)
            .expect("the sprite")
            .clone()
    };

    opened.set(SPRITE, sprite, "flip_x", "true");
    assert!(held(&opened).flip_x);
    assert_eq!(held(&opened).image, image, "after the set");

    opened.history("history.undo");
    assert!(!held(&opened).flip_x);
    assert_eq!(held(&opened).image, image, "after undo");

    opened.history("history.redo");
    assert!(held(&opened).flip_x);
    assert_eq!(held(&opened).image, image, "after redo");
}

#[test]
fn a_sprite_atlas_index_edit_keeps_its_layout() {
    let mut opened = open_scene();
    let layout = opened
        .app
        .world_mut()
        .resource_mut::<Assets<TextureAtlasLayout>>()
        .add(TextureAtlasLayout::new_empty(UVec2::splat(16)));
    let sprite = {
        let world = opened.app.world_mut();
        let entity = world
            .spawn(Sprite::from_atlas_image(
                Handle::default(),
                TextureAtlas {
                    layout: layout.clone(),
                    index: 0,
                },
            ))
            .id();
        jackdaw::scene_io::register_entity_in_ast(world, entity);
        entity
    };
    opened.app.update();
    let held = |opened: &Opened| {
        opened
            .app
            .world()
            .get::<Sprite>(sprite)
            .and_then(|sprite| sprite.texture_atlas.clone())
            .expect("the atlas")
    };

    opened.set(SPRITE, sprite, "texture_atlas.0.index", "3");
    assert_eq!(held(&opened).index, 3);
    assert_eq!(held(&opened).layout, layout, "after the set");
    opened.history("history.undo");
    assert_eq!(held(&opened).index, 0);
    assert_eq!(held(&opened).layout, layout, "after undo");
    opened.history("history.redo");
    assert_eq!(held(&opened).index, 3);
    assert_eq!(held(&opened).layout, layout, "after redo");
}

#[test]
fn clearing_a_sprite_image_undoes_back_to_it() {
    let mut opened = open_scene();
    let image = opened
        .app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::default());
    opened
        .app
        .world_mut()
        .resource_mut::<BsnSceneAssets>()
        .0
        .insert("#Hero".to_string(), image.clone().untyped());
    let sprite = {
        let world = opened.app.world_mut();
        let entity = world.spawn(Sprite::from_image(image.clone())).id();
        jackdaw::scene_io::register_entity_in_ast(world, entity);
        entity
    };
    opened.app.update();
    opened.set(SPRITE, sprite, "flip_x", "true");
    let held = |opened: &Opened| {
        opened
            .app
            .world()
            .get::<Sprite>(sprite)
            .expect("the sprite")
            .image
            .clone()
    };

    opened.set(SPRITE, sprite, "image", "\"\"");
    assert_eq!(held(&opened), Handle::default(), "the field cleared");
    opened.history("history.undo");
    assert_eq!(held(&opened), image, "undo brings the image back");
}

/// The document names what the entities hold from the first frame on, without
/// waiting for a save to work it out.
#[test]
fn the_document_names_every_material_from_the_first_frame() {
    let opened = open_scene();
    for (face, expected) in SAVED.iter().enumerate() {
        assert_eq!(
            opened.document_material(face),
            Some(BsnValue::String((*expected).to_string())),
            "face {face}"
        );
    }
}

#[test]
fn a_duplicate_holds_every_material_in_the_document_and_the_save() {
    let mut opened = open_scene();
    let handles = opened.handles();
    opened.select_brush();
    opened.invoke("entity.duplicate");
    opened.app.update();

    let copy = opened
        .brushes()
        .into_iter()
        .find(|entity| *entity != opened.brush)
        .expect("the duplicate is a brush");
    let held: Vec<_> = opened
        .app
        .world()
        .get::<Brush>(copy)
        .expect("the duplicate's brush")
        .faces
        .iter()
        .map(|face| face.material.clone())
        .collect();
    assert_eq!(held, handles, "the duplicate's live handles");
    assert_eq!(opened.handles(), handles, "the original's live handles");

    let saved = opened.saved_materials();
    assert_eq!(saved.len(), SAVED.len() * 2, "faces saved: {saved:?}");
    assert!(
        saved.iter().all(|name| !name.is_empty()),
        "no face lost its material: {saved:?}"
    );

    opened.history("history.undo");
    opened.refind_brush();
    assert_kept(&mut opened, &handles, "after undoing the duplicate");
}

#[test]
fn undoing_a_delete_brings_every_material_back() {
    let mut opened = open_scene();
    let handles = opened.handles();
    opened.select_brush();
    opened.invoke("entity.delete");
    opened.app.update();
    assert!(opened.brushes().is_empty(), "the brush is gone");

    opened.history("history.undo");
    opened.app.update();
    opened.refind_brush();
    assert_kept(&mut opened, &handles, "after undoing the delete");
}

#[test]
fn adding_a_component_and_undoing_it_keeps_every_material() {
    let mut opened = open_scene();
    opened.app.register_type::<Marked>();
    let handles = opened.handles();
    let marked = <Marked as TypePath>::type_path();

    opened.component_op("component.add", marked);
    opened.app.update();
    opened.refind_brush();
    assert!(
        opened.app.world().get::<Marked>(opened.brush).is_some(),
        "the component landed"
    );
    assert_kept(&mut opened, &handles, "after the add");

    opened.history("history.undo");
    opened.app.update();
    opened.refind_brush();
    assert!(
        opened.app.world().get::<Marked>(opened.brush).is_none(),
        "undo took it off"
    );
    assert_kept(&mut opened, &handles, "after undoing the add");
}

#[test]
fn removing_another_component_and_undoing_it_keeps_every_material() {
    let mut opened = open_scene();
    opened.app.register_type::<Marked>();
    let handles = opened.handles();
    let marked = <Marked as TypePath>::type_path();
    opened.component_op("component.add", marked);
    opened.app.update();
    opened.refind_brush();

    opened.component_op("component.remove", marked);
    opened.app.update();
    opened.refind_brush();
    assert_kept(&mut opened, &handles, "after the remove");

    opened.history("history.undo");
    opened.app.update();
    opened.refind_brush();
    assert!(
        opened.app.world().get::<Marked>(opened.brush).is_some(),
        "undo put it back"
    );
    assert_kept(&mut opened, &handles, "after undoing the remove");
}

#[test]
fn removing_the_brush_and_undoing_it_brings_every_material_back() {
    let mut opened = open_scene();
    let handles = opened.handles();
    opened.component_op("component.remove", BRUSH);
    opened.app.update();
    assert!(
        opened.app.world().get::<Brush>(opened.brush).is_none(),
        "the brush came off"
    );

    opened.history("history.undo");
    opened.app.update();
    opened.refind_brush();
    assert_kept(&mut opened, &handles, "after undoing the remove");
}

const NEVER: [&str; 6] = ["#Never", "@Never", "materials/gone.bsn", SOIL, MOSS, SOIL];

fn assert_never_kept(saved: &str, moment: &str) {
    for reference in ["#Never", "@Never", "materials/gone.bsn"] {
        assert!(
            saved.contains(&format!("material: \"{reference}\"")),
            "{moment}: '{reference}' is kept:\n{saved}"
        );
    }
}

/// A reference that never resolved holds a placeholder or a load that cannot
/// finish, and neither is the default handle that says it was cleared.
#[test]
fn a_reference_that_never_resolved_survives_a_brush_change_and_a_save() {
    let mut opened = open_scene_naming(NEVER);
    opened.mutate_brush(|brush| brush.faces[3].uv_rotation = 0.5);

    for (face, expected) in NEVER.iter().enumerate() {
        assert_eq!(
            opened.document_material(face),
            Some(BsnValue::String((*expected).to_string())),
            "the document after a brush change, face {face}"
        );
    }
    assert_never_kept(&opened.save(), "after a brush change");

    let Opened { app, dir, .. } = opened;
    drop(app);
    let mut reopened = open_project(dir);
    assert_never_kept(&reopened.save(), "after a reload");
}

/// A cleared material counted by what the save still names: soil on faces 0,
/// 3, 4, moss on faces 1, 2, 5.
fn assert_first_face_cleared(opened: &mut Opened, moment: &str) {
    let held = opened.handles();
    assert_eq!(
        held[0],
        Handle::default(),
        "{moment}: the face stays cleared"
    );
    assert!(
        held[1..].iter().all(|handle| *handle != Handle::default()),
        "{moment}: the other faces keep theirs"
    );
    let saved = opened.saved_materials();
    let named = |wanted: &str| saved.iter().filter(|name| *name == wanted).count();
    assert_eq!(
        (named(SOIL), named(MOSS)),
        (2, 3),
        "{moment}: saved materials {saved:?}"
    );
}

#[test]
fn a_material_cleared_by_a_field_edit_stays_cleared_through_a_save_and_a_reload() {
    let mut opened = open_scene();
    let handles = opened.handles();
    opened.set_face("faces[0].material", "\"\"");
    assert_eq!(
        opened.document_material(0),
        None,
        "a list element leaves out a material at its default"
    );
    assert_first_face_cleared(&mut opened, "after the edit");

    opened.mutate_brush(|brush| brush.faces[3].uv_rotation = 0.5);
    assert_first_face_cleared(&mut opened, "after a brush change");

    opened.history("history.undo");
    assert_eq!(opened.handles(), handles, "undo brings the material back");
    opened.history("history.redo");

    let Opened { app, dir, .. } = opened;
    drop(app);
    let mut reopened = open_project(dir);
    assert_first_face_cleared(&mut reopened, "after a reload");
}

/// The live brush says a material it could resolve is gone, so the document
/// takes that as the user's word and does not bring the old name back.
#[test]
fn a_material_cleared_on_the_live_brush_stays_cleared_through_a_save_and_a_reload() {
    let mut opened = open_scene();
    opened.mutate_brush(|brush| brush.faces[0].material = Handle::default());
    assert_ne!(
        opened.document_material(0),
        Some(BsnValue::String(SOIL.to_string())),
        "the document no longer names it"
    );
    assert_first_face_cleared(&mut opened, "after the change");

    let Opened { app, dir, .. } = opened;
    drop(app);
    let mut reopened = open_project(dir);
    assert_first_face_cleared(&mut reopened, "after a reload");
}

/// An element written whole leaves out what it holds at the default, and those
/// fields of the live face take the default too.
#[test]
fn a_face_written_whole_clears_what_it_leaves_out() {
    let mut opened = open_scene();
    let brush = opened.brush;
    let world = opened.app.world_mut();
    let registry = world.resource::<AppTypeRegistry>().clone();
    let element = BsnValue::Struct(jackdaw_bsn::BsnStructData {
        type_path: "jackdaw_geometry::BrushFaceData".to_string(),
        fields: jackdaw_bsn::BsnStructFields(vec![jackdaw_bsn::BsnField {
            name: "uv_rotation".to_string(),
            value: BsnValue::Float(0.5),
        }]),
    });
    {
        let mut ast = world.resource_mut::<SceneBsnAst>();
        let node = ast.ast_for(brush).expect("the brush is in the document");
        jackdaw_bsn::set_bsn_field(&mut ast, node, BRUSH, "faces[0]", element, &registry.read());
    }
    jackdaw_bsn::apply_component_patch_field(world, brush, BRUSH, "faces[0]");

    let face = opened.live().faces[0].clone();
    assert_eq!(face.uv_rotation, 0.5, "the written field lands");
    assert_eq!(
        face.material,
        Handle::default(),
        "the left-out material reads as the default"
    );
    assert_eq!(face.plane.distance, 0.0, "the left-out plane does too");
}

/// A sync that names materials on list elements writes them as a load would
/// after compacting: nothing about the text changes when one is run over it.
#[test]
fn a_sync_writes_named_materials_in_the_form_a_load_compacts_to() {
    let mut opened = open_scene();
    opened.mutate_brush(|brush| brush.faces[3].uv_rotation = 0.5);

    let world = opened.app.world();
    let synced = jackdaw_bsn::emit_scene(world.resource::<SceneBsnAst>());
    assert!(
        synced.contains(SOIL),
        "the sync names the material: {synced}"
    );
    assert!(!synced.contains("uv_offset"), "defaults stay out: {synced}");
    assert!(!synced.contains("material: \"\""), "{synced}");

    let mut loaded = jackdaw_bsn::parse_bsn_text(&synced).expect("the text parses");
    jackdaw_bsn::compact_document(&mut loaded, &world.resource::<AppTypeRegistry>().read());
    assert_eq!(jackdaw_bsn::emit_scene(&loaded), synced);
}

/// Scene names no file or entry answers for yet, face by face.
const LATE: [&str; 6] = ["#Late", "@late", LATE_FILE, "#Still", MOSS, SOIL];
const LATE_FILE: &str = "materials/late.bsn";

/// `#Late` joins the scene as pasted content brings it.
fn adopt_late_entry(app: &mut App) {
    let source = jackdaw_bsn::parse_bsn_text(
        "#Late\nbevy_pbr::pbr_material::StandardMaterial {\n    perceptual_roughness: 0.5,\n}\n",
    )
    .expect("the entry parses");
    assert!(jackdaw_bsn::adopt_asset_roots(app.world_mut(), &source).is_empty());
}

/// `@late` and its path resolve as the asset index finds the file.
fn file_appears(opened: &mut Opened) {
    std::fs::write(
        opened.dir.path().join("assets").join(LATE_FILE),
        material_file(0.5),
    )
    .expect("the file is written");
    jackdaw::asset_index::rescan_asset_index(opened.app.world_mut());
}

/// A name that starts resolving swaps the placeholder for the asset. A later sync
/// and a save keep the name.
#[test]
fn a_name_that_resolves_after_spawn_links_up_and_survives_syncs_and_a_save() {
    let mut opened = open_scene_naming(LATE);
    let unlinked = opened.handles();
    assert!(is_placeholder(&unlinked[0]), "unresolved at spawn");
    assert!(is_placeholder(&unlinked[1]), "unresolved at spawn");

    adopt_late_entry(&mut opened.app);
    file_appears(&mut opened);
    opened.app.update();

    let linked = opened.handles();
    assert!(!holds_no_asset(&linked[0]), "#Late links up");
    assert!(!holds_no_asset(&linked[1]), "@late links up");
    assert_ne!(
        linked[2], unlinked[2],
        "the failed load gives way to the file"
    );
    assert_eq!(linked[1], linked[2], "the name and the path are one file");
    assert!(is_placeholder(&linked[3]), "#Still has not arrived");

    opened.mutate_brush(|brush| brush.faces[4].uv_rotation = 0.5);
    assert_eq!(
        opened.document_material(0),
        Some(BsnValue::String("#Late".to_string())),
        "the sync keeps #Late"
    );
    assert_eq!(
        opened.document_material(1),
        Some(BsnValue::String(LATE_FILE.to_string())),
        "the sync names the file as it names every file"
    );
    assert_eq!(
        opened.document_material(3),
        Some(BsnValue::String("#Still".to_string())),
        "a name still unresolved stays"
    );

    let saved = opened.saved_materials();
    assert_eq!(saved.len(), LATE.len(), "every face is named: {saved:?}");
    assert_eq!(saved[0], "#Late", "the save writes it: {saved:?}");
    assert_eq!(saved[1], LATE_FILE, "the save writes it: {saved:?}");
}

/// A sync in the same frame as the arrival, either side of the link.
#[test]
fn a_sync_in_the_frame_a_name_arrives_keeps_it() {
    let mut opened = open_scene_naming(LATE);
    adopt_late_entry(&mut opened.app);
    file_appears(&mut opened);

    opened.mutate_brush(|brush| brush.faces[4].uv_rotation = 0.5);

    assert_eq!(
        opened.document_material(0),
        Some(BsnValue::String("#Late".to_string())),
    );
    assert!(!holds_no_asset(&opened.handles()[0]));
    assert!(!holds_no_asset(&opened.handles()[1]));
}

/// A material cleared on purpose is not a name waiting to arrive: the document
/// no longer holds it, so a file taking that name later changes nothing, and
/// undo is what brings it back.
#[test]
fn a_material_cleared_on_purpose_stays_cleared_when_its_file_appears() {
    let mut opened = open_scene_naming(LATE);
    opened.set_face("faces[1].material", "\"\"");
    opened.set_face("faces[2].material", "\"\"");
    assert_eq!(opened.document_material(1), None);
    assert_eq!(opened.document_material(2), None);

    file_appears(&mut opened);
    opened.app.update();
    opened.mutate_brush(|brush| brush.faces[4].uv_rotation = 0.5);

    assert_eq!(opened.handles()[1], Handle::default(), "stays cleared");
    assert_eq!(opened.handles()[2], Handle::default(), "stays cleared");
    assert_eq!(opened.document_material(1), None);
    assert_eq!(opened.document_material(2), None);
    let saved = opened.saved_materials();
    assert!(
        !saved.iter().any(|name| name.contains("late")),
        "the save names neither: {saved:?}"
    );

    opened.history("history.undo");
    opened.history("history.undo");
    opened.history("history.undo");
    let restored = opened.handles();
    assert_eq!(
        restored[1], restored[2],
        "undo restores both, now resolving"
    );
    assert_ne!(restored[1], Handle::default());
}

/// A file no document names is a lookup: nothing live is written.
#[test]
fn a_file_nothing_names_leaves_the_scene_alone() {
    let mut opened = open_scene_naming(LATE);
    let before = opened
        .app
        .world()
        .entity(opened.brush)
        .get_change_ticks::<Brush>()
        .expect("the brush is live")
        .changed;
    std::fs::write(
        opened.dir.path().join("assets/materials/unrelated.bsn"),
        material_file(0.5),
    )
    .expect("the file is written");
    jackdaw::asset_index::rescan_asset_index(opened.app.world_mut());
    opened.app.update();

    let after = opened
        .app
        .world()
        .entity(opened.brush)
        .get_change_ticks::<Brush>()
        .expect("the brush is live")
        .changed;
    assert_eq!(before, after, "the brush was not written");
}

/// A prefab's inherited face waits on its name the same way.
#[test]
fn an_inherited_face_links_up_when_its_name_arrives() {
    let mut opened = open_scene();
    let body = spawn_late_prefab_instance(&mut opened);
    let face = |app: &App| {
        app.world().get::<Brush>(body).expect("live").faces[0]
            .material
            .clone()
    };
    assert!(is_placeholder(&face(&opened.app)), "unresolved at spawn");

    file_appears(&mut opened);
    opened.app.update();

    assert!(!holds_no_asset(&face(&opened.app)), "the instance links up");
}

/// An entity holding the game's streamed values is not the editor's to mirror:
/// what the stream wrote stays out of the document, so out of a save.
#[test]
fn a_streamed_brush_value_stays_out_of_the_document() {
    let mut opened = open_scene();
    let brush = opened.brush;
    let authored = opened.document_material(0);
    assert_eq!(authored, Some(BsnValue::String(SOIL.to_string())));

    let world = opened.app.world_mut();
    let mut written = world.get_mut::<Brush>(brush).expect("the brush is live");
    written.faces[1].uv_rotation = 0.5;
    written.faces[0].material = Handle::default();
    jackdaw::pie_projection::mark_streamed(world, brush, BRUSH);
    opened.settle();

    let ast = opened.app.world().resource::<SceneBsnAst>();
    let node = ast.ast_for(brush).expect("the brush is in the document");
    assert_ne!(
        get_bsn_field(ast, node, BRUSH, "faces[1].uv_rotation"),
        Some(BsnValue::Float(0.5)),
        "the streamed rotation stays out of the document"
    );
    assert_eq!(
        opened.document_material(0),
        authored,
        "the streamed handle stays out of the document"
    );
}

/// An asset held only in memory has no name yet, so the document cannot bring
/// it back; a field edit leaves it alone and a save still embeds it.
#[test]
fn a_material_held_only_in_memory_survives_a_field_edit_and_saves_embedded() {
    let mut opened = open_scene();
    let handmade = opened
        .app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    let held = handmade.clone();
    opened.mutate_brush(move |brush| brush.faces[0].material = held);

    opened.set_face("faces[1].uv_rotation", "0.5");
    assert_eq!(opened.handles()[0], handmade, "after the set");
    opened.history("history.undo");
    assert_eq!(opened.handles()[0], handmade, "after undo");

    let saved = opened.saved_materials();
    assert!(
        saved[0].starts_with('#'),
        "the save embeds it under a name: {saved:?}"
    );
}

/// What the document holds for the live face whose plane faces `normal`.
fn material_of_face_facing(opened: &Opened, normal: Vec3) -> Option<BsnValue> {
    let face = opened
        .live()
        .faces
        .iter()
        .position(|face| face.plane.normal.distance(normal) < 1e-3)
        .expect("a face faces that way");
    opened.document_material(face)
}

fn named(reference: &str) -> Option<BsnValue> {
    Some(BsnValue::String(reference.to_string()))
}

impl Opened {
    /// Enter face mode with `faces` picked, as the card's operators expect.
    fn pick_faces(&mut self, faces: Vec<usize>) {
        self.select_brush();
        let brush = self.brush;
        let world = self.app.world_mut();
        *world.resource_mut::<jackdaw::brush::EditMode>() =
            jackdaw::brush::EditMode::BrushEdit(jackdaw::brush::BrushEditMode::Face);
        let topology = world.get::<Brush>(brush).expect("live").topology.clone();
        world
            .entity_mut(brush)
            .insert(jackdaw::brush::BrushHalfedge::from_topology(&topology));
        let mut selection = world.resource_mut::<jackdaw::brush::BrushSelection>();
        selection.active_brush = Some(brush);
        selection.sub_mut(brush).faces = faces;
        self.app.update();
    }

    fn settle(&mut self) {
        self.app.update();
        self.app.update();
    }

    fn entity_named(&mut self, name: &str) -> Entity {
        self.app
            .world_mut()
            .query::<(Entity, &Name)>()
            .iter(self.app.world())
            .find(|(_, held)| held.as_str() == name)
            .map(|(entity, _)| entity)
            .expect("the entity spawned")
    }
}

/// A dissolve shortens the list: a kept name stays with its face, and a cleared
/// face does not take the name of the face before it.
#[test]
fn a_dissolve_leaves_kept_names_on_their_own_faces() {
    let mut opened = open_scene_naming(["#A", "", "#B", "", "", ""]);
    opened.pick_faces(vec![0]);
    opened.invoke("brush.mesh.dissolve_faces");
    opened.settle();

    assert_eq!(opened.live().faces.len(), 5, "the face is gone");
    assert_eq!(
        material_of_face_facing(&opened, Vec3::NEG_X),
        None,
        "cleared stays cleared"
    );
    assert_eq!(material_of_face_facing(&opened, Vec3::Y), named("#B"));
    assert!(
        !opened.save().contains("#A"),
        "the dissolved face's name goes with it"
    );
}

#[test]
fn a_dissolve_beside_a_resolved_material_keeps_the_unresolved_name() {
    let mut opened = open_scene_naming([SOIL, "#Gone", "", "", "", ""]);
    opened.pick_faces(vec![0]);
    opened.invoke("brush.mesh.dissolve_faces");
    opened.settle();

    assert_eq!(opened.live().faces.len(), 5);
    assert_eq!(
        material_of_face_facing(&opened, Vec3::NEG_X),
        named("#Gone")
    );
}

#[test]
fn kept_names_follow_their_faces_through_a_reorder() {
    let mut opened = open_scene_naming(["#A", "", "#B", "", "", ""]);
    opened.mutate_brush(|brush| brush.faces.swap(0, 2));

    assert_eq!(material_of_face_facing(&opened, Vec3::X), named("#A"));
    assert_eq!(material_of_face_facing(&opened, Vec3::Y), named("#B"));
    assert_eq!(material_of_face_facing(&opened, Vec3::NEG_X), None);
}

/// A face edited in place has nothing else to match, and its list did not move.
#[test]
fn a_face_moved_in_place_keeps_its_unresolved_name() {
    let mut opened = open_scene_naming(["#A", "", "#B", "", "", ""]);
    opened.mutate_brush(|brush| brush.faces[0].plane.distance = 3.0);

    assert_eq!(opened.document_material(0), named("#A"));
    assert_eq!(opened.document_material(1), None);
    assert_eq!(opened.document_material(2), named("#B"));
}

/// Two faces alike in everything but the name, one removed: each handle says
/// which went, so the survivor keeps its own.
#[test]
fn identical_faces_with_one_removed_keep_the_survivors_name() {
    let plane = |x: f32| {
        format!(
            "        jackdaw_geometry::BrushFaceData {{\n            \
             plane: jackdaw_geometry::BrushPlane {{\n                \
             normal: glam::Vec3 {{ x: {x:?}, y: 0.0, z: 0.0 }},\n                \
             distance: 1.0,\n            }},\n"
        )
    };
    let face = |x: f32, material: &str| {
        format!(
            "{}            material: \"{material}\",\n        }},\n",
            plane(x)
        )
    };
    let scene = format!(
        "#Block\n\
         bevy_transform::components::transform::Transform\n\
         jackdaw_scene_types::types::Brush {{\n    faces: [\n{}{}{}    ],\n}}\n",
        face(1.0, "#A"),
        face(1.0, "#B"),
        face(-1.0, "#C"),
    );
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(&dir, &scene);
    let mut opened = open_project(dir);

    opened.mutate_brush(|brush| {
        brush.faces.remove(0);
    });

    assert_eq!(opened.live().faces.len(), 2);
    assert_eq!(opened.document_material(0), named("#B"), "the survivor's");
    assert_eq!(opened.document_material(1), named("#C"), "the rest stay");
}

/// The card's clear is the user's word even when the name never resolved.
#[test]
fn clearing_an_unresolved_face_from_the_card_clears_and_undoes() {
    let mut opened = open_scene_naming(NEVER);
    opened.pick_faces(vec![0]);
    opened.invoke("brush.face.clear_material");
    opened.settle();

    assert_eq!(opened.document_material(0), None, "the document clears");
    assert_eq!(
        opened.document_material(1),
        named("@Never"),
        "only that face"
    );
    assert!(
        !opened.save().contains("#Never"),
        "the save names it no more"
    );

    opened.history("history.undo");
    opened.settle();
    assert_eq!(
        opened.document_material(0),
        named("#Never"),
        "undo restores"
    );

    opened.history("history.redo");
    opened.settle();
    assert_eq!(opened.document_material(0), None, "redo clears again");
}

#[test]
fn clearing_every_material_includes_the_ones_that_never_resolved() {
    let mut opened = open_scene_naming(["#A", "@B", "#C", "#D", "#E", "#F"]);
    opened.select_brush();
    opened.invoke("brush.clear_all_materials");
    opened.settle();

    assert!(
        (0..6).all(|face| opened.document_material(face).is_none()),
        "every name goes"
    );
    opened.history("history.undo");
    opened.settle();
    assert_eq!(opened.document_material(0), named("#A"), "undo restores");
    assert_eq!(opened.document_material(1), named("@B"), "undo restores");
}

/// Undo restores what the document named, which the old handles cannot say.
#[test]
fn undoing_a_material_applied_over_an_unresolved_name_brings_the_name_back() {
    let mut opened = open_scene_naming(NEVER);
    opened.pick_faces(vec![0]);
    opened
        .app
        .world_mut()
        .operator("material.apply".to_string())
        .param("material", SOIL.to_string())
        .call()
        .expect("material.apply dispatches")
        .assert_finished();
    opened.settle();
    assert_eq!(opened.document_material(0), named(SOIL), "applied");

    opened.history("history.undo");
    opened.settle();
    assert_eq!(
        opened.document_material(0),
        named("#Never"),
        "undo restores"
    );

    opened.history("history.redo");
    opened.settle();
    assert_eq!(opened.document_material(0), named(SOIL), "redo applies");
}

/// A brush command recorded before a name linked holds the placeholder as its old
/// value; undoing it gets the material that has since arrived.
#[test]
fn undoing_a_brush_command_recorded_before_a_name_linked_keeps_the_name() {
    let mut opened = open_scene_naming(["@late", MOSS, MOSS, SOIL, SOIL, MOSS]);
    assert!(is_placeholder(&opened.handles()[0]), "unresolved at spawn");
    opened.pick_faces(vec![1]);
    opened
        .app
        .world_mut()
        .operator("brush.face.set_uv_scale_preset".to_string())
        .param("scale", 2.0_f64)
        .call()
        .expect("the preset dispatches")
        .assert_finished();
    opened.settle();

    file_appears(&mut opened);
    opened.app.update();
    assert!(!holds_no_asset(&opened.handles()[0]), "linked");

    opened.history("history.undo");
    opened.settle();
    assert_eq!(
        opened.document_material(0),
        named(LATE_FILE),
        "the document still names it"
    );
    assert!(!holds_no_asset(&opened.handles()[0]), "still linked");
}

/// A stream writing one component says nothing about the entity's others.
#[test]
fn an_edit_on_an_entity_streaming_another_component_reaches_the_document() {
    use jackdaw_geometry::{MeshMirror, Modifier, ModifierEntry, ModifierStack};

    let mut opened = open_scene();
    let brush = opened.brush;
    let world = opened.app.world_mut();
    world
        .resource_mut::<PieProjection>()
        .by_bits
        .insert(77, brush);
    jackdaw::pie_projection::project_event(
        world,
        jackdaw_pie_protocol::StateEvent::ComponentChanged {
            entity: 77,
            type_path: "bevy_transform::components::transform::Transform".to_string(),
            value: serde_json::json!({
                "translation": [4.0, 0.0, 0.0],
                "rotation": [0.0, 0.0, 0.0, 1.0],
                "scale": [1.0, 1.0, 1.0],
            }),
        },
    );
    assert!(
        world.resource::<PieProjection>().overlaid.contains(&brush),
        "the stream wrote to the entity"
    );

    world.entity_mut(brush).insert(ModifierStack {
        modifiers: vec![ModifierEntry::new(Modifier::Mirror(MeshMirror::default()))],
    });
    opened.settle();

    let ast = opened.app.world().resource::<SceneBsnAst>();
    let node = ast.ast_for(brush).expect("the brush is in the document");
    assert!(
        ast.find_patch_by_type_path(
            node,
            <ModifierStack as bevy::reflect::TypePath>::type_path()
        )
        .is_some(),
        "the stack is in the document"
    );
}

const SMALL_PNG: &[u8] = &[
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4,
    0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9c, 0x63, 0x60, 0x60, 0x60, 0x60,
    0x00, 0x00, 0x00, 0x05, 0x00, 0x01, 0xa5, 0xf6, 0x45, 0x40, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45,
    0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
];

/// A scene holding an instance of a prefab whose sprite names an image.
fn open_sprite_instance() -> Opened {
    let mut opened = open_scene();
    let assets = opened.dir.path().join("assets");
    std::fs::write(assets.join("a.png"), SMALL_PNG).expect("the image is written");
    let prefab = assets.join("door.bsn");
    let prefab_id = |id: i128| {
        BsnPatch::TupleStruct(jackdaw_bsn::BsnTupleStructData {
            type_path: "jackdaw::prefab::components::PrefabEntityId".to_string(),
            values: vec![BsnValue::Int(id)],
        })
    };
    let sprite = BsnPatch::Struct(jackdaw_bsn::BsnStructData {
        type_path: SPRITE.to_string(),
        fields: jackdaw_bsn::BsnStructFields(vec![jackdaw_bsn::BsnField {
            name: "image".to_string(),
            value: BsnValue::String("a.png".to_string()),
        }]),
    });
    let mut doc = SceneBsnAst::default();
    let root = doc.create_entity_node(vec![
        BsnPatch::Type("jackdaw::prefab::components::Prefab".to_string()),
        prefab_id(0),
        BsnPatch::Name("Door".to_string()),
    ]);
    doc.add_to_roots(root);
    let child = doc.create_entity_node(vec![
        prefab_id(1),
        BsnPatch::Name("DoorSign".to_string()),
        sprite,
    ]);
    doc.add_child_to_ast(root, child);
    std::fs::write(&prefab, jackdaw_bsn::emit_scene(&doc)).expect("the prefab is written");
    jackdaw::prefab::operators::spawn_instance(opened.app.world_mut(), &prefab, Vec3::ZERO);
    opened.settle();
    opened
}

fn sprite_image(opened: &mut Opened) -> Handle<Image> {
    let sign = opened.entity_named("DoorSign");
    opened
        .app
        .world()
        .get::<Sprite>(sign)
        .expect("the sprite")
        .image
        .clone()
}

fn assert_sprite_stays_cleared(mut opened: Opened) {
    let text = opened.save();
    assert!(
        text.contains("image: \"\""),
        "the save writes the clear over the prefab's image:\n{text}"
    );
    let Opened { app, dir, .. } = opened;
    drop(app);
    let mut reopened = open_project(dir);
    assert_eq!(
        sprite_image(&mut reopened),
        Handle::default(),
        "the reload does not inherit the image"
    );
}

/// Written `""` overrides what the prefab names; an omitted field would inherit.
#[test]
fn a_field_cleared_on_a_prefab_instance_stays_cleared_through_a_reload() {
    let mut opened = open_sprite_instance();
    assert_ne!(sprite_image(&mut opened), Handle::default(), "inherited");
    let sign = opened.entity_named("DoorSign");

    opened.set(SPRITE, sign, "image", "\"\"");

    assert_sprite_stays_cleared(opened);
}

/// The same clear reaching the document through a sync of the live value.
#[test]
fn a_live_clear_on_a_prefab_instance_stays_cleared_through_a_reload() {
    let mut opened = open_sprite_instance();
    assert_ne!(sprite_image(&mut opened), Handle::default(), "inherited");
    let sign = opened.entity_named("DoorSign");

    let world = opened.app.world_mut();
    let mut sprite = world.get_mut::<Sprite>(sign).expect("the sprite");
    sprite.image = Handle::default();
    let cleared = sprite.clone();
    jackdaw::commands::sync_component_to_ast(world, sign, SPRITE, &cleared);
    opened.settle();

    assert_sprite_stays_cleared(opened);
}

/// A unit component comes off the live entity with its undo, as a valued one does.
#[test]
fn undoing_an_added_marker_takes_it_off_the_live_entity() {
    undo_of_added_marker(open_scene());
}

/// The same when the edit cannot be put back in place, as a name the scene does
/// not hold forces a respawn.
#[test]
fn undoing_an_added_marker_beside_an_unresolved_name_takes_it_off() {
    undo_of_added_marker(open_scene_naming(NEVER));
}

fn undo_of_added_marker(mut opened: Opened) {
    use bevy::light::NotShadowCaster;

    let marker = <NotShadowCaster as TypePath>::type_path();

    opened.select_brush();
    let brush = opened.brush;
    opened
        .app
        .world_mut()
        .operator("component.add".to_string())
        .param("entity", brush)
        .param("type_path", marker.to_string())
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: true,
        })
        .call()
        .expect("the component operator dispatches")
        .assert_finished();
    opened.settle();
    opened.refind_brush();
    assert!(
        opened
            .app
            .world()
            .get::<NotShadowCaster>(opened.brush)
            .is_some(),
        "the component landed"
    );

    opened.history("history.undo");
    opened.settle();
    opened.refind_brush();
    assert!(
        opened
            .app
            .world()
            .get::<NotShadowCaster>(opened.brush)
            .is_none(),
        "undo took it off the live entity"
    );
    let ast = opened.app.world().resource::<SceneBsnAst>();
    let node = ast
        .ast_for(opened.brush)
        .expect("the brush is in the document");
    assert!(
        ast.find_patch_by_type_path(node, marker).is_none(),
        "and out of the document"
    );
}

/// A material kept only in the project's catalog resolves like any other name.
#[test]
fn a_material_only_in_the_catalog_links_and_keeps_its_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(
        &dir,
        &scene_text_naming(["@Steel", MOSS, MOSS, SOIL, SOIL, MOSS]),
    );
    std::fs::write(
        dir.path().join("assets/catalog.bsn"),
        "#Steel\nbevy_pbr::pbr_material::StandardMaterial {\n    perceptual_roughness: 0.5,\n}\n",
    )
    .expect("the catalog is written");
    let mut opened = open_project(dir);
    opened.settle();

    assert_ne!(opened.handles()[0], Handle::default(), "the name links up");
    opened.mutate_brush(|brush| brush.faces[3].uv_rotation = 0.5);
    assert_eq!(opened.document_material(0), named("@Steel"));
}

/// A name held under another asset type never links, so it never reads as
/// cleared: a sync keeps it.
#[test]
fn a_name_held_under_another_asset_type_stays_in_the_document() {
    let mut opened = open_scene_naming(["#Picture", MOSS, MOSS, SOIL, SOIL, MOSS]);
    let image = opened
        .app
        .world_mut()
        .resource_mut::<Assets<Image>>()
        .add(Image::default());
    opened
        .app
        .world_mut()
        .resource_mut::<BsnSceneAssets>()
        .0
        .insert("#Picture".to_string(), image.untyped());
    opened.settle();
    assert!(is_placeholder(&opened.handles()[0]), "cannot link");

    opened.mutate_brush(|brush| brush.faces[3].uv_rotation = 0.5);
    assert_eq!(opened.document_material(0), named("#Picture"));
    opened.mutate_brush(|brush| brush.faces[3].uv_rotation = 0.75);
    assert_eq!(opened.document_material(0), named("#Picture"));
}

impl Opened {
    fn undo_entries(&self) -> usize {
        self.app
            .world()
            .resource::<jackdaw::commands::CommandHistory>()
            .undo_stack
            .len()
    }

    /// A count of the faces the save names `reference` on.
    fn saved_faces_naming(&mut self, reference: &str) -> usize {
        self.saved_materials()
            .iter()
            .filter(|name| *name == reference)
            .count()
    }

    /// Write `change` of the live brush to it as the game's stream does.
    fn stream_brush(&mut self, change: impl FnOnce(&mut Brush)) {
        let brush = self.brush;
        let mut streamed = self.live();
        change(&mut streamed);
        let world = self.app.world_mut();
        let probe = world.spawn(streamed).id();
        let registry = world.resource::<AppTypeRegistry>().clone();
        let snapshot = jackdaw_pie_protocol::build_snapshot(world, &registry.read(), &[probe]);
        world.entity_mut(probe).despawn();
        let value = snapshot[0].components[BRUSH].clone();
        world
            .resource_mut::<PieProjection>()
            .by_bits
            .insert(77, brush);
        jackdaw::pie_projection::project_event(
            world,
            jackdaw_pie_protocol::StateEvent::ComponentChanged {
                entity: 77,
                type_path: BRUSH.to_string(),
                value,
            },
        );
        self.settle();
    }

    /// Run an operator with its own history entry.
    fn call_op(&mut self, id: &str, params: &[(&'static str, f64)]) {
        let mut call = self.app.world_mut().operator(id.to_string());
        for (name, value) in params {
            call = call.param(*name, *value);
        }
        call.settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: true,
        })
        .call()
        .expect("the operator dispatches")
        .assert_finished();
        self.settle();
    }
}

/// Each operator pushes its one brush command and nothing else, so one undo
/// takes back the whole of it.
#[test]
fn each_brush_card_operator_undoes_exactly_once() {
    let operators: [(&str, &[(&str, f64)]); 4] = [
        ("brush.face.clear_material", &[]),
        ("brush.face.apply_texture_to_all", &[]),
        ("brush.face.set_uv_scale_preset", &[("scale", 2.0)]),
        ("brush.clear_all_materials", &[]),
    ];
    for (id, params) in operators {
        let mut opened = open_scene();
        opened.pick_faces(vec![0]);
        let before = opened.live();
        let entries = opened.undo_entries();

        opened.call_op(id, params);
        assert_eq!(opened.undo_entries(), entries + 1, "{id} pushes one entry");

        opened.history("history.undo");
        opened.settle();
        assert_eq!(opened.undo_entries(), entries, "{id} undoes in one");
        let after = opened.live();
        for (index, (was, now)) in before.faces.iter().zip(&after.faces).enumerate() {
            assert_eq!(was.material, now.material, "{id}: face {index} material");
            assert_eq!(was.uv_scale, now.uv_scale, "{id}: face {index} uv scale");
        }
    }
}

/// A save writes what the document holds: a second writes the same bytes, with
/// names that never resolved, and again after an edit and after a reload.
#[test]
fn a_second_save_writes_the_same_bytes() {
    let mut opened = open_scene_naming(NEVER);
    let first = opened.save();
    assert_eq!(opened.save(), first, "unedited");

    opened.mutate_brush(|brush| brush.faces[3].uv_rotation = 0.5);
    let edited = opened.save();
    assert_eq!(opened.save(), edited, "after an edit");

    let Opened { app, dir, .. } = opened;
    drop(app);
    let mut reopened = open_project(dir);
    let reloaded = reopened.save();
    assert_eq!(reloaded, edited, "a reload writes what it read");
    assert_eq!(reopened.save(), reloaded, "and again");
}

/// A field keeps its place in its element through a sync.
#[test]
fn a_named_material_keeps_its_place_in_its_element() {
    let mut opened = open_scene_naming(["#Gone", "", "", "", "", ""]);
    opened.mutate_brush(|brush| brush.faces[0].uv_rotation = 0.5);
    let text = opened.save();
    let material = text.find("material: \"#Gone\"").expect("still named");
    let rotation = text.find("uv_rotation").expect("the edit");
    assert!(material < rotation, "{text}");
}

/// A scene of two overlapping brushes, the first naming `first` face by face.
fn open_two_brushes(first: [&str; 6]) -> Opened {
    let block = scene_text_naming(first);
    let other = scene_text_naming([""; 6]).replace("#Block", "#Other").replace(
        "bevy_transform::components::transform::Transform\n",
        "bevy_transform::components::transform::Transform { translation: glam::Vec3 { x: 1.5, y: 0.0, z: 0.0 } }\n",
    );
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(
        &dir,
        &format!("bevy_ecs::hierarchy::Children [\n{block},\n{other}]\n"),
    );
    let mut opened = open_project(dir);
    opened.brush = opened.entity_named("Block");
    opened
}

const ONE_GONE: [&str; 6] = ["", "#Gone", "", "", "", ""];
const CAP_GONE: [&str; 6] = ["#Gone", "", "", "", "", ""];

#[test]
fn an_extrude_keeps_an_unresolved_name_on_the_faces_it_moves() {
    let mut opened = open_scene_naming(CAP_GONE);
    opened.pick_faces(vec![0]);
    opened.call_op("brush.mesh.extrude", &[("distance", 0.5)]);

    assert_eq!(material_of_face_facing(&opened, Vec3::X), named("#Gone"));
    assert!(opened.saved_faces_naming("#Gone") >= 1);
    opened.history("history.undo");
    opened.settle();
    opened.refind_brush();
    assert_eq!(opened.live().faces.len(), 6, "undone");
    assert_eq!(opened.document_material(0), named("#Gone"));
}

/// A join leaves the primary brush's faces that stay on the hull as they were.
#[test]
fn a_join_keeps_an_unresolved_name_on_the_faces_that_stay() {
    let mut opened = open_two_brushes(ONE_GONE);
    let (block, other) = (opened.brush, opened.entity_named("Other"));
    opened.app.world_mut().resource_mut::<Selection>().entities = vec![block, other];
    opened.invoke("brush.join");
    opened.settle();

    assert_eq!(
        material_of_face_facing(&opened, Vec3::NEG_X),
        named("#Gone"),
        "the far face is still on the hull"
    );
    assert_eq!(opened.saved_faces_naming("#Gone"), 1);
}

#[test]
fn a_csg_cut_keeps_an_unresolved_name_on_what_is_left() {
    let mut opened = open_two_brushes(ONE_GONE);
    let (block, other) = (opened.brush, opened.entity_named("Other"));
    opened
        .app
        .world_mut()
        .operator("brush.csg_subtract".to_string())
        .param("target", block)
        .param("cutters", other)
        .call()
        .expect("the cut dispatches")
        .assert_finished();
    opened.settle();
    assert_eq!(opened.saved_faces_naming("#Gone"), 1, "the far face");
}

/// What a mirror does with a name is what it does with a material that resolves,
/// the kept face and any copy of it alike.
#[test]
fn a_mirror_treats_an_unresolved_name_as_it_treats_a_resolved_one() {
    let mirrored = |material: &str| {
        let mut opened = open_scene_naming([material, "", "", "", "", ""]);
        opened.select_brush();
        opened.invoke("mesh.mirror.add");
        opened.settle();
        assert_eq!(opened.document_material(0), named(material), "added");
        opened.invoke("mesh.mirror.apply");
        opened.settle();
        let faces = opened.live().faces.len();
        (0..faces)
            .map(|face| opened.document_material(face).is_some())
            .collect::<Vec<bool>>()
    };
    let unresolved = mirrored("#Gone");
    assert!(unresolved.contains(&true), "the name stays");
    assert_eq!(unresolved, mirrored(SOIL));
}

#[test]
fn a_clip_keeps_an_unresolved_name_on_both_pieces() {
    let mut opened = open_scene_naming(["", "", "#Gone", "", "", ""]);
    let brush = opened.brush;
    opened
        .app
        .world_mut()
        .operator("brush.clip".to_string())
        .param("entity", brush)
        .param("normal", Vec3::X)
        .param("point", Vec3::ZERO)
        .param("keep", "both".to_string())
        .call()
        .expect("the clip dispatches")
        .assert_finished();
    opened.settle();
    assert_eq!(opened.brushes().len(), 2, "two pieces");
    assert_eq!(opened.saved_faces_naming("#Gone"), 2, "one face on each");
}

/// A tab switch stores the document and spawns it again.
#[test]
fn a_tab_switch_keeps_unresolved_names_and_clears() {
    let mut opened = open_scene_naming(NEVER);
    opened.set_face("faces[3].material", "\"\"");
    opened
        .app
        .world_mut()
        .resource_mut::<jackdaw::scenes::Scenes>()
        .tabs
        .push(jackdaw::scenes::SceneTab::new_untitled(2));
    assert!(jackdaw::scenes::swap::swap_active_tab(
        opened.app.world_mut(),
        1
    ));
    opened.settle();
    assert!(jackdaw::scenes::swap::swap_active_tab(
        opened.app.world_mut(),
        0
    ));
    opened.settle();
    opened.refind_brush();

    assert_eq!(opened.document_material(0), named("#Never"));
    assert_eq!(opened.document_material(1), named("@Never"));
    assert_eq!(opened.document_material(3), None, "cleared stays cleared");
    assert_eq!(opened.handles()[3], Handle::default());

    opened.mutate_brush(|brush| brush.faces[4].uv_rotation = 0.5);
    assert_eq!(opened.document_material(0), named("#Never"));
    assert_eq!(opened.document_material(3), None, "after a sync too");
}

/// Play then Stop: the game's value goes back, and the names with the live brush.
#[test]
fn play_then_stop_keeps_unresolved_names() {
    let mut opened = open_scene_naming(NEVER);
    opened.stream_brush(|brush| brush.faces[0].uv_rotation = 0.5);
    assert_eq!(opened.live().faces[0].uv_rotation, 0.5, "the stream landed");
    assert_ne!(
        opened.handles()[3],
        Handle::default(),
        "a stream writes no handle"
    );

    jackdaw::pie_projection::revert_preview(opened.app.world_mut());
    opened.settle();
    opened.refind_brush();

    assert_eq!(opened.live().faces[0].uv_rotation, 0.0, "the prior is back");
    assert_never_kept(&opened.save(), "after Stop");
}

/// A name cleared by hand is not waiting: its arrival changes nothing, and only
/// undo brings it back.
#[test]
fn a_cleared_unresolved_name_stays_cleared_when_it_arrives() {
    let mut opened = open_scene_naming(LATE);
    opened.pick_faces(vec![0]);
    opened.invoke("brush.face.clear_material");
    opened.settle();
    assert_eq!(
        opened.handles()[0],
        Handle::default(),
        "a default is cleared"
    );
    assert_eq!(opened.document_material(0), None);

    adopt_late_entry(&mut opened.app);
    opened.settle();
    assert_eq!(opened.handles()[0], Handle::default(), "stays cleared");
    assert_eq!(opened.document_material(0), None);
    assert!(!opened.save().contains("material: \"#Late\""));

    opened.history("history.undo");
    opened.settle();
    assert_eq!(opened.document_material(0), named("#Late"), "undo restores");
    let stored = opened.app.world().resource::<Assets<StandardMaterial>>();
    assert!(
        stored.contains(&opened.handles()[0]),
        "and the name has arrived, so it holds the asset"
    );
}

/// A face waiting on its name shows the palette default, as a cleared one does,
/// rather than a material no renderer can draw.
#[test]
fn a_face_waiting_on_its_name_wears_the_default_material() {
    let mut opened = open_scene_naming(["#Gone"; 6]);
    opened.settle();
    let brush = opened.brush;
    let world = opened.app.world_mut();
    let chunks: Vec<_> = world
        .query::<&jackdaw::brush::BrushMeshChunk>()
        .iter(world)
        .filter(|chunk| chunk.brush_entity == brush)
        .map(|chunk| (chunk.uses_default_material, chunk.material.clone()))
        .collect();
    assert!(!chunks.is_empty(), "the brush has chunks");
    let stored = world.resource::<Assets<StandardMaterial>>();
    for (uses_default, material) in chunks {
        assert!(uses_default, "drawn as the default");
        assert!(stored.contains(&material), "with a material that exists");
    }
}

/// An edit on a brush the game has streamed reaches the document, so Stop keeps
/// it and its undo entry stands for something. The value the stream wrote alone
/// stays out, as [`a_streamed_brush_value_stays_out_of_the_document`] holds.
#[test]
fn a_card_edit_on_a_streamed_brush_survives_stop() {
    let mut opened = open_scene();
    opened.stream_brush(|brush| brush.faces[0].uv_offset = Vec2::new(9.0, 9.0));
    assert_eq!(
        opened.live().faces[0].uv_offset,
        Vec2::new(9.0, 9.0),
        "the stream landed"
    );
    assert!(
        !opened
            .app
            .world()
            .resource::<SceneBsnAst>()
            .ast_for(opened.brush)
            .is_some_and(|node| get_bsn_field(
                opened.app.world().resource::<SceneBsnAst>(),
                node,
                BRUSH,
                "faces[0].uv_offset"
            )
            .is_some()),
        "the write alone stays out"
    );

    opened.pick_faces(vec![1]);
    let before = opened.live().faces[1].uv_scale;
    opened.call_op("brush.face.set_uv_scale_preset", &[("scale", 4.0)]);

    jackdaw::pie_projection::revert_preview(opened.app.world_mut());
    opened.settle();
    opened.refind_brush();
    assert_eq!(
        opened.live().faces[1].uv_scale,
        Vec2::splat(4.0),
        "the edit stays through Stop"
    );

    opened.history("history.undo");
    opened.settle();
    assert_eq!(
        opened.live().faces[1].uv_scale,
        before,
        "and its undo entry takes it back"
    );
}

/// The same for a live edit in the inspector, which the game has yet to echo.
#[test]
fn a_card_edit_after_a_live_inspector_edit_survives_stop() {
    let mut opened = open_scene();
    let brush = opened.brush;
    jackdaw::live_edits::record_live_edit(
        opened.app.world_mut(),
        brush,
        77,
        BRUSH,
        "faces[0].uv_rotation",
        serde_json::json!(0.25),
    );
    opened.pick_faces(vec![1]);
    opened.call_op("brush.face.set_uv_scale_preset", &[("scale", 4.0)]);
    assert_eq!(
        get_bsn_field(
            opened.app.world().resource::<SceneBsnAst>(),
            opened
                .app
                .world()
                .resource::<SceneBsnAst>()
                .ast_for(brush)
                .expect("the brush is in the document"),
            BRUSH,
            "faces[1].uv_scale.x"
        ),
        Some(BsnValue::Float(4.0)),
        "the edit reaches the document"
    );
}

/// Other asset types and shapes of field hold a placeholder as a face does, and
/// an entity holding one saves and reloads as it was.
#[test]
fn other_asset_fields_hold_an_unresolved_name() {
    let scene = "#Holder\n\
         bevy_sprite::sprite::Sprite { image: \"#GoneImage\" }\n\
         bevy_mesh::components::Mesh3d(\"@GoneMesh\")\n";
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(&dir, scene);
    let assets = dir.path().join("assets");
    let mut app = util::editor_test_app_reading(&assets);
    app.insert_resource(ProjectRoot::new(
        dir.path().to_path_buf(),
        ProjectConfig::default(),
    ));
    request_open_with_conversion(app.world_mut(), &assets.join("scene.bsn"));
    util::settle_scene_load(&mut app);
    for _ in 0..3 {
        app.update();
    }
    let holder = app
        .world_mut()
        .query_filtered::<Entity, With<Sprite>>()
        .iter(app.world())
        .next()
        .expect("the sprite spawned");
    let world = app.world();
    let image = &world.get::<Sprite>(holder).expect("a sprite").image;
    assert!(is_placeholder(image), "the sprite's image waits");
    assert!(!world.resource::<Assets<Image>>().contains(image));
    let mesh = &world.get::<Mesh3d>(holder).expect("a mesh").0;
    assert!(is_placeholder(mesh), "the mesh waits");

    let text = jackdaw::scene_io::emit_bsn_scene_for_file(app.world_mut(), &assets)
        .expect("the scene emits");
    assert!(text.contains("image: \"#GoneImage\""), "{text}");
    assert!(text.contains("\"@GoneMesh\""), "{text}");
}

/// Typing a name no asset answers holds it, as loading one does.
#[test]
fn setting_a_field_to_an_unresolved_name_holds_it() {
    let mut opened = open_scene();
    opened.set_face("faces[0].material", "\"#Gone\"");
    assert_eq!(opened.document_material(0), named("#Gone"));
    assert!(is_placeholder(&opened.handles()[0]));

    opened.mutate_brush(|brush| brush.faces[3].uv_rotation = 0.5);
    assert_eq!(opened.document_material(0), named("#Gone"), "after a sync");

    opened.set_face("faces[0].material", "\"\"");
    assert_eq!(opened.document_material(0), None);
    assert_eq!(opened.handles()[0], Handle::default(), "a clear is a clear");
}

/// Copying a face's material copies its name, when it waits like any other.
#[test]
fn applying_an_unresolved_material_to_all_faces_names_them_all() {
    let mut opened = open_scene_naming(NEVER);
    opened.pick_faces(vec![0]);
    opened.call_op("brush.face.apply_texture_to_all", &[]);
    for face in 0..6 {
        assert_eq!(
            opened.document_material(face),
            named("#Never"),
            "face {face}"
        );
    }
    opened.history("history.undo");
    opened.settle();
    assert_eq!(
        opened.document_material(1),
        named("@Never"),
        "undo restores"
    );
    assert_eq!(opened.document_material(3), named(SOIL), "undo restores");
}

/// Faces naming files, and names nothing answers for.
const NAMED_AND_WAITING: [&str; 6] = [SOIL, "#Still", "@still", MOSS, SOIL, MOSS];

/// Saving a live entity to the scene writes the game's values and leaves the
/// references alone: a stream never carries a handle.
#[test]
fn saving_a_live_brush_to_the_scene_keeps_named_and_waiting_materials() {
    let mut opened = open_scene_naming(NAMED_AND_WAITING);
    let brush = opened.brush;
    let handles = opened.handles();
    let world = opened.app.world_mut();
    *world.resource_mut::<jackdaw::pie_mirror::PieViewMode>() =
        jackdaw::pie_mirror::PieViewMode::Live;
    world
        .resource_mut::<PieProjection>()
        .by_bits
        .insert(77, brush);
    world.resource_mut::<Selection>().entities = vec![brush];
    world.get_mut::<Brush>(brush).expect("live").faces[1].uv_rotation = 0.5;
    jackdaw::pie_projection::mark_streamed(world, brush, BRUSH);
    opened.settle();

    jackdaw::pie::save_live_entity_to_scene(opened.app.world_mut());
    opened.settle();

    for (face, name) in NAMED_AND_WAITING.iter().enumerate() {
        assert_eq!(opened.document_material(face), named(name), "face {face}");
    }
    assert_eq!(opened.handles(), handles, "the live handles stay");
    let ast = opened.app.world().resource::<SceneBsnAst>();
    let node = ast.ast_for(brush).expect("the brush is in the document");
    assert_eq!(
        get_bsn_field(ast, node, BRUSH, "faces[1].uv_rotation"),
        Some(BsnValue::Float(0.5)),
        "the game's value is what was asked for"
    );
    assert_eq!(opened.saved_materials(), NAMED_AND_WAITING);
}

/// A material file that goes away while the scene is open.
fn file_goes(opened: &mut Opened, file: &str) {
    std::fs::remove_file(opened.dir.path().join("assets").join(file)).expect("the file goes");
    jackdaw::asset_index::rescan_asset_index(opened.app.world_mut());
    opened.settle();
}

/// A file leaving does not rewrite what names it: the name stays through a sync
/// and a save, and the faces keep drawing what they hold.
#[test]
fn a_file_deleted_while_open_keeps_its_name_through_a_sync_and_a_save() {
    let mut opened = open_scene();
    let handles = opened.handles();

    file_goes(&mut opened, SOIL);
    opened.mutate_brush(|brush| brush.faces[1].uv_rotation = 0.5);

    assert_eq!(opened.document_material(0), named(SOIL), "after a sync");
    assert_eq!(opened.handles(), handles, "the view stays");
    let saved = opened.save();
    assert!(!saved.contains("#StandardMaterial"), "not inlined: {saved}");
    assert_eq!(materials_in(&saved), SAVED);
}

/// What the file's name reads as after a reload is what any reference that cannot
/// be found reads as, and the next save still names it.
#[test]
fn a_file_deleted_while_open_stays_named_after_a_reload() {
    let mut opened = open_scene();
    file_goes(&mut opened, SOIL);
    opened.save();

    let mut reopened = open_project(opened.dir);
    assert_eq!(reopened.saved_materials(), SAVED);
}

/// A rebuild reads the document, which still names the file.
#[test]
fn a_rebuild_after_a_file_is_deleted_keeps_its_name() {
    let mut opened = open_scene();
    let handles = opened.handles();
    file_goes(&mut opened, SOIL);
    opened.mutate_brush(|brush| brush.faces[1].uv_rotation = 0.5);

    opened.select_brush();
    opened.invoke("entity.delete");
    opened.history("history.undo");
    opened.settle();
    opened.refind_brush();

    assert_eq!(opened.document_material(0), named(SOIL));
    assert_eq!(opened.handles(), handles, "the view stays");
    assert_eq!(opened.saved_materials(), SAVED);
}

/// The file coming back reads into the handle the faces hold, which has a file
/// again.
#[test]
fn a_deleted_file_that_returns_goes_back_into_the_handle() {
    let mut opened = open_scene();
    let before = opened.handles();
    file_goes(&mut opened, SOIL);
    opened.mutate_brush(|brush| brush.faces[1].uv_rotation = 0.5);

    std::fs::write(
        opened.dir.path().join("assets").join(SOIL),
        material_file(0.5),
    )
    .expect("the file returns");
    jackdaw::asset_index::rescan_asset_index(opened.app.world_mut());
    opened.settle();

    assert_eq!(opened.handles(), before);
    let world = opened.app.world();
    let roughness = world
        .resource::<Assets<StandardMaterial>>()
        .get(&before[0])
        .expect("the material is held")
        .perceptual_roughness;
    assert_eq!(roughness, 0.5, "what the file holds now");
    assert!(
        !world
            .resource::<jackdaw_bsn::UnresolvedReferences>()
            .is_retired(before[0].id().untyped()),
        "it has a file again"
    );
    assert_eq!(opened.saved_materials(), SAVED);
}

/// A rename in the editor keeps the one handle under its new file, so the old
/// path is not held as gone.
#[test]
fn renaming_a_material_in_the_editor_names_the_new_file() {
    let mut opened = open_scene();
    let handles = opened.handles();
    opened
        .app
        .world_mut()
        .resource_mut::<jackdaw::material_preview::MaterialPreviewState>()
        .active_material = Some(handles[0].clone());
    opened
        .app
        .world_mut()
        .operator("material.save")
        .param("material", SOIL.to_string())
        .param("name", "loam".to_string())
        .call()
        .expect("material.save dispatches")
        .assert_finished();
    opened.settle();
    opened.mutate_brush(|brush| brush.faces[1].uv_rotation = 0.5);

    assert_eq!(opened.handles(), handles, "the one handle");
    assert_eq!(
        opened.document_material(0),
        named("materials/loam.bsn"),
        "the sync names the new file"
    );
    let saved = opened.saved_materials();
    assert_eq!(saved[0], "materials/loam.bsn", "{saved:?}");
}

/// A file arriving during Play leaves an entity holding the game's values alone,
/// so nothing it holds reaches the document with no edit. A stop links it.
#[test]
fn a_file_arriving_during_play_waits_for_the_stop() {
    let mut opened = open_scene_naming(LATE);
    let brush = opened.brush;
    let waiting = opened.handles();
    let world = opened.app.world_mut();
    world.get_mut::<Brush>(brush).expect("live").faces[4].uv_rotation = 0.5;
    jackdaw::pie_projection::mark_streamed(world, brush, BRUSH);
    opened.settle();

    file_appears(&mut opened);
    opened.settle();

    let ast = opened.app.world().resource::<SceneBsnAst>();
    let node = ast.ast_for(brush).expect("the brush is in the document");
    assert_ne!(
        get_bsn_field(ast, node, BRUSH, "faces[4].uv_rotation"),
        Some(BsnValue::Float(0.5)),
        "the game's value stays out of the document"
    );
    assert_eq!(opened.handles(), waiting, "the entity is left alone");

    jackdaw::pie_projection::revert_preview(opened.app.world_mut());
    opened.settle();

    assert!(
        !holds_no_asset(&opened.handles()[1]),
        "the stop links @late"
    );
    assert_eq!(opened.saved_materials()[2], LATE_FILE);
}

/// A name answering again for what a brush already holds writes nothing, so the
/// brush does not remesh.
#[test]
fn a_reference_arriving_for_a_handle_already_held_leaves_the_brush_alone() {
    let mut opened = open_scene_naming(LATE);
    file_appears(&mut opened);
    opened.settle();
    let brush = opened.brush;
    let world = opened.app.world_mut();
    let before = world
        .entity(brush)
        .get_ref::<Brush>()
        .expect("live")
        .last_changed();
    world
        .resource::<jackdaw_bsn::UnresolvedReferences>()
        .note(LATE_FILE, std::any::TypeId::of::<StandardMaterial>());
    world.resource_mut::<BsnSceneAssets>().set_changed();
    opened.settle();

    let after = opened
        .app
        .world()
        .entity(brush)
        .get_ref::<Brush>()
        .expect("live")
        .last_changed();
    assert_eq!(after, before, "nothing wrote the brush");
}

/// An instance of a prefab whose one brush face spells `@late`, and its brush.
fn spawn_late_prefab_instance(opened: &mut Opened) -> Entity {
    let prefab = opened.dir.path().join("assets/crate.bsn");
    let prefab_id = |id: i128| {
        BsnPatch::TupleStruct(jackdaw_bsn::BsnTupleStructData {
            type_path: "jackdaw::prefab::components::PrefabEntityId".to_string(),
            values: vec![BsnValue::Int(id)],
        })
    };
    let brush = BsnPatch::Struct(jackdaw_bsn::BsnStructData {
        type_path: BRUSH.to_string(),
        fields: jackdaw_bsn::BsnStructFields(vec![jackdaw_bsn::BsnField {
            name: "faces".to_string(),
            value: BsnValue::List(vec![BsnValue::Struct(jackdaw_bsn::BsnStructData {
                type_path: "jackdaw_geometry::BrushFaceData".to_string(),
                fields: jackdaw_bsn::BsnStructFields(vec![jackdaw_bsn::BsnField {
                    name: "material".to_string(),
                    value: BsnValue::String("@late".to_string()),
                }]),
            })]),
        }]),
    });
    let mut doc = SceneBsnAst::default();
    let root = doc.create_entity_node(vec![
        BsnPatch::Type("jackdaw::prefab::components::Prefab".to_string()),
        prefab_id(0),
        BsnPatch::Name("Crate".to_string()),
    ]);
    doc.add_to_roots(root);
    let child = doc.create_entity_node(vec![
        prefab_id(1),
        BsnPatch::Name("CrateBody".to_string()),
        brush,
    ]);
    doc.add_child_to_ast(root, child);
    std::fs::write(&prefab, jackdaw_bsn::emit_scene(&doc)).expect("the prefab is written");
    jackdaw::prefab::operators::spawn_instance(opened.app.world_mut(), &prefab, Vec3::ZERO);
    opened.app.update();
    opened.entity_named("CrateBody")
}

/// The instance's face reads the file's path once it arrives, and that is the
/// asset the baseline's `@late` stands for: no override.
#[test]
fn a_prefab_face_spelled_by_name_stays_inherited_once_its_file_arrives() {
    let mut opened = open_scene();
    let body = spawn_late_prefab_instance(&mut opened);
    file_appears(&mut opened);
    opened.settle();
    let face = opened.app.world().get::<Brush>(body).expect("live").faces[0]
        .material
        .clone();
    assert!(!holds_no_asset(&face), "the instance links up");

    let saved = opened.save();
    assert!(
        !saved.contains(LATE_FILE),
        "the inherited face is an override: {saved}"
    );
}

/// A face waiting on its name reads as that name, so it matches the baseline.
#[test]
fn a_waiting_face_in_a_saved_prefab_instance_stays_sparse() {
    let mut opened = open_scene();
    spawn_late_prefab_instance(&mut opened);
    opened.settle();

    let saved = opened.save();
    assert!(
        !saved.contains("@late"),
        "the inherited face is an override: {saved}"
    );
}

/// A component with a handle, as an extension's brush default might hold.
#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Dressed {
    material: Handle<StandardMaterial>,
    spare: Option<Handle<StandardMaterial>>,
}

/// A default a new brush takes names its asset, as the sync of any other does.
#[test]
fn a_brush_default_holding_a_handle_names_it_in_the_document() {
    let mut opened = open_scene();
    let soil = opened.handles()[0].clone();
    let world = opened.app.world_mut();
    world
        .resource::<AppTypeRegistry>()
        .write()
        .register::<Dressed>();
    world.spawn(jackdaw::brush::BrushDefault(Box::new(Dressed {
        material: soil.clone(),
        spare: Some(soil),
    })));
    world
        .operator("entity.add.cube".to_string())
        .param("name", "Dressed".to_string())
        .call()
        .expect("entity.add.cube dispatches")
        .assert_finished();
    opened.settle();

    let cube = opened.entity_named("Dressed");
    let path = <Dressed as bevy::reflect::TypePath>::type_path();
    let ast = opened.app.world().resource::<SceneBsnAst>();
    let node = ast.ast_for(cube).expect("the cube is in the document");
    assert_eq!(get_bsn_field(ast, node, path, "material"), named(SOIL));
    assert_eq!(get_bsn_field(ast, node, path, "spare"), named(SOIL));
}

/// The stream only ever writes a stack, so one coming off an entity is the editor's.
#[test]
fn removing_a_stack_the_stream_wrote_reaches_the_document() {
    use jackdaw_geometry::{MeshMirror, Modifier, ModifierEntry, ModifierStack};

    let type_path = <ModifierStack as bevy::reflect::TypePath>::type_path();
    let mut opened = open_scene();
    let brush = opened.brush;
    opened
        .app
        .world_mut()
        .entity_mut(brush)
        .insert(ModifierStack {
            modifiers: vec![ModifierEntry::new(Modifier::Mirror(MeshMirror::default()))],
        });
    opened.settle();
    let world = opened.app.world_mut();
    jackdaw::pie_projection::mark_streamed(world, brush, type_path);
    world.entity_mut(brush).remove::<ModifierStack>();
    opened.settle();

    let ast = opened.app.world().resource::<SceneBsnAst>();
    let node = ast.ast_for(brush).expect("the brush is in the document");
    assert!(
        ast.find_patch_by_type_path(node, type_path).is_none(),
        "the removal reaches the document"
    );
}

/// What an agent reads names what the document names, where `""` would say
/// every material was cleared.
#[test]
fn the_remote_view_of_a_brush_names_its_materials() {
    let mut opened = open_scene_naming(NAMED_AND_WAITING);
    let bits = opened.brush.to_bits();
    let answer = opened
        .app
        .world_mut()
        .run_system_cached_with(
            jackdaw::remote::server::entity_handler,
            Some(serde_json::json!({ "entity": bits })),
        )
        .expect("the handler ran")
        .unwrap_or_else(|err| panic!("refused: {}", err.message));
    let bsn = answer["bsn"].as_str().expect("text");
    assert_eq!(materials_in(bsn), NAMED_AND_WAITING, "{bsn}");
}

/// An asset row shows the name a placeholder or a gone file stands for.
#[test]
fn an_asset_row_names_what_a_missing_reference_stands_for() {
    let mut opened = open_scene_naming(NAMED_AND_WAITING);
    file_goes(&mut opened, MOSS);
    let handles = opened.handles();
    let world = opened.app.world();
    let registry = world.resource::<AppTypeRegistry>().read();
    let text = |handle: &Handle<StandardMaterial>| {
        jackdaw::typed_values::asset_path_json(
            &registry,
            world.get_resource(),
            world.get_resource(),
            world.get_resource(),
            handle,
        )
    };
    assert_eq!(text(&handles[1]), Some(serde_json::json!("#Still")));
    assert_eq!(text(&handles[3]), Some(serde_json::json!(MOSS)));
    assert_eq!(text(&Handle::default()), Some(serde_json::json!("")));
}

const GONE_FILE: &str = "materials/gone.bsn";

/// A path whose file is not there waits as any other name does: it draws the
/// default, says which file is missing, stays through a save and links up when
/// the file comes back. The asset server is never asked for it.
#[test]
fn a_path_whose_file_is_missing_waits_like_any_name() {
    let mut opened = open_scene_naming([GONE_FILE; 6]);
    opened.settle();

    let waiting = opened.handles()[0].clone();
    assert!(is_placeholder(&waiting), "the face waits on its path");
    let world = opened.app.world();
    assert_eq!(
        world
            .resource::<jackdaw_bsn::UnresolvedReferences>()
            .name_of(waiting.id().untyped()),
        Some(GONE_FILE.to_string()),
        "what the card reads to say which file is missing"
    );
    assert!(
        world
            .resource::<AssetServer>()
            .get_path_id(GONE_FILE)
            .is_none(),
        "nothing asked the asset server for it"
    );

    let brush = opened.brush;
    let world = opened.app.world_mut();
    let chunks: Vec<_> = world
        .query::<&jackdaw::brush::BrushMeshChunk>()
        .iter(world)
        .filter(|chunk| chunk.brush_entity == brush)
        .map(|chunk| (chunk.uses_default_material, chunk.material.clone()))
        .collect();
    assert!(!chunks.is_empty(), "the brush has chunks");
    let stored = world.resource::<Assets<StandardMaterial>>();
    for (uses_default, material) in chunks {
        assert!(uses_default, "drawn as the default");
        assert!(stored.contains(&material), "with a material that exists");
    }

    opened.mutate_brush(|brush| brush.faces[1].uv_rotation = 0.5);
    assert_eq!(
        opened.document_material(0),
        named(GONE_FILE),
        "after a sync"
    );
    assert_eq!(opened.saved_materials(), [GONE_FILE; 6]);

    std::fs::write(
        opened.dir.path().join("assets").join(GONE_FILE),
        material_file(0.5),
    )
    .expect("the file returns");
    jackdaw::asset_index::rescan_asset_index(opened.app.world_mut());
    opened.settle();
    let linked = opened.handles()[0].clone();
    assert!(!is_placeholder(&linked), "the face links up");
    assert!(
        opened
            .app
            .world()
            .resource::<Assets<StandardMaterial>>()
            .contains(&linked),
        "and holds what the file holds"
    );
    assert_eq!(opened.saved_materials(), [GONE_FILE; 6]);
}

/// Only a document with no file changes. A document no kind claims, and a path to
/// another asset type whether or not its file is there, still go to the asset
/// server, which keeps the path on the handle.
#[test]
fn a_path_with_a_file_or_of_another_asset_type_loads_through_the_server() {
    const PLAIN: &str = "materials/plain.bsn";
    let dir = tempfile::tempdir().expect("tempdir");
    write_project(&dir, &scene_text_naming([PLAIN; 6]));
    let assets = dir.path().join("assets");
    std::fs::write(assets.join(PLAIN), "#Plain\n").expect("a document");
    let opened = open_project(dir);
    let held = opened.handles()[0].clone();
    assert!(!is_placeholder(&held), "a file that is there is asked for");
    assert_eq!(
        held.path().map(ToString::to_string),
        Some(PLAIN.to_string()),
        "and keeps its path"
    );

    for image in ["textures/gone.png", "textures/notes.xyz"] {
        let dir = tempfile::tempdir().expect("tempdir");
        write_project(
            &dir,
            &format!("#Holder\nbevy_sprite::sprite::Sprite {{ image: \"{image}\" }}\n"),
        );
        let assets = dir.path().join("assets");
        std::fs::create_dir_all(assets.join("textures")).expect("a folder");
        std::fs::write(assets.join("textures/notes.xyz"), "no loader").expect("a file");
        let mut app = util::editor_test_app_reading(&assets);
        app.insert_resource(ProjectRoot::new(
            dir.path().to_path_buf(),
            ProjectConfig::default(),
        ));
        request_open_with_conversion(app.world_mut(), &assets.join("scene.bsn"));
        util::settle_scene_load(&mut app);
        let holder = app
            .world_mut()
            .query_filtered::<Entity, With<Sprite>>()
            .iter(app.world())
            .next()
            .expect("the sprite spawned");
        let held = &app.world().get::<Sprite>(holder).expect("a sprite").image;
        assert!(!is_placeholder(held), "{image} goes to the server");
        assert_eq!(
            held.path().map(ToString::to_string),
            Some(image.to_string())
        );
    }
}

impl Opened {
    /// Save the brush's live values to the scene, as the Live view's action does.
    fn save_live_to_scene(&mut self) {
        let brush = self.brush;
        let world = self.app.world_mut();
        world.resource_mut::<Selection>().entities = vec![brush];
        jackdaw::pie::save_live_entity_to_scene(world);
        self.settle();
    }

    fn view_live(&mut self) {
        *self
            .app
            .world_mut()
            .resource_mut::<jackdaw::pie_mirror::PieViewMode>() =
            jackdaw::pie_mirror::PieViewMode::Live;
    }

    fn notice(&self) -> String {
        self.app
            .world()
            .resource::<jackdaw::status_bar::StatusNotice>()
            .text()
            .to_string()
    }
}

/// A material held only in memory has no name to write, so the document leaves it
/// out and the live face keeps it, through undo and redo, for a save to embed.
#[test]
fn saving_a_live_brush_to_the_scene_keeps_a_material_held_only_in_memory() {
    let mut opened = open_scene();
    let handmade = opened
        .app
        .world_mut()
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial::default());
    let held = handmade.clone();
    opened.mutate_brush(move |brush| brush.faces[0].material = held);
    opened.view_live();
    opened.stream_brush(|brush| brush.faces[1].uv_rotation = 0.5);
    assert_eq!(opened.handles()[0], handmade, "the stream keeps it");

    opened.save_live_to_scene();
    assert_eq!(opened.handles()[0], handmade, "after the save to scene");
    assert_eq!(opened.document_material(1), named(MOSS));

    opened.history("history.undo");
    opened.settle();
    assert_eq!(opened.handles()[0], handmade, "after undo");
    opened.history("history.redo");
    opened.settle();
    assert_eq!(opened.handles()[0], handmade, "after redo");

    let saved = opened.saved_materials();
    assert!(
        saved[0].starts_with('#'),
        "embedded under a name: {saved:?}"
    );
}

/// A face the game added, outside the brush, so it changes no other.
fn another_face() -> jackdaw_geometry::BrushFaceData {
    jackdaw_geometry::BrushFaceData {
        plane: jackdaw_geometry::BrushPlane {
            normal: Vec3::Y,
            distance: 2.0,
        },
        ..default()
    }
}

/// The game changing a list's length leaves every streamed element without a
/// handle. The names stay on the elements that still match, by content.
#[test]
fn saving_a_live_list_of_another_length_keeps_the_names_of_matching_elements() {
    let mut opened = open_scene_naming(NAMED_AND_WAITING);
    opened.view_live();
    opened.stream_brush(|brush| brush.faces.push(another_face()));

    opened.save_live_to_scene();

    assert_eq!(opened.live().faces.len(), 7);
    for (face, name) in NAMED_AND_WAITING.iter().enumerate() {
        assert_eq!(opened.document_material(face), named(name), "face {face}");
    }
    assert_eq!(
        opened.document_material(6),
        None,
        "the new face names nothing"
    );
    assert_eq!(opened.notice(), "", "nothing was dropped");
    assert_eq!(opened.saved_materials(), NAMED_AND_WAITING);
}

/// An element the game changed has nothing to match, so its name goes, and the
/// user is told.
#[test]
fn a_name_with_no_matching_element_is_dropped_with_a_warning() {
    let mut opened = open_scene_naming(NAMED_AND_WAITING);
    opened.view_live();
    opened.stream_brush(|brush| {
        brush.faces.push(another_face());
        brush.faces[1].uv_rotation = 0.5;
    });

    opened.save_live_to_scene();

    assert_eq!(opened.document_material(0), named(SOIL));
    assert_eq!(opened.document_material(1), None);
    assert_eq!(opened.document_material(2), named("@still"));
    assert!(
        opened.notice().contains("#Still"),
        "says which name went: {:?}",
        opened.notice()
    );
}

/// A file going is put in front of the user, once.
#[test]
fn a_file_going_is_noticed_once() {
    let mut opened = open_scene();
    file_goes(&mut opened, SOIL);
    assert!(
        opened.notice().contains(SOIL),
        "the status bar names it: {:?}",
        opened.notice()
    );

    opened
        .app
        .world_mut()
        .resource_mut::<jackdaw::status_bar::StatusNotice>()
        .show("something else");
    jackdaw::asset_index::rescan_asset_index(opened.app.world_mut());
    assert_eq!(opened.notice(), "something else", "and only once");
}

/// A face whose file went still wears that material and the preview shows it, so
/// the Material row names the file too.
#[test]
fn the_material_row_names_a_file_that_went() {
    let mut opened = open_scene();
    file_goes(&mut opened, SOIL);
    let brush = opened.brush;
    assert_eq!(
        jackdaw::inspector::material_path(opened.app.world(), brush),
        SOIL
    );
}
