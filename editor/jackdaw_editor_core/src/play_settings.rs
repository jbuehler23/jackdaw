//! The launch environment Play hands the game it starts.
//!
//! A run configuration in the project's committed settings says how the game
//! is launched for everyone. These are one developer's own additions on top of
//! it: the variables and arguments that point a launch at a particular server,
//! identity or offline mode. They live with the rest of the project's editor
//! preferences, which no project commits, and their values are never logged.

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

/// Environment variables and arguments the editor adds to every game it
/// launches for this project.
#[derive(Resource, Default, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct PlaySettings {
    /// Variables added to the launched process's environment, overriding the
    /// run configuration's own.
    pub env: BTreeMap<String, String>,
    /// Arguments appended to the ones the run configuration names.
    pub args: Vec<String>,
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
    ctx.register_operator::<PlaySettingsOp>();
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
}

fn store(world: &World) {
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
    let mut written = PlaySettings::default();
    for (field, value) in fields {
        match field {
            PlaySettingsField::Env => written.env = parse_env(&value.0),
            PlaySettingsField::Args => written.args = parse_args(&value.0),
        }
    }
    commands.queue(move |world: &mut World| {
        *world.resource_mut::<PlaySettings>() = written;
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
            args: Vec::new(),
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
        });

        store(&world);
        let read: PlaySettings = crate::project_settings::load_section(
            project.path(),
            crate::project_settings::Section::Key(SECTION),
        );

        assert_eq!(read.env_line(), "REALM=dev");
        assert_eq!(read.args_line(), "--offline");
        assert!(
            crate::project_settings::settings_path(project.path())
                .starts_with(project.path().join(".jackdaw")),
            "the values stay where the project keeps preferences it does not commit",
        );
    }
}
