//! Localization for jackdaw Editor.

use bevy::asset::{LoadedFolder, embedded_asset};
use bevy::prelude::*;
use bevy_fluent::prelude::*;
use fluent_content::Content;
use unic_langid::langid;

macro_rules! supported_languages {
    ($($lang:literal),+ $(,)?) => {
        pub const SUPPORTED_LANGUAGES: &[&str] = &[$($lang),+];

        /// Macro-generated boilerplate to register our locale assets.
        fn embed_locale_assets(app: &mut App) {
            $(
                embedded_asset!(app, concat!("locales/", $lang, "/main.ftl"));
                embedded_asset!(app, concat!("locales/", $lang, "/main.ftl.yml"));
            )+
        }
    };
}

supported_languages!("en-US");

pub struct LocalizationPlugin;

impl Plugin for LocalizationPlugin {
    fn build(&self, app: &mut App) {
        embed_locale_assets(app);

        // TODO: Offer a way for user to customize language + persist to disk.
        let locale = sys_locale::get_locale().unwrap_or_else(|| String::from("en-US"));
        let parsed_locale = locale.parse().unwrap_or(langid!("en-US"));
        app.insert_resource(SelectedLocale { locale });
        app.insert_resource(Locale::new(parsed_locale).with_default(langid!("en-US")));

        app.add_plugins(FluentPlugin);
        app.init_resource::<LocaleFolder>();
        app.init_resource::<Localization>();

        app.add_systems(PreStartup, load_editor_locales);
        app.add_systems(
            Update,
            (
                update_used_locale.run_if(resource_changed::<SelectedLocale>),
                rebuild_localization.run_if(localization_needs_rebuild),
                update_all_text.run_if(resource_changed::<Localization>),
                update_changed_text,
            )
                .chain(),
        );
    }
}

#[derive(Resource)]
pub struct SelectedLocale {
    pub locale: String,
}

#[derive(Resource, Default)]
pub struct LocaleFolder(pub Option<Handle<LoadedFolder>>);

#[derive(Component, Default, Reflect)]
#[require(Text)]
pub struct LocalizedText(pub String);

impl LocalizedText {
    pub fn new(request: impl Into<String>) -> Self {
        Self(request.into())
    }
}

fn load_editor_locales(asset_server: Res<AssetServer>, mut folder: ResMut<LocaleFolder>) {
    folder.0 = Some(asset_server.load_folder("embedded://jackdaw_localization/locales"));
}

/// Parse [`SelectedLocale`] and store it into [`Locale`].
pub fn update_used_locale(selected: Res<SelectedLocale>, mut locale: ResMut<Locale>) {
    locale.requested = selected.locale.parse().unwrap_or(langid!("en-US"));
}

fn localization_needs_rebuild(
    locale: Res<Locale>,
    mut folder_events: MessageReader<AssetEvent<LoadedFolder>>,
    mut bundle_events: MessageReader<AssetEvent<BundleAsset>>,
) -> bool {
    let folder_ready = folder_events
        .read()
        .any(|e| matches!(e, AssetEvent::LoadedWithDependencies { .. }));
    let bundle_ready = bundle_events.read().any(|e| {
        matches!(
            e,
            AssetEvent::LoadedWithDependencies { .. } | AssetEvent::Modified { .. }
        )
    });
    locale.is_changed() || folder_ready || bundle_ready
}

fn rebuild_localization(
    builder: LocalizationBuilder,
    folder: Res<LocaleFolder>,
    mut localization: ResMut<Localization>,
) {
    let Some(handle) = folder.0.as_ref() else {
        return;
    };
    *localization = builder.build(handle);
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
