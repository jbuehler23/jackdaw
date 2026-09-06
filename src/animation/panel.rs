//! The dockable "Animation" panel: a clip library over the keyframe timeline.
//!
//! The Library tab answers "what can this rig play", which used to be
//! answerable only by reading the clip entities a scene had accumulated. The
//! Timeline tab is the keyframe editor, unchanged.

use bevy::prelude::*;
use bevy::reflect::TypePath;
use jackdaw_animation_runtime::{AnimationSet, AnimationStateDef};
use jackdaw_api::prelude::*;
use jackdaw_feathers::{
    button::{
        ButtonOperatorCall, ButtonProps, ButtonSize, ButtonVariant, IconButtonProps, button,
        icon_button,
    },
    icons::{Icon, IconFont},
    progress::{ProgressBarFill, progress_bar, set_progress_fill},
    tab_strip::{TabStripItem, TabStripOrientation, spawn_tab_strip},
    text_edit::{TextEditProps, TextEditValue, text_edit},
    tokens,
};

use super::library::AnimationLibrary;
use super::preview::{
    AnimationPreview, AnimationPreviewOp, AnimationPreviewPauseOp, AnimationPreviewStopOp,
};
use crate::selection::Selection;

/// Width of the file column, wide enough for a nested asset path.
const FILE_COLUMN_WIDTH: f32 = 220.0;

/// Which tab the panel shows.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum AnimationPanelTab {
    #[default]
    Library,
    Timeline,
}

/// What the Library tab is looking at.
#[derive(Resource, Default, Debug)]
pub struct AnimationPanelState {
    /// The file whose clips are listed.
    pub file: Option<String>,
    /// The clip last chosen, which is what "Add as state" appends.
    pub clip: Option<String>,
    /// What the filter field holds.
    pub filter: String,
}

/// Marker for the panel's tab-strip container.
#[derive(Component)]
struct AnimationPanelTabStrip;

/// Marker for the panel's tab-content container.
#[derive(Component)]
struct AnimationPanelBody;

/// Marker on the Library tab's filter field.
#[derive(Component)]
struct AnimationLibraryFilter;

/// Marker on the transport's progress bar.
#[derive(Component)]
struct AnimationPreviewProgress;

/// Marker on the transport's "what is playing" label.
#[derive(Component)]
struct PreviewTargetLabel;

/// Marker on the scrolling column of files.
#[derive(Component)]
struct LibraryFileList;

/// Marker on the scrolling column of clips.
#[derive(Component)]
struct LibraryClipList;

/// Marker on everything this panel's systems spawn into the window.
///
/// The dock rebuilds a window's content wholesale, which can take away a
/// container between the frame a system read it and the frame that system's
/// spawn lands. A UI node left with no parent is a root of its own and draws
/// itself over the top left of the editor, so anything of ours that ends up
/// parentless is dropped.
#[derive(Component)]
struct AnimationPanelPart;

/// What the transport says when no clip is up.
const NOTHING_PREVIEWING: &str = "nothing is previewing";

/// Switches the Animation panel between its Library and Timeline tabs.
///
/// The tab strip dispatches this operator on click, so a scripted run and a
/// click take the same path.
#[operator(
    id = "animation.panel.tab",
    label = "Animation Panel Tab",
    description = "Switch the Animation panel between its Library and Timeline tabs.",
    params(tab(
        String,
        default = "library",
        doc = "Which tab to show: \"library\" or \"timeline\"."
    ),),
    allows_undo = false
)]
pub(crate) fn animation_panel_tab(
    params: In<OperatorParameters>,
    mut tab: ResMut<AnimationPanelTab>,
) -> OperatorResult {
    *tab = match params.as_str("tab").unwrap_or("library") {
        "timeline" => AnimationPanelTab::Timeline,
        "library" => AnimationPanelTab::Library,
        other => {
            warn!("animation.panel.tab: no tab is called \"{other}\", showing the library");
            AnimationPanelTab::Library
        }
    };
    OperatorResult::Finished
}

/// Lists one file's clips in the Library tab.
#[operator(
    id = "animation.library.select",
    label = "Show Clips",
    description = "List the clips one glTF file holds.",
    params(file(String, doc = "Assets-relative path of the file whose clips to list."),),
    allows_undo = false
)]
pub(crate) fn animation_library_select(
    params: In<OperatorParameters>,
    mut state: ResMut<AnimationPanelState>,
) -> OperatorResult {
    state.file = Some(params.as_str("file").map(str::to_string)?);
    OperatorResult::Finished
}

/// Append the chosen clip to the selected entity's animation set.
#[operator(
    id = "animation.library.add_state",
    label = "Add As State",
    description = "Append the chosen library clip to the selected entity's animation set, \
                   adding its file to the set's sources when it is missing.",
    is_available = a_set_and_a_clip_are_chosen,
    allows_undo = false
)]
pub(crate) fn animation_library_add_state(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(add_chosen_clip_as_state);
    OperatorResult::Finished
}

fn a_set_and_a_clip_are_chosen(
    selection: Res<Selection>,
    state: Res<AnimationPanelState>,
    sets: Query<(), With<AnimationSet>>,
) -> bool {
    state.file.is_some()
        && state.clip.is_some()
        && selection.primary().is_some_and(|e| sets.contains(e))
}

/// Write the chosen clip into the selected set as one undoable edit.
///
/// The whole component goes through the field-set path rather than two edits
/// to `sources` and `states`, so adding a state and the source it needs is one
/// step to undo.
fn add_chosen_clip_as_state(world: &mut World) {
    let state = world.resource::<AnimationPanelState>();
    let (Some(file), Some(clip)) = (state.file.clone(), state.clip.clone()) else {
        return;
    };
    let Some(entity) = world.resource::<Selection>().primary() else {
        return;
    };
    let Some(mut set) = world.get::<AnimationSet>(entity).cloned() else {
        return;
    };

    let source = match set.sources.iter().position(|held| *held == file) {
        Some(at) => at,
        None => {
            set.sources.push(file.clone());
            set.sources.len() - 1
        }
    };
    let looped = world
        .resource::<AnimationLibrary>()
        .clip(&file, &clip)
        .is_none_or(|clip| clip.looped_hint);
    set.states.push(AnimationStateDef {
        name: unused_state_name(&set, &clip),
        source,
        clip,
        looped,
        ..default()
    });

    let registry = world.resource::<AppTypeRegistry>().clone();
    let value = {
        let registry = registry.read();
        crate::inspector::reflect_fields::reflect_to_json(&set, &registry)
    };
    let Some(value) = value else {
        warn!("animation.library.add_state: the set did not convert to a value to author");
        return;
    };
    if !crate::commands::field_edit_commit_on(world, entity, AnimationSet::type_path(), "", &value)
    {
        warn!("animation.library.add_state: the set refused the appended state");
    }
}

/// A state name the set does not already use, so adding the same clip twice
/// does not shadow the first one.
fn unused_state_name(set: &AnimationSet, clip: &str) -> String {
    if !set.states.iter().any(|def| def.name == clip) {
        return clip.to_string();
    }
    (2..)
        .map(|n| format!("{clip} {n}"))
        .find(|name| !set.states.iter().any(|def| def.name == *name))
        .unwrap_or_else(|| clip.to_string())
}

/// Builds the Animation panel: a tab strip over a tab body. Both containers
/// start empty; the systems in this module fill them.
pub fn animation_panel_content() -> impl Bundle {
    (
        Node {
            width: percent(100),
            height: percent(100),
            flex_direction: FlexDirection::Column,
            ..default()
        },
        BackgroundColor(tokens::PANEL_BG),
        children![
            (
                AnimationPanelTabStrip,
                Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: px(tokens::SPACING_XS),
                    padding: UiRect::all(px(tokens::SPACING_SM)),
                    flex_shrink: 0.0,
                    ..default()
                },
            ),
            (
                AnimationPanelBody,
                Node {
                    width: percent(100),
                    flex_grow: 1.0,
                    min_height: px(0),
                    flex_direction: FlexDirection::Column,
                    ..default()
                },
            ),
        ],
    )
}

/// Redraw the tab strip when the tab it highlights changed.
fn update_animation_panel_tabs(
    mut commands: Commands,
    tab: Res<AnimationPanelTab>,
    strips: Query<(Entity, Option<&Children>), With<AnimationPanelTabStrip>>,
    mut last: Local<Option<AnimationPanelTab>>,
) {
    let unchanged = *last == Some(*tab);
    for (strip, children) in &strips {
        if unchanged && children.is_some_and(|kids| !kids.is_empty()) {
            continue;
        }
        despawn_children(&mut commands, children);
        let row = spawn_tab_strip(
            &mut commands,
            strip,
            AnimationPanelTabOp::ID,
            "tab",
            TabStripOrientation::Horizontal,
            [
                TabStripItem::new("Library", *tab == AnimationPanelTab::Library, "library"),
                TabStripItem::new("Timeline", *tab == AnimationPanelTab::Timeline, "timeline"),
            ],
        );
        commands.entity(row).insert(AnimationPanelPart);
    }
    *last = Some(*tab);
}

/// Build the body of whichever tab is showing.
///
/// Only the tab changing rebuilds this. The lists inside the Library tab are
/// refilled where they stand, so typing in the filter does not throw away the
/// field being typed into.
fn update_animation_panel_body(
    mut commands: Commands,
    tab: Res<AnimationPanelTab>,
    state: Res<AnimationPanelState>,
    icon_font: Option<Res<IconFont>>,
    bodies: Query<(Entity, Option<&Children>), With<AnimationPanelBody>>,
    mut last: Local<Option<AnimationPanelTab>>,
) {
    let Some(icon_font) = icon_font else {
        return;
    };
    let unchanged = *last == Some(*tab);
    for (body, children) in &bodies {
        if unchanged && children.is_some_and(|kids| !kids.is_empty()) {
            continue;
        }
        despawn_children(&mut commands, children);
        match *tab {
            AnimationPanelTab::Timeline => {
                commands.spawn((
                    AnimationPanelPart,
                    jackdaw_animation::timeline_panel(),
                    ChildOf(body),
                ));
            }
            AnimationPanelTab::Library => {
                spawn_library_tab(&mut commands, body, &state, &icon_font);
            }
        }
    }
    *last = Some(*tab);
}

fn despawn_children(commands: &mut Commands, children: Option<&Children>) {
    for child in children.into_iter().flatten() {
        commands.entity(*child).despawn();
    }
}

/// The Library tab's frame: a file column beside a filtered clip list, over a
/// transport. The two lists are filled by their own systems.
fn spawn_library_tab(
    commands: &mut Commands,
    body: Entity,
    state: &AnimationPanelState,
    icon_font: &IconFont,
) {
    let content = commands
        .spawn((
            AnimationPanelPart,
            Node {
                flex_direction: FlexDirection::Row,
                flex_grow: 1.0,
                min_height: px(0),
                width: percent(100),
                ..default()
            },
            ChildOf(body),
        ))
        .id();

    commands.spawn((
        LibraryFileList,
        Node {
            width: px(FILE_COLUMN_WIDTH),
            flex_shrink: 0.0,
            flex_direction: FlexDirection::Column,
            overflow: Overflow::scroll_y(),
            padding: UiRect::all(px(tokens::SPACING_SM)),
            row_gap: px(tokens::SPACING_XS),
            border: UiRect::right(px(1.0)),
            ..default()
        },
        BorderColor::all(tokens::BORDER_SUBTLE),
        ScrollPosition::default(),
        ChildOf(content),
    ));

    let clips = commands
        .spawn((
            Node {
                flex_grow: 1.0,
                min_width: px(0),
                flex_direction: FlexDirection::Column,
                ..default()
            },
            ChildOf(content),
        ))
        .id();
    let toolbar = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(tokens::SPACING_SM),
                padding: UiRect::all(px(tokens::SPACING_SM)),
                flex_shrink: 0.0,
                ..default()
            },
            ChildOf(clips),
        ))
        .id();
    commands.spawn((
        AnimationLibraryFilter,
        text_edit(
            TextEditProps::default()
                .with_placeholder("Filter clips...")
                .with_default_value(state.filter.clone())
                .allow_empty(),
        ),
        ChildOf(toolbar),
    ));
    commands.spawn((
        button(
            ButtonProps::new("Add as state")
                .with_variant(ButtonVariant::Default)
                .with_left_icon(Icon::Plus),
        ),
        ButtonOperatorCall::new(AnimationLibraryAddStateOp::ID),
        ChildOf(toolbar),
    ));
    commands.spawn((
        LibraryClipList,
        Node {
            flex_grow: 1.0,
            min_height: px(0),
            flex_direction: FlexDirection::Column,
            overflow: Overflow::scroll_y(),
            padding: UiRect::horizontal(px(tokens::SPACING_SM)),
            row_gap: px(tokens::SPACING_XS),
            ..default()
        },
        ScrollPosition::default(),
        ChildOf(clips),
    ));

    spawn_transport(commands, body, icon_font);
}

fn spawn_transport(commands: &mut Commands, parent: Entity, icon_font: &IconFont) {
    let footer = commands
        .spawn((
            AnimationPanelPart,
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(tokens::SPACING_SM),
                width: percent(100),
                height: px(32.0),
                flex_shrink: 0.0,
                padding: UiRect::axes(px(tokens::SPACING_SM), px(tokens::SPACING_XS)),
                border: UiRect::top(px(1.0)),
                ..default()
            },
            BackgroundColor(tokens::PANEL_HEADER_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            ChildOf(parent),
        ))
        .id();

    for (icon, op) in [
        (Icon::Play, AnimationPreviewOp::ID),
        (Icon::Pause, AnimationPreviewPauseOp::ID),
        (Icon::Square, AnimationPreviewStopOp::ID),
    ] {
        commands.spawn((
            icon_button(IconButtonProps::new(icon), &icon_font.0),
            ButtonOperatorCall::new(op),
            ChildOf(footer),
        ));
    }

    commands.spawn((
        AnimationPreviewProgress,
        Node {
            flex_grow: 1.0,
            min_width: px(0),
            ..default()
        },
        children![progress_bar(0.0)],
        ChildOf(footer),
    ));
    commands.spawn((
        PreviewTargetLabel,
        Text::new(NOTHING_PREVIEWING),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
            ..default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(footer),
    ));
}

/// What the file column was last drawn for.
#[derive(PartialEq)]
struct FileListSignature {
    selection: Option<Entity>,
    files: usize,
    chosen: Option<String>,
}

/// Refill the file column: the selection's own files above the project's.
fn update_library_files(
    mut commands: Commands,
    state: Res<AnimationPanelState>,
    library: Res<AnimationLibrary>,
    selection: Res<Selection>,
    sets: Query<&AnimationSet>,
    gltf_sources: Query<&jackdaw_scene_types::GltfSource>,
    lists: Query<(Entity, Option<&Children>), With<LibraryFileList>>,
    mut last: Local<Option<FileListSignature>>,
) {
    if lists.is_empty() {
        return;
    }
    let primary = selection.primary();
    let signature = FileListSignature {
        selection: primary,
        files: library.len(),
        chosen: state.file.clone(),
    };
    if last.as_ref() == Some(&signature) {
        return;
    }
    *last = Some(signature);

    let selected: Vec<String> = primary
        .into_iter()
        .flat_map(|entity| {
            sets.get(entity)
                .map(|set| set.sources.clone())
                .unwrap_or_default()
                .into_iter()
                .chain(gltf_sources.get(entity).map(|source| source.path.clone()))
        })
        .filter(|path| library.file(path).is_some())
        .collect();

    for (list, children) in &lists {
        despawn_children(&mut commands, children);
        if !selected.is_empty() {
            spawn_group_heading(&mut commands, list, "Selected");
            for path in &selected {
                spawn_file_row(&mut commands, list, &state, &library, path);
            }
        }
        spawn_group_heading(&mut commands, list, "Project");
        if library.is_empty() {
            spawn_hint(
                &mut commands,
                list,
                "No glTF file in this project holds a clip.",
            );
            continue;
        }
        for file in library.files() {
            spawn_file_row(&mut commands, list, &state, &library, &file.path);
        }
    }
}

/// What the clip list was last drawn for.
#[derive(PartialEq)]
struct ClipListSignature {
    file: Option<String>,
    clips: usize,
    filter: String,
}

/// Refill the clip list for the chosen file, through the filter.
fn update_library_clips(
    mut commands: Commands,
    state: Res<AnimationPanelState>,
    library: Res<AnimationLibrary>,
    lists: Query<(Entity, Option<&Children>), With<LibraryClipList>>,
    mut last: Local<Option<ClipListSignature>>,
) {
    if lists.is_empty() {
        return;
    }
    let file = state.file.as_deref().and_then(|path| library.file(path));
    let signature = ClipListSignature {
        file: state.file.clone(),
        clips: file.map_or(0, |file| file.clips.len()),
        filter: state.filter.clone(),
    };
    if last.as_ref() == Some(&signature) {
        return;
    }
    *last = Some(signature);

    let filter = state.filter.to_ascii_lowercase();
    for (list, children) in &lists {
        despawn_children(&mut commands, children);
        let Some(file) = file else {
            spawn_hint(
                &mut commands,
                list,
                "Choose a file to see the clips it holds.",
            );
            continue;
        };
        let mut shown = 0;
        for clip in &file.clips {
            if !filter.is_empty() && !clip.name.to_ascii_lowercase().contains(&filter) {
                continue;
            }
            shown += 1;
            let mut props = ButtonProps::new(clip.name.clone())
                .with_variant(ButtonVariant::Ghost)
                .align_left()
                .with_subtitle(format!("{:.2}s", clip.duration_secs));
            if clip.looped_hint {
                props = props.with_right_icon(Icon::Repeat);
            }
            commands.spawn((
                AnimationPanelPart,
                button(props),
                ButtonOperatorCall::new(AnimationPreviewOp::ID)
                    .with_param("clip", format!("{}#{}", file.path, clip.name)),
                ChildOf(list),
            ));
        }
        if shown == 0 {
            spawn_hint(
                &mut commands,
                list,
                "No clip in this file matches the filter.",
            );
        }
    }
}

fn spawn_group_heading(commands: &mut Commands, parent: Entity, label: &str) {
    commands.spawn((
        AnimationPanelPart,
        Text::new(label.to_uppercase()),
        TextFont {
            font_size: tokens::TEXT_SIZE_XS,
            ..default()
        },
        TextColor(tokens::TEXT_MUTED_COLOR.into()),
        Node {
            margin: UiRect::top(px(tokens::SPACING_XS)),
            ..default()
        },
        ChildOf(parent),
    ));
}

fn spawn_hint(commands: &mut Commands, parent: Entity, text: &str) {
    commands.spawn((
        AnimationPanelPart,
        Text::new(text.to_string()),
        TextFont {
            font_size: tokens::TEXT_SIZE_SM,
            ..default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(parent),
    ));
}

fn spawn_file_row(
    commands: &mut Commands,
    parent: Entity,
    state: &AnimationPanelState,
    library: &AnimationLibrary,
    path: &str,
) {
    let count = library.file(path).map_or(0, |file| file.clips.len());
    let clips = if count == 1 {
        "1 clip".to_string()
    } else {
        format!("{count} clips")
    };
    let active = state.file.as_deref() == Some(path);
    commands.spawn((
        AnimationPanelPart,
        button(
            ButtonProps::new(path.to_string())
                .with_variant(if active {
                    ButtonVariant::Active
                } else {
                    ButtonVariant::Ghost
                })
                .with_size(ButtonSize::MD)
                .align_left()
                .with_subtitle(clips),
        ),
        ButtonOperatorCall::new(AnimationLibrarySelectOp::ID).with_param("file", path.to_string()),
        ChildOf(parent),
    ));
}

/// Keep the transport's bar in step with the clip without redrawing the tab.
fn update_preview_progress(
    preview: Res<AnimationPreview>,
    bars: Query<Entity, With<AnimationPreviewProgress>>,
    children: Query<&Children>,
    mut fills: Query<&mut Node, With<ProgressBarFill>>,
) {
    for wrapper in &bars {
        for bar in children.get(wrapper).into_iter().flatten() {
            set_progress_fill(*bar, preview.progress(), &children, &mut fills);
        }
    }
}

/// Say what the transport is playing on.
fn update_preview_target_label(
    preview: Res<AnimationPreview>,
    names: Query<&Name>,
    mut labels: Query<&mut Text, With<PreviewTargetLabel>>,
) {
    let wanted = preview
        .target()
        .and_then(|entity| names.get(entity).ok())
        .map_or_else(
            || NOTHING_PREVIEWING.to_string(),
            |name| name.as_str().to_string(),
        );
    for mut label in &mut labels {
        if label.0 != wanted {
            label.0 = wanted.clone();
        }
    }
}

/// Drop anything this panel spawned that ended up with no parent.
/// See [`AnimationPanelPart`].
fn drop_orphaned_panel_parts(
    orphans: Query<Entity, (With<AnimationPanelPart>, Without<ChildOf>)>,
    mut commands: Commands,
) {
    for orphan in &orphans {
        commands.entity(orphan).despawn();
    }
}

/// Read the filter field into the state the clip list is drawn from.
fn update_library_filter(
    mut state: ResMut<AnimationPanelState>,
    filters: Query<&TextEditValue, (With<AnimationLibraryFilter>, Changed<TextEditValue>)>,
) {
    for filter in &filters {
        if state.filter != filter.0 {
            state.filter = filter.0.clone();
        }
    }
}

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<AnimationPanelTab>()
        .init_resource::<AnimationPanelState>()
        .add_systems(
            Update,
            (
                update_library_filter,
                update_animation_panel_tabs,
                update_animation_panel_body,
                update_library_files,
                update_library_clips,
                update_preview_progress,
                update_preview_target_label,
                drop_orphaned_panel_parts,
            )
                .chain()
                .run_if(in_state(crate::AppState::Editor)),
        );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_state_for_the_same_clip_takes_another_name() {
        let mut set = AnimationSet::default();
        assert_eq!(unused_state_name(&set, "Idle_Loop"), "Idle_Loop");
        set.states.push(AnimationStateDef {
            name: "Idle_Loop".into(),
            ..default()
        });
        assert_eq!(unused_state_name(&set, "Idle_Loop"), "Idle_Loop 2");
    }
}
