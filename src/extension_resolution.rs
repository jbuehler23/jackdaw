//! Persistence for the enabled-extensions list at
//! `~/.config/jackdaw/extensions.json`. Read on startup, rewritten
//! whenever the user toggles an extension.

use bevy::{platform::collections::HashMap, prelude::*};
use jackdaw_api::prelude::ExtensionKind;
use jackdaw_api_internal::{extensions_config::read_extension_config, lifecycle::ExtensionCatalog};

/// Extensions that must always be loaded; the editor panics without
/// the resources they install. Anything listed here is force-enabled
/// in [`resolve_enabled_list`] regardless of what's persisted on
/// disk, so a stale config (e.g. one written before the extension
/// was extracted) can't take the editor down. The Extensions dialog
/// should also hide or lock these so users can't try to turn them
/// off.
pub const REQUIRED_EXTENSIONS: &[&str] = &[crate::core_extension::CORE_EXTENSION_ID];

/// True if the named extension is load-bearing and must not be
/// user-toggleable.
pub fn is_required(name: &str) -> bool {
    REQUIRED_EXTENSIONS.contains(&name)
}

/// Resolve which catalog entries to enable on startup.
///
/// Pre-dogfood files list none of the built-ins; fall back to enabling
/// everything so the editor stays usable until the next toggle rewrites
/// the file. Files that already record at least one built-in are
/// trusted exactly as written.
pub fn resolve_enabled_list(world: &World) -> Vec<String> {
    let catalog = world.resource::<ExtensionCatalog>();
    let available: Vec<String> = catalog.iter().map(ToString::to_string).collect();
    let builtins: Vec<String> = catalog
        .iter_with_content()
        .filter(|(.., kind)| *kind == ExtensionKind::Builtin)
        .map(|(id, ..)| id.to_string())
        .collect();

    let enabled_in_config: Option<HashMap<String, bool>> = read_extension_config().map(|config| {
        config
            .iter()
            .map(|(id, entry)| (id.clone(), entry.enabled))
            .collect()
    });

    resolve_against_config(&available, &builtins, enabled_in_config.as_ref())
}

/// The startup decision, with the file already read.
///
/// Three rules, in order:
///
/// 1. No file, or a file naming no built-in at all: enable everything.
/// 2. Otherwise the file is trusted for every extension it names.
/// 3. A built-in the file does not name postdates the file, and arrives
///    enabled. A file cannot name an extension that did not exist when it was
///    written, so treating "absent" as "disabled" would keep such a built-in
///    permanently off. Disabling it is then an explicit choice, which the next
///    toggle records. Only built-ins get this; a third-party extension the file
///    does not name counts as not installed.
///
/// [`REQUIRED_EXTENSIONS`] is a separate, stronger rule: those are
/// force-enabled even against a file that explicitly disables them, since the
/// editor panics without them.
fn resolve_against_config(
    available: &[String],
    builtins: &[String],
    enabled_in_config: Option<&HashMap<String, bool>>,
) -> Vec<String> {
    let mut resolved: Vec<String> = match enabled_in_config {
        Some(config) if builtins.iter().any(|id| config.contains_key(id)) => available
            .iter()
            .filter(|id| match config.get(*id) {
                Some(enabled) => *enabled,
                None => builtins.contains(id),
            })
            .cloned()
            .collect(),
        _ => available.to_vec(),
    };

    // Force-include any REQUIRED extension the catalog knows about but the
    // resolved list dropped, such as one the persisted config predates.
    // Without this, upgrading into a build that moved a resource into a new
    // required extension panics on first launch.
    for required in REQUIRED_EXTENSIONS {
        let in_catalog = available.iter().any(|n| n == required);
        let already_listed = resolved.iter().any(|n| n == required);
        if in_catalog && !already_listed {
            resolved.push((*required).to_string());
        }
    }

    resolved
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(ToString::to_string).collect()
    }

    fn config(entries: &[(&str, bool)]) -> HashMap<String, bool> {
        entries
            .iter()
            .map(|(id, enabled)| ((*id).to_string(), *enabled))
            .collect()
    }

    #[test]
    fn a_builtin_the_config_predates_arrives_enabled() {
        let available = ids(&["jackdaw.core", "jackdaw.inspector", "jackdaw.ui_palette"]);
        let builtins = available.clone();
        // A file written before the palette existed: it names other built-ins,
        // so it is trusted, but it cannot have named this one.
        let written_before = config(&[("jackdaw.core", true), ("jackdaw.inspector", true)]);

        let resolved = resolve_against_config(&available, &builtins, Some(&written_before));

        assert!(
            resolved.contains(&"jackdaw.ui_palette".to_string()),
            "a built-in added after the config was written must not ship dark: {resolved:?}",
        );
    }

    #[test]
    fn a_builtin_the_user_turned_off_stays_off() {
        let available = ids(&["jackdaw.core", "jackdaw.ui_palette"]);
        let builtins = available.clone();
        let turned_off = config(&[("jackdaw.core", true), ("jackdaw.ui_palette", false)]);

        let resolved = resolve_against_config(&available, &builtins, Some(&turned_off));

        assert!(
            !resolved.contains(&"jackdaw.ui_palette".to_string()),
            "the new-builtin rule must not override an explicit choice: {resolved:?}",
        );
    }

    #[test]
    fn an_unlisted_third_party_extension_stays_off() {
        let available = ids(&["jackdaw.core", "someone.else"]);
        let builtins = ids(&["jackdaw.core"]);
        let trusted = config(&[("jackdaw.core", true)]);

        let resolved = resolve_against_config(&available, &builtins, Some(&trusted));

        assert_eq!(resolved, ids(&["jackdaw.core"]));
    }

    #[test]
    fn a_config_naming_no_builtin_enables_everything() {
        let available = ids(&["jackdaw.core", "someone.else"]);
        let builtins = ids(&["jackdaw.core"]);
        let pre_dogfood = config(&[("someone.else", true)]);

        let resolved = resolve_against_config(&available, &builtins, Some(&pre_dogfood));

        assert_eq!(resolved, available);
    }
}
