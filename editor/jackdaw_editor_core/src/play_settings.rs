//! The launch environment Play hands the game it starts.
//!
//! A run configuration in the project's committed settings says how the game
//! is launched for everyone. These are one developer's own additions on top of
//! it: the variables and arguments that point a launch at a particular server,
//! identity or offline mode. They live with the rest of the project's editor
//! preferences, which no project commits, and their values are never logged.
//! The same section holds what becomes of edits made during Play, where the
//! game's window goes, the Game panel's view size and whether the game is
//! muted.

use std::collections::BTreeMap;
use std::path::PathBuf;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_feathers::dialog::{
    DialogActionEvent, DialogChildrenSlot, EditorDialog, OpenDialogEvent,
};
use jackdaw_feathers::text_edit::{TextEditProps, TextEditValue, text_edit};
use jackdaw_feathers::tokens;

const SECTION: &str = "play";

/// One developer's Play preferences for this project: the environment variables
/// and arguments the editor adds to every game it launches, and what becomes of
/// the edits made during Play.
#[derive(Resource, Default, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PlaySettings {
    /// Variables added to the launched process's environment, overriding the
    /// run configuration's own.
    pub env: BTreeMap<String, String>,
    /// Arguments appended to the ones the run configuration names.
    pub args: Vec<String>,
    /// What becomes of the edits made during Play once it stops.
    pub edits: PlayEdits,
    /// Whether a launch opens the game in its own window.
    pub separate_window: bool,
    /// The size the Game panel asks the game to render at.
    pub view: GameView,
    /// Whether the games Play runs are silenced.
    pub muted: bool,
}

/// The size the Game panel asks the game to render at: the panel's own, the
/// largest of an aspect that fits in it, or a fixed resolution scaled to fit.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GameView {
    #[default]
    #[serde(rename = "free")]
    Free,
    #[serde(rename = "16:9")]
    Wide,
    #[serde(rename = "4:3")]
    Standard,
    #[serde(rename = "1:1")]
    Square,
    #[serde(rename = "1280x720")]
    Hd,
    #[serde(rename = "1920x1080")]
    FullHd,
}

impl GameView {
    pub const ALL: [Self; 6] = [
        Self::Free,
        Self::Wide,
        Self::Standard,
        Self::Square,
        Self::Hd,
        Self::FullHd,
    ];

    /// The name the operator takes and the settings file holds.
    pub fn name(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::Wide => "16:9",
            Self::Standard => "4:3",
            Self::Square => "1:1",
            Self::Hd => "1280x720",
            Self::FullHd => "1920x1080",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|view| view.name().eq_ignore_ascii_case(name.trim()))
    }

    /// How the Game panel names it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Free => "Free",
            Self::Hd => "1280 x 720",
            Self::FullHd => "1920 x 1080",
            other => other.name(),
        }
    }

    /// The stream size to ask for inside a surface of `surface` pixels. An
    /// aspect's width is kept to a multiple of 64, the row the game captures
    /// at, so the frame comes back at the aspect asked for, and both sides
    /// are at least 64.
    pub fn stream_size(self, surface: UVec2) -> UVec2 {
        let aspect = match self {
            Self::Free => return surface,
            Self::Hd => return UVec2::new(1280, 720),
            Self::FullHd => return UVec2::new(1920, 1080),
            Self::Wide => UVec2::new(16, 9),
            Self::Standard => UVec2::new(4, 3),
            Self::Square => UVec2::ONE,
        };
        const ROW: u32 = 64;
        let fit_width = surface.x.min(surface.y * aspect.x / aspect.y);
        let min_width = (ROW * aspect.x).div_ceil(aspect.y).div_ceil(ROW) * ROW;
        let width = (fit_width / ROW * ROW).max(min_width);
        UVec2::new(width, width * aspect.y / aspect.x)
    }
}

/// What becomes of the edits made during Play once the last game stops.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlayEdits {
    /// They stay in the documents.
    #[default]
    Keep,
    /// Each document edited during Play goes back to where it stood when Play
    /// started.
    Revert,
}

impl PlayEdits {
    fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "keep" => Some(Self::Keep),
            "revert" => Some(Self::Revert),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Keep => "keep",
            Self::Revert => "revert",
        }
    }
}

impl PlaySettings {
    /// The environment as one editable line of `NAME=value` words.
    fn env_line(&self) -> String {
        self.env
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The arguments as one editable line.
    fn args_line(&self) -> String {
        self.args.join(" ")
    }
}

/// Read `NAME=value` words into an environment, dropping a word with no `=`.
fn parse_env(line: &str) -> BTreeMap<String, String> {
    words(line)
        .into_iter()
        .filter_map(|word| {
            word.split_once('=')
                .map(|(name, value)| (name.to_string(), value.to_string()))
        })
        .collect()
}

fn parse_args(line: &str) -> Vec<String> {
    words(line)
}

/// Split a line into words, so a quoted run of them is one value with its
/// spaces: a path, a name or a token is a word whatever it holds.
fn words(line: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quoted = false;
    let mut quote = '"';
    for character in line.chars() {
        match character {
            _ if quoted && character == quote => quoted = false,
            '"' | '\'' if !quoted => {
                quoted = true;
                quote = character;
            }
            _ if !quoted && character.is_whitespace() => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            _ => word.push(character),
        }
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}

/// Present while the Play settings dialog is the open one.
#[derive(Resource)]
struct PlaySettingsOpen;

/// Marks a field of the open dialog, so the save reads the right one.
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum PlaySettingsField {
    Env,
    Args,
}

/// Show the launch environment for editing.
#[derive(Event)]
pub struct OpenPlaySettingsEvent;

pub struct PlaySettingsPlugin;

impl Plugin for PlaySettingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PlaySettings>()
            .add_systems(
                Update,
                (load_for_open_project, populate_dialog, close_with_dialog)
                    .run_if(in_state(crate::AppState::Editor)),
            )
            .add_observer(open_dialog)
            .add_observer(save_from_dialog);
    }
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<PlaySettingsOp>()
        .register_operator::<PlayEditsOp>()
        .register_operator::<PieViewAspectOp>()
        .register_operator::<PieMuteOp>();
}

/// Set the size the Game panel asks the game to render at.
#[operator(
    id = "pie.view_aspect",
    writes_disk = true,
    allows_undo = false,
    label = "Game View Size",
    description = "Fill the Game panel, keep an aspect, or render at a fixed resolution scaled to fit.",
    params(value(
        String,
        doc = "`free`, `16:9`, `4:3`, `1:1`, `1280x720` or `1920x1080`."
    ))
)]
pub fn pie_view_aspect(params: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let name = params.as_str("value").unwrap_or_default();
    let Some(view) = GameView::parse(name) else {
        warn!("pie.view_aspect: unknown value '{name}'");
        return OperatorResult::Cancelled;
    };
    commands.queue(move |world: &mut World| {
        let Some(mut settings) = world.get_resource_mut::<PlaySettings>() else {
            warn!("pie.view_aspect: no Play settings to hold the view");
            return;
        };
        settings.view = view;
        store(world);
    });
    OperatorResult::Finished
}

/// Silence the games Play runs, or give them their sound back.
#[operator(
    id = "pie.mute",
    writes_disk = true,
    allows_undo = false,
    label = "Mute Game",
    description = "Silence the running game, and the games Play launches, until unmuted.",
    params(on(
        bool,
        doc = "Mute when true, unmute when false; switches when left out."
    ))
)]
pub fn pie_mute(params: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let named = params.as_bool("on");
    commands.queue(move |world: &mut World| {
        let Some(mut settings) = world.get_resource_mut::<PlaySettings>() else {
            warn!("pie.mute: no Play settings to hold the mute");
            return;
        };
        settings.muted = named.unwrap_or(!settings.muted);
        let muted = settings.muted;
        store(world);
        crate::pie::broadcast_control(world, jackdaw_pie_protocol::ControlEvent::SetMuted(muted));
    });
    OperatorResult::Finished
}

/// Keep or revert the edits made during Play when it stops, or switch between
/// the two when no mode is named.
#[operator(
    id = "play.edits",
    writes_disk = true,
    allows_undo = false,
    label = "Revert Edits When Play Stops",
    description = "Choose whether edits made during Play stay or are undone when the last game stops.",
    params(mode(
        String,
        doc = "`keep` or `revert`; switches between them when left out."
    ))
)]
pub fn play_edits(params: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let named = match params.as_str("mode") {
        Some(name) => match PlayEdits::parse(name) {
            Some(mode) => Some(mode),
            None => {
                warn!("play.edits: unknown mode '{name}', expected keep or revert");
                return OperatorResult::Cancelled;
            }
        },
        None => None,
    };
    commands.queue(move |world: &mut World| {
        let mode = {
            let mut settings = world.resource_mut::<PlaySettings>();
            settings.edits = named.unwrap_or(match settings.edits {
                PlayEdits::Keep => PlayEdits::Revert,
                PlayEdits::Revert => PlayEdits::Keep,
            });
            settings.edits
        };
        store(world);
        info!("PIE: edits made during Play: {}", mode.name());
    });
    OperatorResult::Finished
}

/// Set the launch environment, or show it for editing when neither is named.
#[operator(
    id = "play.settings",
    label = "Play Settings",
    description = "Set the environment variables and arguments Play hands the game.",
    params(
        env(
            String,
            doc = "Environment variables as NAME=value words, replacing the ones held."
        ),
        args(String, doc = "Arguments to pass the game, replacing the ones held.")
    )
)]
pub fn play_settings(params: In<OperatorParameters>, mut commands: Commands) -> OperatorResult {
    let env = params.as_str("env").map(str::to_string);
    let args = params.as_str("args").map(str::to_string);
    if env.is_none() && args.is_none() {
        commands.trigger(OpenPlaySettingsEvent);
        return OperatorResult::Finished;
    }
    commands.queue(move |world: &mut World| {
        {
            let mut settings = world.resource_mut::<PlaySettings>();
            if let Some(env) = &env {
                settings.env = parse_env(env);
            }
            if let Some(args) = &args {
                settings.args = parse_args(args);
            }
        }
        store(world);
    });
    OperatorResult::Finished
}

/// Read the open project's settings, once per project.
fn load_for_open_project(
    project: Option<Res<crate::project::ProjectRoot>>,
    mut settings: ResMut<PlaySettings>,
    window_mode: Option<ResMut<crate::pie::PieWindowMode>>,
    mut loaded_root: Local<Option<PathBuf>>,
) {
    let Some(project) = project else {
        return;
    };
    if loaded_root.as_ref() == Some(&project.root) {
        return;
    }
    *loaded_root = Some(project.root.clone());
    *settings = crate::project_settings::load_section(
        &project.root,
        crate::project_settings::Section::Key(SECTION),
    );
    if let Some(mut mode) = window_mode {
        *mode = crate::pie::PieWindowMode::from_separate(settings.separate_window);
    }
}

pub(crate) fn store(world: &World) {
    let (Some(project), Some(settings)) = (
        world.get_resource::<crate::project::ProjectRoot>(),
        world.get_resource::<PlaySettings>(),
    ) else {
        return;
    };
    crate::project_settings::store_section(
        &project.root,
        crate::project_settings::Section::Key(SECTION),
        settings,
    );
    let named: Vec<&str> = settings.env.keys().map(String::as_str).collect();
    match named.is_empty() {
        true => info!("PIE: the launch environment names no variable"),
        false => info!("PIE: the launch environment names {}", named.join(", ")),
    }
}

fn open_dialog(
    _: On<OpenPlaySettingsEvent>,
    mut commands: Commands,
    open: Option<Res<PlaySettingsOpen>>,
) {
    if open.is_some() {
        return;
    }
    commands.insert_resource(PlaySettingsOpen);
    commands.trigger(
        OpenDialogEvent::new("Play Settings", "Save")
            .with_description(
                "Variables and arguments the editor adds to every game it launches for this \
                 project. They stay on this machine.",
            )
            .with_max_width(px(520)),
    );
}

fn populate_dialog(
    slots: Query<Entity, Added<DialogChildrenSlot>>,
    open: Option<Res<PlaySettingsOpen>>,
    settings: Res<PlaySettings>,
    mut commands: Commands,
) {
    if open.is_none() {
        return;
    }
    for slot in slots {
        let wrapper = commands
            .spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: px(tokens::SPACING_SM),
                    width: percent(100),
                    ..Default::default()
                },
                ChildOf(slot),
            ))
            .id();
        for (field, label, value) in [
            (PlaySettingsField::Env, "Environment", settings.env_line()),
            (PlaySettingsField::Args, "Arguments", settings.args_line()),
        ] {
            commands.spawn((
                field,
                text_edit(TextEditProps {
                    label: Some(label.to_string()),
                    default_value: Some(value),
                    allow_empty: true,
                    grow: true,
                    ..Default::default()
                }),
                ChildOf(wrapper),
            ));
        }
    }
}

fn save_from_dialog(
    _: On<DialogActionEvent>,
    open: Option<Res<PlaySettingsOpen>>,
    fields: Query<(&PlaySettingsField, &TextEditValue)>,
    mut commands: Commands,
) {
    if open.is_none() || fields.is_empty() {
        return;
    }
    let mut env = None;
    let mut args = None;
    for (field, value) in fields {
        match field {
            PlaySettingsField::Env => env = Some(parse_env(&value.0)),
            PlaySettingsField::Args => args = Some(parse_args(&value.0)),
        }
    }
    commands.queue(move |world: &mut World| {
        {
            let mut settings = world.resource_mut::<PlaySettings>();
            settings.env = env.unwrap_or_default();
            settings.args = args.unwrap_or_default();
        }
        store(world);
    });
}

fn close_with_dialog(
    mut commands: Commands,
    open: Option<Res<PlaySettingsOpen>>,
    dialogs: Query<(), With<EditorDialog>>,
) {
    if open.is_some() && dialogs.is_empty() {
        commands.remove_resource::<PlaySettingsOpen>();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_of_named_values_reads_back_as_the_line_it_came_from() {
        let settings = PlaySettings {
            env: parse_env("REALM=dev TOKEN=secret"),
            args: parse_args("--offline"),
            ..Default::default()
        };

        assert_eq!(settings.env_line(), "REALM=dev TOKEN=secret");
        assert_eq!(settings.args_line(), "--offline");
    }

    #[test]
    fn a_quoted_value_keeps_its_spaces() {
        assert_eq!(
            parse_args("--realm \"my realm\" --offline"),
            vec!["--realm", "my realm", "--offline"],
        );
        assert_eq!(
            parse_env("NAME='a b' OTHER=c")
                .get("NAME")
                .map(String::as_str),
            Some("a b"),
        );
    }

    #[test]
    fn an_action_from_another_dialog_leaves_the_settings_alone() {
        let mut app = App::new();
        app.insert_resource(PlaySettings {
            env: parse_env("REALM=dev"),
            ..Default::default()
        });
        app.insert_resource(PlaySettingsOpen);
        app.add_observer(save_from_dialog);
        let elsewhere = app.world_mut().spawn_empty().id();

        app.world_mut()
            .trigger(DialogActionEvent { entity: elsewhere });
        app.update();

        assert_eq!(
            app.world().resource::<PlaySettings>().env_line(),
            "REALM=dev"
        );
    }

    #[test]
    fn an_aspect_fits_the_panel_and_a_resolution_is_asked_for_as_named() {
        let panel = UVec2::new(1000, 400);
        assert_eq!(GameView::Free.stream_size(panel), panel);
        assert_eq!(GameView::Wide.stream_size(panel), UVec2::new(704, 396));
        assert_eq!(GameView::Standard.stream_size(panel), UVec2::new(512, 384));
        assert_eq!(GameView::Square.stream_size(panel), UVec2::new(384, 384));
        assert_eq!(
            GameView::Wide.stream_size(UVec2::new(800, 1200)),
            UVec2::new(768, 432)
        );
        assert_eq!(GameView::Hd.stream_size(panel), UVec2::new(1280, 720));
        assert_eq!(GameView::FullHd.stream_size(panel), UVec2::new(1920, 1080));
        assert_eq!(
            GameView::Square.stream_size(UVec2::new(20, 20)),
            UVec2::new(64, 64),
            "a tiny panel still asks for a frame the game can capture"
        );
        assert_eq!(
            GameView::Wide.stream_size(UVec2::new(20, 20)),
            UVec2::new(128, 72),
            "both sides of a tiny frame meet the minimum at the aspect"
        );
        assert_eq!(
            GameView::Standard.stream_size(UVec2::new(20, 20)),
            UVec2::new(128, 96)
        );
    }

    #[test]
    fn a_view_reads_back_from_its_name() {
        for view in GameView::ALL {
            assert_eq!(GameView::parse(view.name()), Some(view));
        }
        assert_eq!(GameView::parse("16:10"), None);
        let json = serde_json::to_string(&GameView::Wide).expect("encode");
        assert_eq!(json, "\"16:9\"");
    }

    #[test]
    fn a_word_naming_no_value_is_left_out() {
        assert_eq!(parse_env("REALM=dev nonsense").len(), 1);
    }

    #[test]
    fn what_was_set_is_read_back_and_no_committed_file_holds_it() {
        let project = tempfile::tempdir().expect("tempdir");
        let mut world = World::new();
        world.insert_resource(crate::project::ProjectRoot {
            root: project.path().to_path_buf(),
            config: Default::default(),
        });
        world.insert_resource(PlaySettings {
            env: parse_env("REALM=dev"),
            args: parse_args("--offline"),
            edits: PlayEdits::Revert,
            separate_window: true,
            view: GameView::Wide,
            muted: true,
        });

        store(&world);
        let read: PlaySettings = crate::project_settings::load_section(
            project.path(),
            crate::project_settings::Section::Key(SECTION),
        );

        assert_eq!(read.env_line(), "REALM=dev");
        assert_eq!(read.args_line(), "--offline");
        assert_eq!(read.edits, PlayEdits::Revert);
        assert!(read.separate_window && read.muted);
        assert_eq!(read.view, GameView::Wide);
        assert!(
            crate::project_settings::settings_path(project.path())
                .starts_with(project.path().join(".jackdaw")),
            "the values stay where the project keeps preferences it does not commit",
        );
    }
}
