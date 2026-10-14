//! The Projects page: recent projects with the editor each opens with.

use bevy::feathers::controls::{
    ButtonVariant, FeathersMenu, FeathersMenuButton, FeathersMenuItem, FeathersMenuPopup,
};
use bevy::feathers::theme::ThemedText;
use bevy::input_focus::tab_navigation::TabGroup;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::text::{FontSize, FontSourceTemplate};
use bevy::ui_widgets::Button;
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_feathers::icons::{Icon, font_paths};
use jackdaw_feathers::tokens;
use jackdaw_releases::Channel;

use super::super::model::{ProjectEntry, ProjectFilter, filter_projects};
use super::super::ui::{self, Tone};
use super::super::{Launcher, LauncherField, ProjectListView};
use super::TextSlot;
use crate::preflight::CheckStatus;
use crate::project_select::PreflightState;

#[derive(Component, Default, Clone)]
pub struct ProjectListRegion;

#[derive(Component, Default, Clone)]
pub struct EnvironmentRegion;

/// A project row, by its folder, for tests and synthetic clicks.
#[derive(Component, Default, Clone, Debug)]
pub struct ProjectRow(pub std::path::PathBuf);

const OPENED_WIDTH: f32 = 110.0;
const EDITOR_WIDTH: f32 = 220.0;

fn call(id: &'static str, path: &std::path::Path) -> ButtonOperatorCall {
    ButtonOperatorCall::new(id).with_param("path", path.to_string_lossy().into_owned())
}

pub fn page() -> impl Scene {
    bsn! {
        Node {
            flex_grow: 1.0,
            min_height: px(0),
            flex_direction: FlexDirection::Column,
            row_gap: px(12),
            padding: UiRect::axes(px(20), px(16)),
        }
        Children [
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(8),
            }
            Children [
                Node { flex_grow: 1.0 }
                Text("Projects")
                TextFont { font_size: tokens::TEXT_SIZE_LG }
                TextColor(tokens::TEXT_PRIMARY)
                --
                @{ui::dim("Filter")}
                --
                Node { width: px(240) }
                TextSlot {
                    field: LauncherField::ProjectQuery,
                    placeholder: "Filter projects",
                }
                --
                @{ui::button("Add", ButtonOperatorCall::new("launcher.add_project"), ButtonVariant::Normal)}
                --
                @{ui::button("New project", ButtonOperatorCall::new("launcher.new_project"), ButtonVariant::Primary)}
            ]
            --
            ProjectListRegion
            Node {
                flex_grow: 1.0,
                min_height: px(0),
                flex_direction: FlexDirection::Column,
                row_gap: px(8),
            }
            --
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(8),
                flex_wrap: FlexWrap::Wrap,
                row_gap: px(6),
            }
            Children [
                @{ui::dim("Start")}
                --
                @{ui::button("New game", ButtonOperatorCall::new("launcher.new_project").with_param("template", "3d"), ButtonVariant::Normal)}
                --
                @{ui::button("New extension", ButtonOperatorCall::new("launcher.new_project").with_param("template", "extension"), ButtonVariant::Normal)}
                --
                @{ui::button("Import Bevy project", ButtonOperatorCall::new("launcher.import_project"), ButtonVariant::Normal)}
                --
                @{ui::button("Open folder...", ButtonOperatorCall::new("launcher.open_folder"), ButtonVariant::Normal)}
            ]
            --
            EnvironmentRegion
            Node { flex_direction: FlexDirection::Column }
            --
            crate::sdk_setup::SdkStatusRow
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(4),
            }
        ]
    }
}

/// The filter buttons, the column headings and the rows.
pub fn list(launcher: &Launcher, view: &ProjectListView) -> impl Scene {
    let filters: Vec<_> = ProjectFilter::ALL
        .into_iter()
        .map(|filter| {
            ui::filter_button(
                filter.label(),
                filter == view.filter,
                ButtonOperatorCall::new("launcher.project_filter")
                    .with_param("filter", filter.id()),
            )
        })
        .collect();
    let shown = filter_projects(&launcher.projects, view.filter, &view.query);
    let rows: ui::Scenes = if launcher.projects.is_empty() {
        vec![ui::boxed(empty_state(
            "No projects yet. Start a new game or extension, or add a project you already have.",
        ))]
    } else if shown.is_empty() {
        vec![ui::boxed(empty_state("No project matches the filter."))]
    } else {
        shown
            .into_iter()
            .map(|entry| ui::boxed(project_row(entry, launcher)))
            .collect()
    };
    bsn! {
        Node {
            flex_grow: 1.0,
            min_height: px(0),
            flex_direction: FlexDirection::Column,
            row_gap: px(8),
        }
        Children [
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(6),
            }
            Children [
                @{ui::dim("Show")}
                --
                {filters}
                --
                @{ui::spacer()}
                --
                @{ui::dim("Sorted by last opened")}
            ]
            --
            Node {
                flex_direction: FlexDirection::Row,
                column_gap: px(10),
                padding: UiRect::horizontal(px(10)),
            }
            Children [
                Node { width: px(28) }
                --
                Node { flex_grow: 1.0 } Children [ @{ui::dim("Name")} ]
                --
                Node { width: px(OPENED_WIDTH) } Children [ @{ui::dim("Opened")} ]
                --
                Node { width: px(EDITOR_WIDTH) } Children [ @{ui::dim("Editor")} ]
                --
                Node { width: px(28) }
            ]
            --
            Node {
                flex_grow: 1.0,
                min_height: px(0),
                flex_direction: FlexDirection::Column,
                row_gap: px(2),
                overflow: Overflow::scroll_y(),
            }
            ScrollPosition::default()
            bevy::ui_widgets::ScrollArea
            Hovered::default()
            Children [ {rows} ]
        ]
    }
}

fn empty_state(message: &'static str) -> impl Scene {
    bsn! {
        Node {
            padding: UiRect::all(px(16)),
            justify_content: JustifyContent::Center,
        }
        Children [ @{ui::secondary(message)} ]
    }
}

fn opened_label(entry: &ProjectEntry) -> String {
    entry
        .last_opened
        .as_deref()
        .and_then(crate::timestamps::relative_to_now)
        .unwrap_or_default()
}

fn project_row(entry: &ProjectEntry, launcher: &Launcher) -> impl Scene {
    let path = entry.path.clone();
    let name = entry.name.clone();
    let opened = opened_label(entry);
    let folder_color = if entry.folder_missing || entry.needs_editor() {
        tokens::DOC_TAB_INACTIVE_LABEL
    } else {
        tokens::TEXT_ACCENT
    };
    let name_color = if entry.folder_missing {
        tokens::TEXT_SECONDARY
    } else {
        tokens::TEXT_PRIMARY
    };
    let details = row_details(entry);
    let path_line = if entry.folder_missing {
        format!("{} - folder missing", display_path(&entry.path))
    } else {
        display_path(&entry.path)
    };
    let chips_line: Box<dyn SceneList> = if details.is_empty() {
        Box::new(bsn_list! {})
    } else {
        ui::boxed(bsn! {
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                flex_wrap: FlexWrap::Wrap,
                column_gap: px(6),
                row_gap: px(4),
                min_width: px(0),
            }
            Pickable::IGNORE
            Children [ {details} ]
        })
    };
    let action: Box<dyn SceneList> = if entry.folder_missing {
        ui::boxed(ui::button(
            "Remove from list",
            call("launcher.remove_project", &path),
            ButtonVariant::Normal,
        ))
    } else {
        ui::boxed(ui::picker_button(
            entry.editor_chip(&launcher.running),
            call("launcher.open_with", &path),
        ))
    };
    let menu: Box<dyn SceneList> = if entry.folder_missing {
        ui::boxed(bsn! { Node { width: px(28) } })
    } else {
        ui::boxed(row_menu(entry))
    };
    let open = call("launcher.open_project", &path);
    let folder_glyph = String::from(Icon::Folder.unicode());
    bsn! {
        ProjectRow({path})
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(10),
            padding: UiRect::axes(px(10), px(8)),
            border_radius: BorderRadius::all(px(4)),
            flex_shrink: 0.0,
        }
        BackgroundColor(Color::NONE)
        Button
        Hovered::default()
        ~{open}
        on(|over: On<PointerOver>, mut backgrounds: Query<&mut BackgroundColor>| {
            if let Ok(mut background) = backgrounds.get_mut(over.event_target()) {
                background.0 = tokens::ELEVATED_BG;
            }
        })
        on(|out: On<PointerOut>, mut backgrounds: Query<&mut BackgroundColor>| {
            if let Ok(mut background) = backgrounds.get_mut(out.event_target()) {
                background.0 = Color::NONE;
            }
        })
        Children [
            Node { width: px(28), justify_content: JustifyContent::Center }
            Text(folder_glyph)
            TextFont {
                font: FontSourceTemplate::Handle(font_paths::LUCIDE),
                font_size: FontSize::Px(18.0),
            }
            TextColor(folder_color)
            Pickable::IGNORE
            --
            Node {
                flex_grow: 1.0,
                min_width: px(0),
                flex_direction: FlexDirection::Column,
                row_gap: px(4),
            }
            Pickable::IGNORE
            Children [
                @{ui::text(name, tokens::TEXT_SIZE, name_color)}
                --
                @{ui::path_text(path_line)}
                --
                {chips_line}
            ]
            --
            Node { width: px(OPENED_WIDTH) }
            Pickable::IGNORE
            Children [ @{ui::dim(opened)} ]
            --
            Node { width: px(EDITOR_WIDTH), flex_direction: FlexDirection::Column }
            Children [ {action} ]
            --
            {menu}
        ]
    }
}

/// The chips under a project's path.
fn row_details(entry: &ProjectEntry) -> ui::Scenes {
    let mut details: ui::Scenes = Vec::new();
    if entry.folder_missing {
        return details;
    }
    if entry.channel == Channel::BleedingEdge {
        details.push(ui::boxed(ui::chip("Bleeding Edge", Tone::Caution)));
        if let Some(rev) = &entry.bevy_rev {
            details.push(ui::boxed(ui::path_text(format!(
                "bevy@{}",
                rev.get(..7).unwrap_or(rev)
            ))));
        }
    }
    if entry.is_extension {
        details.push(ui::boxed(ui::chip("Extension", Tone::Neutral)));
    }
    for module in &entry.modules {
        details.push(ui::boxed(ui::chip(module.clone(), Tone::Neutral)));
    }
    if entry.extensions > 0 {
        let noun = if entry.extensions == 1 {
            "extension"
        } else {
            "extensions"
        };
        details.push(ui::boxed(ui::chip(
            format!("{} {noun}", entry.extensions),
            Tone::Neutral,
        )));
    }
    if let Some(missing) = entry.missing_label() {
        details.push(ui::boxed(ui::chip(missing, Tone::Warning)));
    }
    details
}

/// A path with the home directory written as `~`.
pub fn display_path(path: &std::path::Path) -> String {
    let text = path.to_string_lossy().into_owned();
    match dirs::home_dir() {
        Some(home) if path.starts_with(&home) => {
            let rest = path.strip_prefix(&home).unwrap_or(path);
            format!("~/{}", rest.to_string_lossy())
        }
        _ => text,
    }
}

fn menu_item(caption: &'static str, call: ButtonOperatorCall) -> impl Scene {
    bsn! {
        @FeathersMenuItem {
            @caption: bsn! { Text(caption) ThemedText },
        }
        ~{call}
    }
}

fn row_menu(entry: &ProjectEntry) -> impl Scene {
    let path = entry.path.clone();
    bsn! {
        @FeathersMenu
        Node { width: px(28) }
        Children [
            @FeathersMenuButton {
                @caption: bsn! { @{ui::button_glyph(Icon::Ellipsis)} },
                @arrow: false,
            }
            --
            @FeathersMenuPopup
            ~{TabGroup::modal()}
            ~{ui::end_aligned_popover()}
            Node { min_width: px(200) }
            Children [
                @{menu_item("Open", call("launcher.open_project", &path))}
                --
                @{menu_item("Open with...", call("launcher.open_with", &path))}
                --
                @{menu_item("Extensions and plugins", call("launcher.extensions", &path))}
                --
                @{menu_item("Show in files", call("launcher.reveal_project", &path))}
                --
                @{menu_item("Remove from list", call("launcher.remove_project", &path))}
            ]
        ]
    }
}

/// The environment checks as chips, with a pointer to Doctor.
pub fn environment(launcher: &Launcher, preflight: &PreflightState) -> impl Scene {
    let mut chips: ui::Scenes = Vec::new();
    if preflight.results.is_empty() {
        chips.push(ui::boxed(ui::chip("Checking", Tone::Neutral)));
    }
    for result in &preflight.results {
        let tone = match result.status {
            CheckStatus::Ok => Tone::Success,
            CheckStatus::Warn => Tone::Caution,
            CheckStatus::Fail => Tone::Warning,
        };
        chips.push(ui::boxed(ui::chip(result.label.clone(), tone)));
    }
    if let Some(update) = launcher.update_available() {
        chips.push(ui::boxed(ui::chip(
            format!("jackdaw {update} available"),
            Tone::Accent,
        )));
    }
    bsn! {
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            flex_wrap: FlexWrap::Wrap,
            column_gap: px(8),
            row_gap: px(6),
            padding: UiRect::axes(px(10), px(8)),
            border_radius: BorderRadius::all(px(4)),
        }
        BackgroundColor(tokens::WINDOW_BG)
        Children [
            @{ui::dim("Environment")}
            --
            {chips}
            --
            @{ui::spacer()}
            --
            @{ui::button("Doctor", ButtonOperatorCall::new("launcher.doctor"), ButtonVariant::Plain)}
        ]
    }
}
