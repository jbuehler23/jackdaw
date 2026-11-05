//! What a running game spawns a scene from.
//!
//! A scene root in the game asks by asset path ([`StateEvent::RequestScene`]).
//! [`scene_answer`] replies with the document's current text, for a scene open
//! in a tab, or tells the game to load the file. Nothing is written: the text
//! holds the node ids a file saved before they existed lacks, and unsaved edits.
//!
//! [`StateEvent::RequestScene`]: jackdaw_pie_protocol::StateEvent::RequestScene

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use bevy::asset::UntypedAssetId;
use bevy::prelude::*;
use jackdaw_pie_protocol::SceneAnswer;
use jackdaw_pie_protocol::manifest::RunConfig;

use crate::pie::{MAIN_SCENE, open_scene_asset_path};
use crate::scene_io::asset_fields::{AbsoluteAssetField, refusal};
use crate::scenes::{Scenes, TabContent, TabKind};

/// What an instance plays in place of the scene its game usually asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchScene {
    /// The asset path this stands in for.
    pub replaces: String,
    /// What the game gets when it asks for `replaces`.
    pub scene: PlayScene,
}

/// A scene recorded when Play was pressed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlayScene {
    /// The scene at this asset path.
    Path(String),
    /// The tab of this name, which has no file.
    Untitled(String),
}

/// The scene the scene in front makes `run` play, if any. A run configuration
/// or project setting that names a scene keeps it, so the scene in front does
/// not take its place.
pub fn launch_scene(world: &World, run: &RunConfig) -> Option<LaunchScene> {
    let names_scene = run.env.contains_key("JACKDAW_PIE_SCENE")
        || world
            .get_resource::<crate::play_settings::PlaySettings>()
            .is_some_and(|settings| settings.env.contains_key("JACKDAW_PIE_SCENE"));
    if names_scene {
        return None;
    }
    let scenes = world.get_resource::<Scenes>()?;
    let tab = scenes.tabs.get(scenes.active)?;
    if tab.kind != TabKind::Scene || tab.is_refused() {
        return None;
    }
    let scene = match &tab.path {
        None if crate::scenes::operators::active_tab_modified(world) => {
            PlayScene::Untitled(tab.display_name.clone())
        }
        None => return None,
        Some(_) => PlayScene::Path(open_scene_asset_path(world).filter(|s| s != MAIN_SCENE)?),
    };
    let replaces = run
        .env
        .get("JACKDAW_PIE_REPLACES")
        .cloned()
        .unwrap_or_else(|| MAIN_SCENE.to_string());
    Some(LaunchScene { replaces, scene })
}

/// What the game spawns for a root asking for the asset path `requested`, for an
/// instance launched with `launch`. The scene the path resolves to, if open in a
/// tab, answers with its document; anything else answers with the file.
pub fn scene_answer(
    world: &mut World,
    launch: Option<&LaunchScene>,
    requested: &str,
) -> SceneAnswer {
    let requested = requested.replace('\\', "/");
    let target = match launch {
        Some(launch) if launch.replaces == requested => launch.scene.clone(),
        _ => PlayScene::Path(requested.clone()),
    };
    let served = match &target {
        PlayScene::Path(path) => ServedAs::of(path, None),
        PlayScene::Untitled(name) => ServedAs::of(&requested, Some(name.clone())),
    };
    let file = match &target {
        PlayScene::Path(path) => path.clone(),
        PlayScene::Untitled(_) => requested.clone(),
    };
    let use_file = SceneAnswer::UseFile { path: file };

    let Some(assets) = crate::asset_index::assets_dir(world) else {
        return use_file;
    };
    let Some(tab) = tab_of(world, &target, &assets) else {
        if let PlayScene::Untitled(name) = &target {
            let message = format!(
                "{name} was saved or closed during the build, so the game loads {requested}"
            );
            warn!("PIE: {message}");
            crate::status_bar::notify_warn(world, message);
        }
        return use_file;
    };
    let parent = tab_folder(world, tab, &assets, &served.dir);
    let Some((text, absolute)) = document_of_tab(world, tab, &parent) else {
        return use_file;
    };
    if let Err(err) = jackdaw_bsn::parse_emitted(&text) {
        warn!("PIE: the document for {requested} does not parse back, using its file: {err}");
        return use_file;
    }
    warn_absolute(world, &absolute);
    SceneAnswer::Document {
        text,
        dir: served.dir,
        stem: served.stem,
    }
}

/// The folder and name a document goes by in the game.
struct ServedAs {
    dir: String,
    stem: String,
}

impl ServedAs {
    fn of(path: &str, name: Option<String>) -> Self {
        let path = Path::new(path);
        let dir = path
            .parent()
            .map(|dir| dir.to_string_lossy().into_owned())
            .unwrap_or_default();
        let stem = name.unwrap_or_else(|| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default()
        });
        Self { dir, stem }
    }
}

/// The index of the tab holding `target`, when it holds a scene that loaded.
fn tab_of(world: &World, target: &PlayScene, assets: &Path) -> Option<usize> {
    let scenes = world.get_resource::<Scenes>()?;
    scenes.tabs.iter().position(|tab| {
        tab.kind == TabKind::Scene
            && !tab.is_refused()
            && match target {
                PlayScene::Untitled(name) => tab.path.is_none() && tab.display_name == *name,
                PlayScene::Path(path) => tab.path.as_deref().is_some_and(|held| {
                    crate::preview_model::relative_to_assets(assets, held).as_deref()
                        == Some(path.as_str())
                }),
            }
    })
}

/// Where the tab's document counts as being, on disk.
fn tab_folder(world: &World, tab: usize, assets: &Path, dir: &str) -> PathBuf {
    world.resource::<Scenes>().tabs[tab]
        .path
        .as_deref()
        .and_then(Path::parent)
        .map_or_else(|| assets.join(dir), Path::to_path_buf)
}

/// The text of the document in tab `index`, read from the document: the live one
/// for the tab in front, and the snapshot for any other.
fn document_of_tab(
    world: &mut World,
    index: usize,
    parent: &Path,
) -> Option<(String, Vec<AbsoluteAssetField>)> {
    let scenes = world.resource::<Scenes>();
    if scenes.active == index {
        if !world.contains_resource::<jackdaw_bsn::SceneBsnAst>() {
            return None;
        }
        return Some(crate::scene_io::save::emit_bsn_scene_for_game(
            world, parent,
        ));
    }
    match &scenes.tabs[index].content {
        TabContent::Scene(Some(document)) => Some(crate::scene_io::save::emit_snapshot_for_game(
            world, document, parent,
        )),
        TabContent::Scene(None) | TabContent::Prefab(_) => None,
    }
}

fn warn_absolute(world: &mut World, absolute: &[AbsoluteAssetField]) {
    if absolute.is_empty() {
        return;
    }
    let message = format!("Playing anyway: {}", refusal(absolute));
    warn!("PIE: {message}");
    crate::status_bar::notify_warn(world, message);
}

/// The `#Name`s runtime assets go by for this Play session, by asset. A file
/// numbers them in document order, so an asset added earlier in the document
/// would renumber the rest between two emits and leave the game holding names
/// that now mean something else. These never change until Play stops, and are
/// spelled so they cannot collide with a name a file embeds.
#[derive(Resource, Default)]
pub struct PlayAssetNames {
    names: HashMap<UntypedAssetId, String>,
    next: usize,
}

impl PlayAssetNames {
    /// The name of the asset `id`, without the `#`, minted on first use.
    pub(crate) fn name_of(&mut self, id: UntypedAssetId, short_type_name: &str) -> String {
        let next = &mut self.next;
        let name = self.names.entry(id).or_insert_with(|| {
            let name = format!("#Play{short_type_name}{next}");
            *next += 1;
            name
        });
        name.trim_start_matches('#').to_string()
    }

    /// Each asset named so far with its `#Name`.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&UntypedAssetId, &String)> {
        self.names.iter()
    }

    /// How many assets have a name.
    pub fn len(&self) -> usize {
        self.names.len()
    }

    /// Whether no asset has a name.
    pub fn is_empty(&self) -> bool {
        self.names.is_empty()
    }
}

pub(crate) fn forget_asset_names(mut names: ResMut<PlayAssetNames>) {
    *names = PlayAssetNames::default();
}

#[cfg(test)]
mod tests {
    use bevy::asset::{AssetPlugin, ReflectAsset, ReflectHandle};
    use jackdaw_bsn::{SceneBsnAst, component_to_bsn_patch};
    use jackdaw_pie_protocol::{RemoteEntity, StateEvent};
    use jackdaw_runtime::JackdawPlugin;
    use jackdaw_scene_types::SceneNodeId;

    use super::*;
    use crate::pie_projection::{PieProjection, project_event};
    use crate::play_sync::AssetReferences;

    const TRANSFORM: &str = "bevy_transform::components::transform::Transform";
    const PAINTED: &str = "game::Painted";

    #[derive(Asset, Reflect, Default, Clone)]
    #[reflect(Asset, Default)]
    #[type_path = "game"]
    struct Swatch {
        tone: u32,
    }

    #[derive(Component, Reflect, Default)]
    #[reflect(Component, Default)]
    #[type_path = "game"]
    struct Painted {
        level: u32,
        swatch: Handle<Swatch>,
    }

    fn register(app: &mut App) {
        app.init_asset::<Swatch>();
        app.register_asset_reflect::<Swatch>();
        app.register_type_data::<Handle<Swatch>, ReflectHandle>();
        app.register_type::<Painted>();
        app.register_type::<Transform>();
        app.register_type::<SceneNodeId>();
    }

    /// An editor world with an empty document as `zone.bsn`, in front.
    fn editor(dir: &Path) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(AssetPlugin::default());
        register(&mut app);
        app.insert_resource(crate::project::ProjectRoot::new(
            dir.to_path_buf(),
            crate::project::ProjectConfig::default(),
        ));
        let mut tab = crate::scenes::SceneTab::new_untitled(1);
        tab.path = Some(dir.join("assets/zone.bsn"));
        app.insert_resource(Scenes {
            tabs: vec![tab],
            active: 0,
        });
        app.init_resource::<PieProjection>();
        app.init_resource::<PlayAssetNames>();
        app.init_resource::<SceneBsnAst>();
        app
    }

    /// A root node with the given components, linked to a new editor entity.
    fn add_node(app: &mut App, id: u64, painted: Painted, level: u32) -> Entity {
        let world = app.world_mut();
        let entity = world
            .spawn((SceneNodeId(id), Transform::default(), painted))
            .id();
        let registry = world.resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        let patches = vec![
            component_to_bsn_patch(&SceneNodeId(id), &registry),
            component_to_bsn_patch(&Transform::default(), &registry),
            component_to_bsn_patch(
                &Painted {
                    level,
                    swatch: Handle::default(),
                },
                &registry,
            ),
        ];
        let mut ast = world.resource_mut::<SceneBsnAst>();
        let node = ast.create_entity_node(patches);
        ast.add_to_roots(node);
        ast.link(entity, node);
        entity
    }

    fn swatch(app: &mut App, tone: u32) -> Handle<Swatch> {
        app.world_mut()
            .resource_mut::<Assets<Swatch>>()
            .add(Swatch { tone })
    }

    fn text_of(answer: SceneAnswer) -> String {
        match answer {
            SceneAnswer::Document { text, .. } => text,
            SceneAnswer::UseFile { path } => panic!("expected a document, got the file {path}"),
        }
    }

    fn answer(app: &mut App) -> String {
        text_of(scene_answer(app.world_mut(), None, "zone.bsn"))
    }

    /// A game on the runtime's own loader, spawning `text` as it would an answer.
    fn game_spawning(text: String) -> App {
        let mut game = App::new();
        game.add_plugins(MinimalPlugins);
        game.add_plugins(bevy::transform::TransformPlugin);
        game.add_plugins(AssetPlugin::default());
        game.add_plugins(bevy::world_serialization::WorldSerializationPlugin);
        register(&mut game);
        game.add_plugins(JackdawPlugin);
        let scene = game
            .world_mut()
            .resource_mut::<Assets<jackdaw_runtime::JackdawScene>>()
            .add(jackdaw_runtime::JackdawScene::new(text, PathBuf::new()));
        game.world_mut()
            .spawn(jackdaw_runtime::JackdawSceneRoot(scene));
        game.update();
        game
    }

    fn game_painted(game: &mut App, id: u64) -> (u32, u32) {
        let mut query = game.world_mut().query::<(&SceneNodeId, &Painted)>();
        let swatch = query
            .iter(game.world())
            .find(|(node, _)| node.0 == id)
            .map(|(_, painted)| (painted.level, painted.swatch.clone()))
            .expect("the node spawned");
        let tone = game
            .world()
            .resource::<Assets<Swatch>>()
            .get(&swatch.1)
            .map_or(u32::MAX, |swatch| swatch.tone);
        (swatch.0, tone)
    }

    fn stream_spawn(id: u64, level: u32, x: f32) -> StateEvent {
        StateEvent::EntitySpawned {
            entity: RemoteEntity {
                entity: 1,
                components: [
                    (
                        TRANSFORM.to_string(),
                        serde_json::json!({
                            "translation": [x, 0.0, 0.0],
                            "rotation": [0.0, 0.0, 0.0, 1.0],
                            "scale": [1.0, 1.0, 1.0]
                        }),
                    ),
                    (
                        PAINTED.to_string(),
                        serde_json::json!({ "level": level, "swatch": null }),
                    ),
                ]
                .into_iter()
                .collect(),
                scene_node_id: Some(id),
            },
        }
    }

    #[test]
    fn projected_game_state_never_reaches_the_text() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut app = editor(dir.path());
        let color = swatch(&mut app, 4);
        let entity = add_node(
            &mut app,
            5,
            Painted {
                level: 1,
                swatch: color.clone(),
            },
            1,
        );
        let before = answer(&mut app);

        project_event(app.world_mut(), stream_spawn(5, 99, 77.0));
        let world = app.world();
        let projected = world.get::<Painted>(entity).expect("the component");
        assert_eq!(projected.level, 99, "the entity shows the game's value");
        assert_eq!(
            world
                .get::<Transform>(entity)
                .expect("a transform")
                .translation
                .x,
            77.0
        );
        assert_eq!(projected.swatch, color, "the stream carries no handle");

        let after = answer(&mut app);
        assert_eq!(after, before, "the document alone decides the text");
        assert!(!after.contains("99") && !after.contains("77"), "{after}");
        assert_eq!(game_painted(&mut game_spawning(after), 5), (1, 4));
    }

    /// A document edit made while the entity is projected is in the text, next
    /// to no game value.
    #[test]
    fn an_edit_of_a_projected_entity_is_read_from_the_document() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut app = editor(dir.path());
        let color = swatch(&mut app, 4);
        let entity = add_node(
            &mut app,
            5,
            Painted {
                level: 1,
                swatch: color,
            },
            1,
        );
        project_event(app.world_mut(), stream_spawn(5, 99, 0.0));
        let registry = app.world().resource::<AppTypeRegistry>().clone();
        let registry = registry.read();
        app.world_mut()
            .resource_scope(|_, mut ast: Mut<SceneBsnAst>| {
                let node = ast.ast_for(entity).expect("a node");
                let value = jackdaw_bsn::BsnValue::from_reflect(&12u32, &registry);
                jackdaw_bsn::set_bsn_field(&mut ast, node, PAINTED, "level", value, &registry);
            });
        drop(registry);
        assert_eq!(
            game_painted(&mut game_spawning(answer(&mut app)), 5),
            (12, 4)
        );
    }

    /// A tab switched away from while projected keeps the document's values.
    #[test]
    fn a_snapshot_taken_while_projected_holds_the_document() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut app = editor(dir.path());
        let color = swatch(&mut app, 4);
        add_node(
            &mut app,
            5,
            Painted {
                level: 1,
                swatch: color,
            },
            1,
        );
        project_event(app.world_mut(), stream_spawn(5, 99, 0.0));
        let text = crate::scene_io::emit_bsn_scene_with_inline_assets(app.world_mut(), dir.path());
        assert!(!text.contains("99"), "{text}");
    }

    /// Asset names hold across emits with an asset added ahead in the document,
    /// and live sync's references read the same ones.
    #[test]
    fn an_unsaved_asset_keeps_its_name_across_emits() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut app = editor(dir.path());
        let late = swatch(&mut app, 6);
        add_node(
            &mut app,
            8,
            Painted {
                level: 1,
                swatch: late.clone(),
            },
            1,
        );
        let first = answer(&mut app);
        let name = app
            .world()
            .resource::<PlayAssetNames>()
            .iter()
            .next()
            .map(|(_, name)| name.clone())
            .expect("the asset was named");
        assert!(first.contains(&name), "{first}");
        assert_eq!(game_painted(&mut game_spawning(first), 8), (1, 6));

        let early = swatch(&mut app, 3);
        let added = add_node(
            &mut app,
            9,
            Painted {
                level: 2,
                swatch: early,
            },
            2,
        );
        let world = app.world_mut();
        let mut ast = world.resource_mut::<SceneBsnAst>();
        let node = ast.ast_for(added).expect("a node");
        let moved: Vec<Entity> = std::iter::once(node)
            .chain(ast.roots.iter().copied().filter(|&root| root != node))
            .collect();
        ast.roots = moved;

        let second = answer(&mut app);
        assert!(
            second.contains(&name),
            "the name still means the asset:\n{second}"
        );
        let mut game = game_spawning(second);
        assert_eq!(game_painted(&mut game, 8), (1, 6));
        assert_eq!(game_painted(&mut game, 9), (2, 3));
        assert_eq!(
            AssetReferences::of(app.world()).reference(&late.untyped()),
            Some(name),
            "live sync sends the same name"
        );
    }

    #[test]
    fn forgetting_the_names_starts_over_at_stop() {
        let dir = tempfile::tempdir().expect("a temp dir");
        let mut app = editor(dir.path());
        let color = swatch(&mut app, 4);
        add_node(
            &mut app,
            5,
            Painted {
                level: 1,
                swatch: color,
            },
            1,
        );
        answer(&mut app);
        assert_eq!(app.world().resource::<PlayAssetNames>().len(), 1);
        app.world_mut()
            .run_system_cached(forget_asset_names)
            .expect("it runs");
        assert!(app.world().resource::<PlayAssetNames>().is_empty());
    }
}
