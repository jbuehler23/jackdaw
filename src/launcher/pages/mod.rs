//! The launcher's window: the title bar's header buttons, the sidebar, and
//! the page beside it. Each part lives in a region entity that is refilled
//! from the launcher's state when the state it shows changes.

mod dialogs;
mod editors;
pub(crate) mod extensions;
mod new_project;
mod projects;

use bevy::feathers::controls::ButtonVariant;
use bevy::prelude::*;
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_feathers::icons::Icon;
use jackdaw_feathers::text_edit::{TextEditProps, text_edit};
use jackdaw_feathers::tokens;

use super::ui::{self, Tone};
use super::{
    ExtensionsView, IndexStatus, JdJobs, Launcher, LauncherField, LauncherNotice, LauncherPage,
    NewProjectForm, ProjectListView,
};
use crate::project_select::PreflightState;

pub use dialogs::sync_dialog;
pub use new_project::{create_project, poll_new_project};

/// Present when the New project page's text fields should be refilled from
/// the form, after something other than typing changed it.
#[derive(Resource)]
pub struct NewProjectFieldsStale;

#[derive(Component, Default, Clone)]
pub struct HeaderRegion;

#[derive(Component, Default, Clone)]
pub struct NavRegion;

#[derive(Component, Default, Clone)]
pub struct PageRegion;

#[derive(Component, Default, Clone)]
pub struct NoticeRegion;

/// Where a text field goes, and which state it edits.
#[derive(Component, Default, Clone)]
pub struct TextSlot {
    pub field: LauncherField,
    pub placeholder: String,
}

fn refill(commands: &mut Commands, region: Entity, scene: impl Scene) {
    commands.entity(region).despawn_children();
    commands.spawn_scene(scene).insert(ChildOf(region));
}

/// The title bar's content after the logo: the launcher's name and the
/// header buttons.
pub fn title_bar() -> impl Scene {
    bsn! {
        Node {
            flex_grow: 1.0,
            height: percent(100),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(12),
            padding: UiRect::horizontal(px(tokens::SPACING_MD)),
        }
        Pickable::IGNORE
        Children [
            Text("jackdaw launcher")
            TextFont { font_size: tokens::TEXT_SIZE, weight: FontWeight::BOLD }
            TextColor(tokens::TEXT_PRIMARY)
            Pickable::IGNORE
            --
            @{ui::spacer()}
            --
            HeaderRegion
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(4),
            }
            Pickable::IGNORE
        ]
    }
}

/// The launcher's body: the sidebar and the page.
pub fn body() -> impl Scene {
    bsn! {
        Node {
            width: percent(100),
            flex_grow: 1.0,
            min_height: px(0),
            flex_direction: FlexDirection::Row,
            border: UiRect::top(px(1)),
        }
        BorderColor::all(tokens::PANEL_BORDER)
        Children [
            NavRegion
            Node {
                width: px(200),
                flex_shrink: 0.0,
                flex_direction: FlexDirection::Column,
                border: UiRect::right(px(1)),
            }
            BorderColor::all(tokens::PANEL_BORDER)
            --
            Node {
                flex_grow: 1.0,
                min_width: px(0),
                flex_direction: FlexDirection::Column,
            }
            BackgroundColor(tokens::PANEL_BG)
            Children [
                NoticeRegion
                Node { flex_direction: FlexDirection::Column }
                --
                PageRegion
                Node {
                    flex_grow: 1.0,
                    min_height: px(0),
                    flex_direction: FlexDirection::Column,
                }
            ]
        ]
    }
}

fn op(id: &'static str) -> ButtonOperatorCall {
    ButtonOperatorCall::new(id)
}

/// Downloads, Doctor and Settings, with how many installs are running and
/// how many environment checks failed.
pub fn rebuild_header(
    mut commands: Commands,
    jobs: Res<JdJobs>,
    preflight: Res<PreflightState>,
    regions: Query<Entity, With<HeaderRegion>>,
    fresh: Query<(), Added<HeaderRegion>>,
    mut shown: Local<Option<(usize, usize, bool)>>,
) {
    let running = jobs.running().count();
    let issues = preflight
        .results
        .iter()
        .filter(|result| result.status != crate::preflight::CheckStatus::Ok)
        .count();
    let state = (running, issues, preflight.results.is_empty());
    let stale = shown.as_ref() != Some(&state);
    for region in &regions {
        if stale || fresh.contains(region) {
            refill(&mut commands, region, header(running, issues));
        }
    }
    *shown = Some(state);
}

fn header(running: usize, issues: usize) -> impl Scene {
    let downloads = if running > 0 {
        format!("Downloads {running}")
    } else {
        String::new()
    };
    let doctor = if issues > 0 {
        format!("Doctor ({issues})")
    } else {
        "Doctor".to_string()
    };
    let doctor_variant = if issues > 0 {
        ButtonVariant::Primary
    } else {
        ButtonVariant::Plain
    };
    bsn! {
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(4),
        }
        Pickable::IGNORE
        Children [
            @{ui::icon_button(Icon::Download, downloads, op("launcher.downloads"), ButtonVariant::Plain)}
            --
            @{ui::icon_button(Icon::Activity, doctor, op("launcher.doctor"), doctor_variant)}
            --
            @{ui::icon_button(Icon::Settings, "", op("launcher.settings"), ButtonVariant::Plain)}
        ]
    }
}

/// The sidebar: the page buttons and what this editor is.
pub fn rebuild_nav(
    mut commands: Commands,
    page: Res<LauncherPage>,
    launcher: Res<Launcher>,
    index: Res<IndexStatus>,
    regions: Query<Entity, With<NavRegion>>,
    fresh: Query<(), Added<NavRegion>>,
) {
    let stale = page.is_changed() || launcher.is_changed() || index.is_changed();
    for region in &regions {
        if stale || fresh.contains(region) {
            refill(&mut commands, region, nav(*page, &launcher, &index));
        }
    }
}

fn nav_button(caption: &'static str, page: LauncherPage, current: LauncherPage) -> impl Scene {
    let selected =
        page == current || (page == LauncherPage::Projects && current == LauncherPage::NewProject);
    let variant = if selected {
        ButtonVariant::Normal
    } else {
        ButtonVariant::Plain
    };
    let call = ButtonOperatorCall::new("launcher.show_page").with_param("page", page.id());
    bsn! {
        @{ui::button(caption, call, variant)}
        Node {
            width: percent(100),
            height: px(30),
            justify_content: JustifyContent::FlexStart,
            padding: UiRect::horizontal(px(10)),
        }
    }
}

fn nav(page: LauncherPage, launcher: &Launcher, index: &IndexStatus) -> impl Scene {
    let running = &launcher.running;
    let name = format!("jackdaw {}", super::model::version_label(&running.version));
    let line = format!(
        "Bevy {} - {}",
        running.bevy,
        super::model::channel_label(running.channel)
    );
    let origin: ui::Scenes = match crate::project_select::build_origin() {
        (label, _) if label == "Source checkout" => vec![ui::boxed(ui::dim(label))],
        _ => Vec::new(),
    };
    let jd = match index {
        IndexStatus::Unknown => "jd - not checked".to_string(),
        IndexStatus::Checking => "jd - checking".to_string(),
        IndexStatus::Online { jd_version } => {
            format!("jd {} - online", jd_version.as_deref().unwrap_or("?"))
        }
        IndexStatus::Offline { jd_version, .. } => {
            format!("jd {} - offline", jd_version.as_deref().unwrap_or("?"))
        }
    };
    bsn! {
        Node {
            flex_direction: FlexDirection::Column,
            flex_grow: 1.0,
            row_gap: px(4),
            padding: UiRect::all(px(10)),
        }
        Children [
            @{nav_button("Projects", LauncherPage::Projects, page)}
            --
            @{nav_button("Editors", LauncherPage::Editors, page)}
            --
            @{nav_button("Extensions", LauncherPage::Extensions, page)}
            --
            @{ui::spacer()}
            --
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(4),
                padding: UiRect::all(px(10)),
                border_radius: BorderRadius::all(px(4)),
            }
            BackgroundColor(tokens::PANEL_BG)
            Children [
                @{ui::text(name, tokens::TEXT_SIZE_SM, tokens::TEXT_PRIMARY)}
                --
                @{ui::dim(line)}
                --
                @{ui::dim(jd)}
                --
                {origin}
            ]
        ]
    }
}

/// The page the launcher shows. Refilled only when the page changes, so a
/// text field on it keeps its focus while the list under it updates.
pub fn rebuild_page(
    mut commands: Commands,
    page: Res<LauncherPage>,
    regions: Query<Entity, With<PageRegion>>,
    fresh: Query<(), Added<PageRegion>>,
) {
    for region in &regions {
        if !(page.is_changed() || fresh.contains(region)) {
            continue;
        }
        commands.entity(region).despawn_children();
        let mut spawned = match *page {
            LauncherPage::Projects => commands.spawn_scene(projects::page()),
            LauncherPage::Editors => commands.spawn_scene(editors::page()),
            LauncherPage::Extensions => commands.spawn_scene(extensions::page()),
            LauncherPage::NewProject => commands.spawn_scene(new_project::page()),
        };
        spawned.insert(ChildOf(region));
    }
}

/// The project list under the Projects page's header.
pub fn rebuild_project_list(
    mut commands: Commands,
    launcher: Res<Launcher>,
    view: Res<ProjectListView>,
    preflight: Res<PreflightState>,
    notice: Res<LauncherNotice>,
    regions: Query<Entity, With<projects::ProjectListRegion>>,
    fresh: Query<(), Added<projects::ProjectListRegion>>,
    environment: Query<Entity, With<projects::EnvironmentRegion>>,
    fresh_environment: Query<(), Added<projects::EnvironmentRegion>>,
    notices: Query<Entity, With<NoticeRegion>>,
) {
    let stale = launcher.is_changed() || view.is_changed();
    for region in &regions {
        if stale || fresh.contains(region) {
            refill(&mut commands, region, projects::list(&launcher, &view));
        }
    }
    let stale = launcher.is_changed() || preflight.is_changed();
    for region in &environment {
        if stale || fresh_environment.contains(region) {
            refill(
                &mut commands,
                region,
                projects::environment(&launcher, &preflight),
            );
        }
    }
    if notice.is_changed() {
        for region in &notices {
            commands.entity(region).despawn_children();
            if let Some(notice) = &notice.0 {
                commands
                    .spawn_scene(notice_strip(notice))
                    .insert(ChildOf(region));
            }
        }
    }
}

fn notice_strip(notice: &super::Notice) -> impl Scene {
    let tone = if notice.error {
        Tone::Warning
    } else {
        Tone::Neutral
    };
    let text = notice.text.clone();
    bsn! {
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(8),
            padding: UiRect::new(px(20), px(20), px(10), px(0)),
        }
        Children [
            @{ui::chip(if notice.error { "Problem" } else { "Note" }, tone)}
            --
            Node { flex_grow: 1.0, min_width: px(0) }
            Children [ @{ui::note(text, tokens::TEXT_PRIMARY)} ]
            --
            @{ui::button("Dismiss", op("launcher.dismiss_notice"), ButtonVariant::Plain)}
        ]
    }
}

/// The installed editors on the Editors page.
pub fn rebuild_editor_list(
    mut commands: Commands,
    launcher: Res<Launcher>,
    view: Res<super::EditorListView>,
    regions: Query<Entity, With<editors::EditorListRegion>>,
    fresh: Query<(), Added<editors::EditorListRegion>>,
) {
    let stale = launcher.is_changed() || view.is_changed();
    for region in &regions {
        if stale || fresh.contains(region) {
            refill(&mut commands, region, editors::list(&launcher, &view));
        }
    }
}

/// The parts of the New project page that follow the form.
pub fn rebuild_new_project(
    mut commands: Commands,
    launcher: Res<Launcher>,
    form: Res<NewProjectForm>,
    catalog: Option<Res<jackdaw_api_internal::lifecycle::ExtensionCatalog>>,
    regions: Query<(Entity, &new_project::NewProjectRegion)>,
    fresh: Query<(), Added<new_project::NewProjectRegion>>,
    mut shown: Local<Option<NewProjectForm>>,
) {
    let previous = shown.clone();
    for (entity, region) in &regions {
        let stale = launcher.is_changed()
            || previous
                .as_ref()
                .is_none_or(|previous| region.follows_change(previous, &form));
        if stale || fresh.contains(entity) {
            refill(
                &mut commands,
                entity,
                new_project::region(*region, &launcher, &form, catalog.as_deref()),
            );
        }
    }
    if previous.as_ref() != Some(&*form) {
        *shown = Some(form.clone());
    }
}

/// The Extensions page's lists and details.
pub fn rebuild_extensions(world: &mut World) {
    extensions::rebuild(world);
}

/// Put a text field in each new slot, holding the state it edits.
pub fn fill_text_slots(
    mut commands: Commands,
    slots: Query<(Entity, &TextSlot)>,
    fresh: Query<(), Added<TextSlot>>,
    stale: Option<Res<NewProjectFieldsStale>>,
    projects: Res<ProjectListView>,
    extensions: Res<ExtensionsView>,
    form: Res<NewProjectForm>,
) {
    let refill_form = stale.is_some();
    if refill_form {
        commands.remove_resource::<NewProjectFieldsStale>();
    }
    for (entity, slot) in &slots {
        let form_field = matches!(
            slot.field,
            LauncherField::NewProjectName | LauncherField::NewProjectLocation
        );
        if !(fresh.contains(entity) || (refill_form && form_field)) {
            continue;
        }
        let value = match slot.field {
            LauncherField::ProjectQuery => projects.query.clone(),
            LauncherField::ExtensionQuery => extensions.query.clone(),
            LauncherField::NewProjectName => form.name.clone(),
            LauncherField::NewProjectLocation => form.location.to_string_lossy().into_owned(),
        };
        let mut props = TextEditProps::default()
            .with_placeholder(slot.placeholder.clone())
            .allow_empty();
        if !value.is_empty() {
            props = props.with_default_value(value);
        }
        commands.entity(entity).despawn_children();
        commands.spawn((text_edit(props), slot.field, ChildOf(entity)));
    }
}
