//! Localization for jackdaw Editor.
//!
//! Each locale's `.ftl` source is compiled into the binary with `include_str!`
//! and parsed into a Fluent bundle. Localization is therefore self-contained:
//! no runtime asset loading, no asset-path resolution, and only published
//! crates as dependencies. [`LocalizedText`] components carry a message key;
//! the systems here resolve each key to its localized string through the active
//! [`Localization`] bundle.

use bevy::prelude::*;
use fluent::FluentResource;
use fluent::concurrent::FluentBundle;
use fluent_content::Content;
use unic_langid::{LanguageIdentifier, langid};

/// Languages compiled into the binary, each paired with its `.ftl` source. Add
/// a row here (and a `locales/<lang>/main.ftl` file) to support another
/// language.
const LOCALES: &[(&str, &str)] = &[("en-US", include_str!("locales/en-US/main.ftl"))];

/// Locale used when the requested one is unknown or unparsable.
const DEFAULT_LOCALE: &str = "en-US";

pub struct LocalizationPlugin;

impl Plugin for LocalizationPlugin {
    fn build(&self, app: &mut App) {
        // TODO: Offer a way for the user to customize language + persist to disk.
        let requested = sys_locale::get_locale().unwrap_or_else(|| DEFAULT_LOCALE.to_owned());
        app.insert_resource(SelectedLocale { locale: requested });
        app.init_resource::<Localization>();

        app.add_systems(
            Update,
            (
                rebuild_localization.run_if(resource_changed::<SelectedLocale>),
                update_all_text.run_if(resource_changed::<Localization>),
                update_changed_text,
            )
                .chain(),
        );
    }
}

/// The language the editor should display, as a BCP-47 string (for example
/// `en-US`). Mutate it to switch languages; [`Localization`] rebuilds on the
/// next frame.
#[derive(Resource)]
pub struct SelectedLocale {
    pub locale: String,
}

#[derive(Component, Default, Reflect)]
#[require(Text)]
pub struct LocalizedText(pub String);

impl LocalizedText {
    pub fn new(request: impl Into<String>) -> Self {
        Self(request.into())
    }
}

/// The active Fluent bundle. Resolves message keys to localized strings.
#[derive(Resource)]
pub struct Localization {
    bundle: FluentBundle<FluentResource>,
}

impl Default for Localization {
    fn default() -> Self {
        Self::for_locale(DEFAULT_LOCALE)
    }
}

impl Localization {
    /// Build the bundle for `tag`, falling back to [`DEFAULT_LOCALE`] when `tag`
    /// is not a compiled-in language.
    fn for_locale(tag: &str) -> Self {
        let (lang, ftl) = LOCALES
            .iter()
            .copied()
            .find(|(lang, _)| *lang == tag)
            .or_else(|| {
                LOCALES
                    .iter()
                    .copied()
                    .find(|(lang, _)| *lang == DEFAULT_LOCALE)
            })
            .unwrap_or(("en-US", ""));

        let langid: LanguageIdentifier = lang.parse().unwrap_or(langid!("en-US"));
        let mut bundle = FluentBundle::new_concurrent(vec![langid]);
        // Our message values have no interpolation, so the bidi isolation marks
        // Fluent would otherwise wrap content in are pure noise (invisible
        // control characters in the rendered string).
        bundle.set_use_isolating(false);

        let resource = match FluentResource::try_new(ftl.to_owned()) {
            Ok(resource) => resource,
            Err((resource, errors)) => {
                warn!(
                    "locale {lang}: {} FTL parse error(s); using partial",
                    errors.len()
                );
                resource
            }
        };
        if let Err(errors) = bundle.add_resource(resource) {
            warn!("locale {lang}: {} message error(s)", errors.len());
        }

        Self { bundle }
    }

    /// Resolve a message key to its localized content, or `None` when the key is
    /// absent.
    pub fn content(&self, key: &str) -> Option<String> {
        self.bundle.content(key)
    }
}

/// Rebuild the bundle when the selected locale changes.
fn rebuild_localization(selected: Res<SelectedLocale>, mut localization: ResMut<Localization>) {
    *localization = Localization::for_locale(&selected.locale);
}

/// Re-resolve every [`LocalizedText`] after the active locale changes.
fn update_all_text(localization: Res<Localization>, mut q: Query<(&LocalizedText, &mut Text)>) {
    for (loc, mut text) in q.iter_mut() {
        text.0 = localization
            .content(&loc.0)
            .unwrap_or_else(|| loc.0.clone());
    }
}

/// Resolve newly inserted or mutated [`LocalizedText`] entries.
fn update_changed_text(
    localization: Res<Localization>,
    mut q: Query<(&LocalizedText, &mut Text), Changed<LocalizedText>>,
) {
    for (loc, mut text) in q.iter_mut() {
        text.0 = localization
            .content(&loc.0)
            .unwrap_or_else(|| loc.0.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_known_keys() {
        let loc = Localization::for_locale("en-US");
        assert_eq!(loc.content("cancel").as_deref(), Some("Cancel"));
        assert_eq!(loc.content("add-node").as_deref(), Some("Add Node"));
        assert_eq!(loc.content("ready").as_deref(), Some("Ready"));
    }

    #[test]
    fn unknown_key_is_none() {
        let loc = Localization::for_locale("en-US");
        assert_eq!(loc.content("definitely-not-a-key"), None);
    }

    #[test]
    fn unknown_locale_falls_back_to_default() {
        // An unsupported tag still yields a working default-locale bundle.
        let loc = Localization::for_locale("zz-ZZ");
        assert_eq!(loc.content("submit").as_deref(), Some("Submit"));
    }
}
