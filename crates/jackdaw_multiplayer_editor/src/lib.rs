//! Editor extension surfacing Jackdaw's networking proxy components
//! (`jackdaw_multiplayer::Replication`, `NetworkRoom`) in the inspector. Pure
//! authoring: no networking runtime, no lightyear dependency.

use jackdaw_api::prelude::{ExtensionContext, ExtensionKind, JackdawExtension};

/// The user-toggleable "Multiplayer" extension. The proxy components are
/// inspector-authorable automatically once their types are registered (by
/// `jackdaw_multiplayer::JackdawMultiplayerTypesPlugin`, which the editor adds alongside this
/// extension), so `register` is a no-op.
#[derive(Default)]
pub struct MultiplayerExtension;

impl JackdawExtension for MultiplayerExtension {
    fn id(&self) -> String {
        "jackdaw.multiplayer".to_string()
    }
    fn label(&self) -> String {
        "Multiplayer".to_string()
    }
    fn description(&self) -> String {
        "Author backend-agnostic networking on entities (Replication, NetworkRoom). \
         A backend (default: lightyear) translates these to real networking at runtime."
            .to_string()
    }
    fn kind(&self) -> ExtensionKind {
        ExtensionKind::Builtin
    }
    fn register(&self, _ctx: &mut ExtensionContext) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_metadata_is_stable_and_builtin() {
        let ext = MultiplayerExtension;
        // The id is the stable key the catalog and saved-enabled-set use;
        // it must stay exactly this string. The "jackdaw." prefix marks it
        // a reserved built-in.
        assert_eq!(ext.id(), "jackdaw.multiplayer");
        assert_eq!(ext.label(), "Multiplayer");
        assert_eq!(ext.kind(), ExtensionKind::Builtin);
        assert!(!ext.description().is_empty());
    }
}
