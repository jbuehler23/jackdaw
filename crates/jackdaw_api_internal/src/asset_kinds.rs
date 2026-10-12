//! Asset kinds: the types the editor can create and edit one value per file.
//!
//! A kind is a reflected type plus the name the menus call it by. Where the
//! type comes from is what differs: the editor has some compiled in, an
//! extension registers its own, and the rest come from the open project's
//! schema, which reports every type the game registers as a reflected asset.
//! Nothing here says where a file lives; a file names its type in its header
//! and can sit in any folder.

use bevy::prelude::*;
use lucide_icons::Icon;

/// Where the editor got a kind's type from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetKindSource {
    /// The editor has the type compiled in and its own subsystem loads the
    /// files.
    Compiled,
    /// The open project reported the type in its schema.
    Schema,
    /// An extension registered the type.
    Extension,
}

/// One kind of asset file, as the menus and the operators name it.
#[derive(Clone, Debug)]
pub struct AssetKind {
    /// Short identifier used by the operators and reported by the remote,
    /// for example `item`.
    pub kind: String,
    /// Name shown on menus and tiles, for example `Item`.
    pub label: String,
    /// Reflect type path of the value a file of this kind holds.
    pub type_path: String,
    /// Icon the browser draws for a file of this kind.
    pub icon: Icon,
    pub source: AssetKindSource,
}

/// The icon a kind draws with when it names no other.
pub const DEFAULT_KIND_ICON: Icon = Icon::FileBox;

impl AssetKind {
    /// A kind whose type the editor has compiled in.
    pub fn compiled(
        kind: impl Into<String>,
        label: impl Into<String>,
        type_path: impl Into<String>,
    ) -> Self {
        Self::new(kind, label, type_path, AssetKindSource::Compiled)
    }

    /// A kind an extension registered.
    pub fn extension(
        kind: impl Into<String>,
        label: impl Into<String>,
        type_path: impl Into<String>,
    ) -> Self {
        Self::new(kind, label, type_path, AssetKindSource::Extension)
    }

    /// A kind the open project's schema reported, named after its type.
    pub fn from_schema(type_path: impl Into<String>) -> Self {
        let type_path = type_path.into();
        let kind = kind_of_type(&type_path);
        let label = label_of_type(&type_path);
        Self::new(kind, label, type_path, AssetKindSource::Schema)
    }

    fn new(
        kind: impl Into<String>,
        label: impl Into<String>,
        type_path: impl Into<String>,
        source: AssetKindSource,
    ) -> Self {
        Self {
            kind: kind.into(),
            label: label.into(),
            type_path: type_path.into(),
            icon: DEFAULT_KIND_ICON,
            source,
        }
    }

    pub fn with_icon(mut self, icon: Icon) -> Self {
        self.icon = icon;
        self
    }

    pub fn with_label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    /// Whether the editor knows this type only as the project's schema, so its
    /// values live as document patches rather than in an asset store.
    pub fn schema_backed(&self) -> bool {
        self.source == AssetKindSource::Schema
    }

    /// Whether the editor's own scan loads this kind's files. A compiled kind
    /// is loaded by the subsystem that owns it.
    pub fn scanned(&self) -> bool {
        self.source != AssetKindSource::Compiled
    }
}

/// The kind id a type path implies: its last segment without a trailing `Def`
/// or `Definition`, in snake case.
pub fn kind_of_type(type_path: &str) -> String {
    let name = short_name(type_path);
    let stem = strip_definition_suffix(name);
    let mut id = String::with_capacity(stem.len() + 4);
    for (index, character) in stem.char_indices() {
        if character.is_ascii_uppercase() {
            if index > 0 {
                id.push('_');
            }
            id.push(character.to_ascii_lowercase());
        } else {
            id.push(character);
        }
    }
    if id.is_empty() {
        "asset".to_string()
    } else {
        id
    }
}

/// The label a type path implies: its last segment without a trailing `Def` or
/// `Definition`, spaced at its capitals.
pub fn label_of_type(type_path: &str) -> String {
    let stem = strip_definition_suffix(short_name(type_path));
    let mut label = String::with_capacity(stem.len() + 4);
    for (index, character) in stem.char_indices() {
        if character.is_ascii_uppercase() && index > 0 {
            label.push(' ');
        }
        label.push(character);
    }
    if label.is_empty() {
        "Asset".to_string()
    } else {
        label
    }
}

fn short_name(type_path: &str) -> &str {
    let name = type_path.rsplit("::").next().unwrap_or(type_path);
    name.split('<').next().unwrap_or(name)
}

fn strip_definition_suffix(name: &str) -> &str {
    for suffix in ["Definition", "Def"] {
        if let Some(stem) = name.strip_suffix(suffix)
            && !stem.is_empty()
        {
            return stem;
        }
    }
    name
}

/// Every asset kind the editor offers.
#[derive(Resource, Default)]
pub struct AssetKinds {
    kinds: Vec<AssetKind>,
}

impl AssetKinds {
    /// Register a kind, replacing an earlier one of the same id. A kind the
    /// editor or an extension supplies keeps both its id and its type against
    /// one the schema reports, so a rebuild never takes a compiled kind over.
    pub fn register(&mut self, kind: AssetKind) {
        if kind.source == AssetKindSource::Schema && self.owned_elsewhere(&kind) {
            return;
        }
        self.kinds.retain(|known| {
            known.kind != kind.kind
                && !(known.type_path == kind.type_path && known.source == AssetKindSource::Schema)
        });
        self.kinds.push(kind);
    }

    pub fn unregister(&mut self, kind: &str) {
        self.kinds.retain(|known| known.kind != kind);
    }

    pub fn by_kind(&self, kind: &str) -> Option<&AssetKind> {
        self.kinds.iter().find(|known| known.kind == kind)
    }

    pub fn by_type_path(&self, type_path: &str) -> Option<&AssetKind> {
        self.kinds.iter().find(|known| known.type_path == type_path)
    }

    /// The kind a caller named: the id the operators take, the type path a
    /// file's header carries, or the label the menus show.
    pub fn by_name(&self, named: &str) -> Option<&AssetKind> {
        self.by_kind(named)
            .or_else(|| self.by_type_path(named))
            .or_else(|| self.by_label(named))
    }

    /// The one kind shown under a label, and nothing when two kinds share it.
    pub fn by_label(&self, label: &str) -> Option<&AssetKind> {
        let mut shown = self
            .kinds
            .iter()
            .filter(|known| known.label.eq_ignore_ascii_case(label));
        let first = shown.next()?;
        shown.next().is_none().then_some(first)
    }

    pub fn iter(&self) -> impl Iterator<Item = &AssetKind> {
        self.kinds.iter()
    }

    fn owned_elsewhere(&self, kind: &AssetKind) -> bool {
        self.kinds.iter().any(|known| {
            known.source != AssetKindSource::Schema
                && (known.kind == kind.kind || known.type_path == kind.type_path)
        })
    }
}

/// Marks an entity as tracking an asset kind an extension registered. An
/// observer unregisters the kind when the marker despawns.
#[derive(Component, Clone, Debug)]
pub struct RegisteredAssetKind {
    pub(crate) kind: String,
}

pub(crate) fn cleanup_asset_kind_on_remove(
    trigger: On<Remove<RegisteredAssetKind>>,
    registrations: Query<&RegisteredAssetKind>,
    mut kinds: ResMut<AssetKinds>,
) {
    if let Ok(registration) = registrations.get(trigger.event_target()) {
        kinds.unregister(&registration.kind);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kind_is_named_after_its_type() {
        assert_eq!(kind_of_type("my_game::content::ItemDef"), "item");
        assert_eq!(kind_of_type("my_game::OutfitDefinition"), "outfit");
        assert_eq!(kind_of_type("my_game::MobArchetype"), "mob_archetype");
        assert_eq!(label_of_type("my_game::content::ItemDef"), "Item");
        assert_eq!(label_of_type("my_game::MobArchetype"), "Mob Archetype");
    }

    #[test]
    fn a_type_called_only_def_keeps_its_name() {
        assert_eq!(kind_of_type("my_game::Def"), "def");
        assert_eq!(label_of_type("my_game::Def"), "Def");
    }

    #[test]
    fn registering_a_kind_again_replaces_the_earlier_registration() {
        let mut kinds = AssetKinds::default();
        kinds.register(AssetKind::extension("item", "Item", "content::ItemDef"));
        kinds.register(AssetKind::extension("item", "Item", "content::OtherDef"));
        assert_eq!(kinds.iter().count(), 1);
        assert_eq!(
            kinds.by_kind("item").unwrap().type_path,
            "content::OtherDef"
        );
    }

    #[test]
    fn a_kind_the_editor_compiled_in_wins_over_the_schemas() {
        let mut kinds = AssetKinds::default();
        kinds.register(AssetKind::compiled(
            "material",
            "Material",
            "bevy_pbr::pbr_material::StandardMaterial",
        ));
        kinds.register(AssetKind::from_schema(
            "bevy_pbr::pbr_material::StandardMaterial",
        ));

        assert_eq!(kinds.iter().count(), 1);
        let material = kinds.by_kind("material").expect("the compiled kind stands");
        assert_eq!(material.source, AssetKindSource::Compiled);
        assert!(!material.schema_backed());
        assert!(!material.scanned());
    }

    #[test]
    fn a_schema_kind_does_not_take_the_name_of_a_compiled_one() {
        let mut kinds = AssetKinds::default();
        kinds.register(AssetKind::compiled(
            "material",
            "Material",
            "bevy_pbr::pbr_material::StandardMaterial",
        ));
        kinds.register(AssetKind::from_schema("my_game::content::MaterialDef"));

        assert_eq!(kinds.iter().count(), 1);
        assert_eq!(
            kinds
                .by_kind("material")
                .map(|kind| kind.type_path.as_str()),
            Some("bevy_pbr::pbr_material::StandardMaterial")
        );
    }

    #[test]
    fn a_schema_kind_gives_way_to_a_compiled_one_registered_after_it() {
        let mut kinds = AssetKinds::default();
        kinds.register(AssetKind::from_schema("my_game::AnimationGraphDef"));
        assert!(kinds.by_kind("animation_graph").is_some());

        kinds.register(AssetKind::compiled(
            "animation_graph",
            "Animation Graph",
            "my_game::AnimationGraphDef",
        ));

        assert_eq!(kinds.iter().count(), 1);
        assert_eq!(
            kinds.by_kind("animation_graph").map(|kind| kind.source),
            Some(AssetKindSource::Compiled)
        );
    }

    #[test]
    fn a_kind_answers_to_its_id_its_type_path_and_its_label() {
        let mut kinds = AssetKinds::default();
        kinds.register(AssetKind::from_schema("my_game::content::ItemDef"));

        for named in ["item", "my_game::content::ItemDef", "Item"] {
            assert_eq!(
                kinds.by_name(named).map(|kind| kind.kind.as_str()),
                Some("item"),
                "'{named}' names the kind"
            );
        }
        assert!(kinds.by_name("ItemDef").is_none());
    }

    #[test]
    fn a_label_two_kinds_share_names_neither_of_them() {
        let mut kinds = AssetKinds::default();
        kinds.register(AssetKind::from_schema("my_game::content::ItemDef"));
        kinds.register(AssetKind::compiled(
            "trade_item",
            "Item",
            "my_game::trade::Ware",
        ));

        assert!(
            kinds.by_name("Item").is_none(),
            "a label standing for two kinds chooses neither",
        );
        assert_eq!(
            kinds.by_name("trade_item").map(|kind| kind.kind.as_str()),
            Some("trade_item"),
            "and each is still named by its own id",
        );
    }

    #[test]
    fn a_schema_kind_is_edited_as_schema_and_scanned_for_files() {
        let kind = AssetKind::from_schema("my_game::content::ItemDef");
        assert_eq!(kind.kind, "item");
        assert_eq!(kind.label, "Item");
        assert!(kind.schema_backed());
        assert!(kind.scanned());
        assert_eq!(kind.icon.unicode(), DEFAULT_KIND_ICON.unicode());
    }
}
