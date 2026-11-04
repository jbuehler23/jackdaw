//! Components the open scene holds that this editor cannot load.
//!
//! A component whose type is neither registered nor described by the
//! project's schema stays in the scene document and is saved back unchanged.
//! The inspector shows each one as a read-only card, and the panel header
//! says which crate the scene needs and, when a jackdaw module provides that
//! crate, which module to enable.

use std::collections::BTreeSet;

use bevy::prelude::*;
use jackdaw_feathers::tokens;
use jackdaw_project_build::modules::ModuleCatalog;

use crate::project_types::ProjectTypes;

/// The crate a type path names first.
pub(crate) fn crate_of(type_path: &str) -> &str {
    type_path.split("::").next().unwrap_or(type_path)
}

/// The name of the jackdaw module that provides `crate_name`, if one does.
pub(crate) fn module_for_crate(crate_name: &str) -> Option<&'static str> {
    ModuleCatalog::builtin()
        .providing_crate(crate_name)
        .map(|module| module.name.as_str())
}

/// Whether the editor cannot load `type_path`: no registration answers to it
/// or to the enum it is a variant of, and the project's schema does not
/// describe it either.
pub(crate) fn is_unavailable(
    registry: &bevy::reflect::TypeRegistry,
    project_types: &ProjectTypes,
    type_path: &str,
) -> bool {
    let enclosing = type_path.rsplit_once("::").map(|(base, _)| base);
    let known = |path: &str| {
        registry.get_with_type_path(path).is_some() || project_types.type_schema(path).is_some()
    };
    project_types.component(type_path).is_none()
        && !known(type_path)
        && !enclosing.is_some_and(known)
}

/// The name a card gives a type it cannot load: the type's own name, or for an
/// enum variant the enum's name and the variant's, as `RigidBody::Dynamic`.
pub(crate) fn card_name(type_path: &str) -> &str {
    let segments: Vec<(usize, &str)> = type_path
        .match_indices("::")
        .map(|(at, _)| (at + 2, &type_path[at + 2..]))
        .collect();
    let starts_upper = |segment: &str| segment.chars().next().is_some_and(char::is_uppercase);
    match segments.as_slice() {
        [.., (owner_at, owner), _] if starts_upper(owner) => &type_path[*owner_at..],
        [.., (last_at, _)] => &type_path[*last_at..],
        [] => type_path,
    }
}

/// Where a card says a type comes from.
pub(crate) fn provider_line(type_path: &str) -> String {
    let crate_name = crate_of(type_path);
    match module_for_crate(crate_name) {
        Some(module) => format!("Provided by the {module} module, which is not enabled."),
        None => format!("From {crate_name}, which this editor does not have."),
    }
}

/// The scene-level notice for a set of unavailable type paths: a title naming
/// the crates and a hint saying what would load them.
pub(crate) fn notice(types: &BTreeSet<String>) -> Option<(String, String)> {
    if types.is_empty() {
        return None;
    }
    let crates: BTreeSet<&str> = types.iter().map(|path| crate_of(path)).collect();
    let crates: Vec<&str> = crates.into_iter().collect();
    let title = match crates.as_slice() {
        [only] => format!("This scene uses components from {only}, which is not available."),
        several => format!(
            "This scene uses components from {}, which are not available.",
            join_names(several)
        ),
    };
    let modules: BTreeSet<&str> = crates
        .iter()
        .filter_map(|name| module_for_crate(name))
        .collect();
    let modules: Vec<&str> = modules.into_iter().collect();
    let unprovided = crates.iter().any(|name| module_for_crate(name).is_none());
    let action = match (modules.as_slice(), unprovided) {
        ([], _) => "Build the project or enable the extension that provides them".to_string(),
        ([only], false) => format!("Enable the {only} module"),
        (several, false) => format!("Enable the {} modules", join_names(several)),
        ([only], true) => {
            format!("Enable the {only} module, and build the project for the rest")
        }
        (several, true) => format!(
            "Enable the {} modules, and build the project for the rest",
            join_names(several)
        ),
    };
    let hint = format!("{action} to edit them. They are kept when the scene is saved.");
    Some((title, hint))
}

fn join_names(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [only] => (*only).to_string(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The unavailable component types the open scene's document names.
#[derive(Resource, Default, Debug, PartialEq, Eq)]
pub struct UnavailableTypes(pub BTreeSet<String>);

/// Marks the notice in the inspector header.
#[derive(Component)]
pub(crate) struct UnavailableNotice;

/// Marks the notice's title line.
#[derive(Component)]
pub(crate) struct UnavailableNoticeTitle;

/// Marks the notice's hint line.
#[derive(Component)]
pub(crate) struct UnavailableNoticeHint;

/// The notice node for the inspector header; hidden until the scene holds a
/// component the editor cannot load.
pub(crate) fn unavailable_notice() -> impl Bundle {
    (
        UnavailableNotice,
        Node {
            display: Display::None,
            flex_direction: FlexDirection::Column,
            width: percent(100),
            row_gap: px(tokens::SPACING_XS),
            padding: UiRect::all(px(tokens::SPACING_SM)),
            border: UiRect::all(px(1.0)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
            ..Default::default()
        },
        BackgroundColor(tokens::ELEVATED_BG),
        BorderColor::all(tokens::TEXT_WARNING),
        children![
            (
                UnavailableNoticeTitle,
                Text::new(String::new()),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    weight: FontWeight::MEDIUM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_PRIMARY),
            ),
            (
                UnavailableNoticeHint,
                Text::new(String::new()),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_SECONDARY),
            ),
        ],
    )
}

/// Recompute [`UnavailableTypes`] when the document, the registry's view of
/// the project, or the project schema changes.
pub(crate) fn track_unavailable_types(
    ast: Option<Res<jackdaw_bsn::SceneBsnAst>>,
    registry: Res<AppTypeRegistry>,
    project_types: Res<ProjectTypes>,
    mut unavailable: ResMut<UnavailableTypes>,
) {
    let Some(ast) = ast else {
        if !unavailable.0.is_empty() {
            unavailable.0.clear();
        }
        return;
    };
    if !ast.is_changed() && !project_types.is_changed() && !unavailable.is_added() {
        return;
    }
    let registry = registry.read();
    let found: BTreeSet<String> = ast
        .all_patch_type_paths()
        .filter(|path| is_unavailable(&registry, &project_types, path))
        .map(str::to_string)
        .collect();
    if unavailable.0 != found {
        unavailable.0 = found;
    }
}

/// Show or hide the notice and keep its text in line with
/// [`UnavailableTypes`].
pub(crate) fn update_unavailable_notice(
    unavailable: Res<UnavailableTypes>,
    added: Query<(), Added<UnavailableNotice>>,
    mut notices: Query<&mut Node, With<UnavailableNotice>>,
    mut titles: Query<&mut Text, (With<UnavailableNoticeTitle>, Without<UnavailableNoticeHint>)>,
    mut hints: Query<&mut Text, (With<UnavailableNoticeHint>, Without<UnavailableNoticeTitle>)>,
) {
    if !unavailable.is_changed() && added.is_empty() {
        return;
    }
    let shown = notice(&unavailable.0);
    for mut node in &mut notices {
        node.display = if shown.is_some() {
            Display::Flex
        } else {
            Display::None
        };
    }
    let (title, hint) = shown.unwrap_or_default();
    for mut text in &mut titles {
        text.0.clone_from(&title);
    }
    for mut text in &mut hints {
        text.0.clone_from(&hint);
    }
}

/// The notice's title and hint while it shows, as a person reads them.
pub fn notice_showing(world: &mut World) -> Option<String> {
    let mut notices = world.query_filtered::<(&Node, &Children), With<UnavailableNotice>>();
    let (node, children) = notices.iter(world).next()?;
    if node.display == Display::None {
        return None;
    }
    let lines: Vec<String> = children
        .iter()
        .filter_map(|child| world.get::<Text>(child))
        .map(|text| text.0.clone())
        .collect();
    Some(lines.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(paths: &[&str]) -> BTreeSet<String> {
        paths.iter().map(|path| (*path).to_string()).collect()
    }

    /// A component path in each crate module `id` provides.
    fn provided_by(id: &str) -> (Vec<String>, Vec<String>) {
        let module = ModuleCatalog::builtin().get(id).expect("a catalog module");
        let paths = module
            .provides
            .iter()
            .map(|krate| format!("{krate}::Body"))
            .collect();
        (module.provides.clone(), paths)
    }

    #[test]
    fn a_crate_a_module_provides_names_the_module() {
        let (crates, paths) = provided_by("physics");
        let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
        let (title, hint) = notice(&set(&paths)).expect("a notice");
        let mut crates: Vec<&str> = crates.iter().map(String::as_str).collect();
        crates.sort_unstable();
        assert_eq!(
            title,
            format!(
                "This scene uses components from {}, which are not available.",
                join_names(&crates)
            )
        );
        assert!(hint.starts_with("Enable the Physics module to edit them."));
    }

    #[test]
    fn a_crate_no_module_provides_is_named_as_it_is() {
        let (title, hint) = notice(&set(&["vehicle_tools::Suspension"])).expect("a notice");
        assert_eq!(
            title,
            "This scene uses components from vehicle_tools, which is not available."
        );
        assert!(hint.starts_with("Build the project or enable the extension"));
    }

    #[test]
    fn modules_and_other_crates_together_name_both_remedies() {
        let (_, navmesh) = provided_by("navmesh");
        let (_, physics) = provided_by("physics");
        let (_, hint) = notice(&set(&[
            navmesh[0].as_str(),
            physics[0].as_str(),
            "vehicle_tools::Suspension",
        ]))
        .expect("a notice");
        assert!(hint.starts_with(
            "Enable the Navmesh and Physics modules, and build the project for the rest"
        ));
    }

    #[test]
    fn a_variant_card_names_its_enum() {
        assert_eq!(
            card_name("vehicle_tools::drive::Drivetrain::Rear"),
            "Drivetrain::Rear"
        );
        assert_eq!(card_name("vehicle_tools::drive::Suspension"), "Suspension");
        assert_eq!(card_name("Lonely"), "Lonely");
    }

    #[test]
    fn nothing_unavailable_has_no_notice() {
        assert!(notice(&BTreeSet::new()).is_none());
    }

    #[test]
    fn a_registered_type_and_its_variants_are_available() {
        let mut registry = bevy::reflect::TypeRegistry::default();
        registry.register::<Visibility>();
        let project = ProjectTypes::default();
        assert!(!is_unavailable(
            &registry,
            &project,
            "bevy_camera::visibility::Visibility"
        ));
        assert!(!is_unavailable(
            &registry,
            &project,
            "bevy_camera::visibility::Visibility::Hidden"
        ));
        assert!(is_unavailable(
            &registry,
            &project,
            "vehicle_tools::Suspension"
        ));
    }
}
