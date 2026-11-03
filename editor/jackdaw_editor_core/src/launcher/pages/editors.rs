//! The Editors page: installed editors grouped by Bevy line, with updates,
//! the editors projects need, and nightlies kept for rollback.

use bevy::feathers::controls::{
    ButtonVariant, FeathersMenu, FeathersMenuButton, FeathersMenuDivider, FeathersMenuItem,
    FeathersMenuPopup,
};
use bevy::feathers::theme::ThemedText;
use bevy::input_focus::tab_navigation::TabGroup;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_feathers::tokens;
use jackdaw_releases::Channel;

use super::super::model::{
    EditorFilter, EditorLine, Installable, InstalledEditor, LineKind, format_bytes, version_label,
};
use super::super::ui::{self, Tone};
use super::super::{EditorListView, Launcher};
use super::projects::display_path;

#[derive(Component, Default, Clone)]
pub struct EditorListRegion;

fn call(id: &'static str, version: &str) -> ButtonOperatorCall {
    ButtonOperatorCall::new(id).with_param("version", version.to_string())
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
                Text("Editors")
                TextFont { font_size: tokens::TEXT_SIZE_LG }
                TextColor(tokens::TEXT_PRIMARY)
                --
                @{ui::button("Check for updates", ButtonOperatorCall::new("launcher.refresh_index"), ButtonVariant::Plain)}
                --
                @{ui::button("Locate...", ButtonOperatorCall::new("launcher.locate_editor"), ButtonVariant::Normal)}
                --
                @{ui::button("Install editor", ButtonOperatorCall::new("launcher.install_editor"), ButtonVariant::Primary)}
            ]
            --
            EditorListRegion
            Node {
                flex_grow: 1.0,
                min_height: px(0),
                flex_direction: FlexDirection::Column,
            }
        ]
    }
}

/// The filter buttons and one section per Bevy line.
pub fn list(launcher: &Launcher, view: &EditorListView) -> impl Scene {
    let filters: Vec<_> = EditorFilter::ALL
        .into_iter()
        .map(|filter| {
            ui::filter_button(
                filter.label(),
                filter == view.filter,
                ButtonOperatorCall::new("launcher.editor_filter").with_param("filter", filter.id()),
            )
        })
        .collect();
    let mut sections: ui::Scenes = launcher
        .lines
        .iter()
        .filter(|line| view.filter.keeps(line))
        .map(|line| ui::boxed(line_section(line, launcher)))
        .collect();
    if sections.is_empty() {
        sections.push(ui::boxed(ui::secondary(
            "No editors to show. Install one, or locate one you installed another way.",
        )));
    }
    let index_note = match &launcher.index {
        super::super::model::CachedIndex::Verified(_) => None,
        super::super::model::CachedIndex::None => Some(
            "The release index has not been fetched yet, so only installed editors are listed.",
        ),
        super::super::model::CachedIndex::Unusable(_) => {
            Some("The cached release index does not verify, so only installed editors are listed.")
        }
    };
    let index_note: ui::Scenes = index_note
        .map(|note| vec![ui::boxed(ui::note(note, tokens::TEXT_SECONDARY))])
        .unwrap_or_default();
    bsn! {
        Node {
            flex_grow: 1.0,
            min_height: px(0),
            flex_direction: FlexDirection::Column,
            row_gap: px(12),
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
            ]
            --
            {index_note}
            --
            Node {
                flex_grow: 1.0,
                min_height: px(0),
                flex_direction: FlexDirection::Column,
                row_gap: px(12),
                overflow: Overflow::scroll_y(),
            }
            ScrollPosition::default()
            bevy::ui_widgets::ScrollArea
            Hovered::default()
            Children [ {sections} ]
        ]
    }
}

fn line_title(line: &EditorLine) -> String {
    if line.is_main() {
        "Bevy main".into()
    } else {
        format!("Bevy {}", line.bevy)
    }
}

fn line_section(line: &EditorLine, launcher: &Launcher) -> impl Scene {
    let mut header: ui::Scenes = vec![ui::boxed(ui::primary(line_title(line)))];
    if line.kind == LineKind::Main {
        header.push(ui::boxed(ui::chip("Bleeding Edge", Tone::Caution)));
    }
    if !line.note().is_empty() {
        header.push(ui::boxed(ui::dim(line.note())));
    }
    let mut body: ui::Scenes = Vec::new();
    if !launcher.running_registered && line.bevy == launcher.running.bevy {
        body.push(ui::boxed(running_card(launcher)));
    }
    for editor in &line.installs {
        body.push(ui::boxed(install_card(editor, launcher)));
    }
    if let Some(installable) = &line.installable {
        body.push(ui::boxed(installable_card(installable)));
    }
    if !line.rollback.is_empty() {
        body.push(ui::boxed(rollback_row(line)));
    }
    ui::section(header, body)
}

fn card(left: ui::Scenes, right: ui::Scenes) -> impl Scene {
    bsn! {
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(10),
            padding: UiRect::axes(px(10), px(8)),
            border_radius: BorderRadius::all(px(4)),
        }
        BackgroundColor(tokens::ELEVATED_BG)
        Children [
            Node {
                flex_grow: 1.0,
                min_width: px(0),
                flex_direction: FlexDirection::Column,
                row_gap: px(6),
            }
            Children [ {left} ]
            --
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(8),
                flex_shrink: 0.0,
            }
            Children [ {right} ]
        ]
    }
}

fn chips_row(chips: ui::Scenes) -> impl Scene {
    bsn! {
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            flex_wrap: FlexWrap::Wrap,
            column_gap: px(6),
            row_gap: px(4),
        }
        Children [ {chips} ]
    }
}

fn install_card(editor: &InstalledEditor, launcher: &Launcher) -> impl Scene {
    let install = &editor.install;
    let mut title: ui::Scenes = vec![ui::boxed(ui::primary(match install.channel {
        Channel::Stable => format!("jackdaw {}", install.version),
        Channel::BleedingEdge => version_label(&install.version),
    }))];
    match install.channel {
        Channel::Stable => title.push(ui::boxed(ui::chip("Stable", Tone::Success))),
        Channel::BleedingEdge if install.known_good => {
            title.push(ui::boxed(ui::chip("Known good", Tone::Success)));
        }
        Channel::BleedingEdge if editor.newest => {
            title.push(ui::boxed(ui::chip("Newest", Tone::Neutral)));
        }
        Channel::BleedingEdge => {}
    }
    if editor.is_default {
        title.push(ui::boxed(ui::chip("Default", Tone::Neutral)));
    }
    if editor.is_running {
        title.push(ui::boxed(ui::chip("This editor", Tone::Accent)));
    }
    if !install.managed {
        title.push(ui::boxed(ui::chip("Located", Tone::Neutral)));
    }
    if let Some(rev) = &install.bevy_rev {
        title.push(ui::boxed(ui::path_text(format!(
            "bevy@{}",
            rev.get(..7).unwrap_or(rev)
        ))));
    }
    title.push(ui::boxed(ui::chip(
        short_target(launcher.host_target),
        Tone::Neutral,
    )));
    let projects = editor.projects.len();
    title.push(ui::boxed(ui::chip(
        if projects == 1 {
            "1 project".to_string()
        } else {
            format!("{projects} projects")
        },
        Tone::Neutral,
    )));
    let mut left: ui::Scenes = vec![
        ui::boxed(chips_row(title)),
        ui::boxed(ui::path_text(display_path(&install.path))),
    ];
    if !install.modules.is_empty() {
        let mut modules: ui::Scenes = vec![ui::boxed(ui::dim("Modules"))];
        for module in &install.modules {
            let name = jackdaw_project_build::modules::ModuleCatalog::builtin()
                .get(module)
                .map_or_else(|| module.clone(), |info| info.name.clone());
            modules.push(ui::boxed(ui::chip(name, Tone::Neutral)));
        }
        left.push(ui::boxed(chips_row(modules)));
    }
    let mut right: ui::Scenes = Vec::new();
    if let Some(update) = &editor.update {
        right.push(ui::boxed(ui::text(
            format!("{update} available"),
            tokens::TEXT_SIZE_SM,
            tokens::CHIP_SUCCESS_TEXT,
        )));
        right.push(ui::boxed(ui::button(
            "Update",
            call("launcher.update_editor", &install.version),
            ButtonVariant::Primary,
        )));
    }
    right.push(ui::boxed(manage_menu(editor)));
    card(left, right)
}

fn short_target(target: &str) -> String {
    let mut parts = target.split('-');
    let arch = parts.next().unwrap_or(target);
    let os = target
        .split('-')
        .find(|part| matches!(*part, "linux" | "windows" | "darwin"))
        .map(|os| if os == "darwin" { "macos" } else { os })
        .unwrap_or("");
    if os.is_empty() {
        arch.to_string()
    } else {
        format!("{arch}-{os}")
    }
}

fn menu_item(caption: String, call: ButtonOperatorCall) -> impl Scene {
    bsn! {
        @FeathersMenuItem {
            @caption: bsn! { Text(caption) ThemedText Node { margin: UiRect::right(px(6)) } },
        }
        ~{call}
    }
}

fn manage_menu(editor: &InstalledEditor) -> impl Scene {
    let version = editor.install.version.clone();
    let projects = editor.projects.len();
    let uninstall = match projects {
        0 => "Uninstall...".to_string(),
        1 => "Uninstall... (1 project uses it)".to_string(),
        n => format!("Uninstall... ({n} projects use it)"),
    };
    let mut items: ui::Scenes = vec![ui::boxed(menu_item(
        "Add or remove modules...".into(),
        call("launcher.editor_modules", &version),
    ))];
    if !editor.is_default {
        items.push(ui::boxed(menu_item(
            "Set as default".into(),
            call("launcher.set_default_editor", &version),
        )));
    }
    items.push(ui::boxed(menu_item(
        "Release notes".into(),
        call("launcher.release_notes", &version),
    )));
    items.push(ui::boxed(menu_item(
        "Show in files".into(),
        call("launcher.reveal_editor", &version),
    )));
    items.push(ui::boxed(menu_item(
        "Verify install".into(),
        call("launcher.verify_editor", &version),
    )));
    if !editor.is_running {
        items.push(ui::boxed(bsn! { @FeathersMenuDivider }));
        items.push(ui::boxed(menu_item(
            uninstall,
            call("launcher.uninstall_editor", &version),
        )));
    }
    bsn! {
        @FeathersMenu
        Children [
            @FeathersMenuButton {
                @caption: bsn! { Text("Manage") ThemedText Node { margin: UiRect::right(px(6)) } },
            }
            --
            @FeathersMenuPopup
            ~{TabGroup::modal()}
            ~{ui::end_aligned_popover()}
            Node { min_width: px(240) }
            Children [ {items} ]
        ]
    }
}

fn running_card(launcher: &Launcher) -> impl Scene {
    let running = &launcher.running;
    let title: ui::Scenes = vec![
        ui::boxed(ui::primary(format!(
            "jackdaw {}",
            version_label(&running.version)
        ))),
        ui::boxed(ui::chip("This editor", Tone::Accent)),
        ui::boxed(ui::chip("Not registered", Tone::Neutral)),
    ];
    let exe = std::env::current_exe()
        .map(|exe| exe.to_string_lossy().into_owned())
        .unwrap_or_default();
    let left: ui::Scenes = vec![
        ui::boxed(chips_row(title)),
        ui::boxed(ui::path_text(display_path(std::path::Path::new(&exe)))),
        ui::boxed(ui::dim(
            "Built or installed outside the launcher. Register it so projects can pin it.",
        )),
    ];
    let right: ui::Scenes = vec![ui::boxed(ui::button(
        "Register",
        ButtonOperatorCall::new("launcher.locate_editor").with_param("path", exe),
        ButtonVariant::Normal,
    ))];
    card(left, right)
}

fn installable_card(installable: &Installable) -> impl Scene {
    let mut title: ui::Scenes = vec![ui::boxed(ui::secondary("Not installed"))];
    for project in &installable.needed_by {
        title.push(ui::boxed(ui::chip(
            format!("{project} needs it"),
            Tone::Warning,
        )));
    }
    let size = installable
        .download
        .map(|bytes| format!(" - {} download", format_bytes(bytes)))
        .unwrap_or_default();
    let left: ui::Scenes = vec![
        ui::boxed(chips_row(title)),
        ui::boxed(ui::dim(format!(
            "jackdaw {}{size}",
            version_label(&installable.version)
        ))),
    ];
    let right: ui::Scenes = vec![ui::boxed(ui::button(
        format!("Install {}", version_label(&installable.version)),
        call("launcher.install_editor", &installable.query),
        ButtonVariant::Normal,
    ))];
    card(left, right)
}

fn rollback_row(line: &EditorLine) -> impl Scene {
    let dates: Vec<String> = line
        .rollback
        .iter()
        .map(|version| version.trim_start_matches("nightly-").to_string())
        .collect();
    let size = line
        .rollback_bytes
        .map(|bytes| format!(" - {}", format_bytes(bytes)))
        .unwrap_or_default();
    let text = format!("Kept for rollback: {}{size}", dates.join(", "));
    bsn! {
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(8),
            padding: UiRect::axes(px(10), px(4)),
        }
        Children [
            @{ui::dim(text)}
            --
            @{ui::spacer()}
            --
            @{ui::button("Clean up", ButtonOperatorCall::new("launcher.clean_nightlies"), ButtonVariant::Plain)}
        ]
    }
}
