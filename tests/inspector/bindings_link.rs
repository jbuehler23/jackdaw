//! `jackdaw_bind`'s authoring types have to be visible to the editor, not just to
//! the games that run them: `Bindings` reaches the Add Component picker and gets
//! an inspector card, while `BindContext`, which game code inserts, does not.

use crate::util;

use std::collections::HashSet;

use bevy::ecs::reflect::AppTypeRegistry;
use bevy::prelude::*;
use jackdaw::inspector::component_picker::{
    PickableComponent, PickerDenylist, enumerate_pickable_components,
};
use jackdaw_api::op::OperatorWorldExt as _;
use jackdaw_api::prelude::JackdawExtension as _;
use jackdaw_bind::Bindings;

use crate::util::OperatorResultExt as _;

fn enumerate(app: &mut App) -> Vec<PickableComponent> {
    let registry = app.world().resource::<AppTypeRegistry>().clone();
    let registry = registry.read();
    enumerate_pickable_components(&registry, &HashSet::new(), &PickerDenylist::default())
}

fn find<'a>(pickables: &'a [PickableComponent], path: &str) -> Option<&'a PickableComponent> {
    pickables.iter().find(|p| p.type_path_full == path)
}

#[test]
fn bindings_is_pickable() {
    let mut app = util::editor_test_app();
    let pickables = enumerate(&mut app);

    let entry = find(&pickables, "jackdaw_bind::types::Bindings").unwrap_or_else(|| {
        panic!(
            "`jackdaw_bind::types::Bindings` must surface in the picker; \
             registered = {}, total pickables = {}",
            app.world()
                .resource::<AppTypeRegistry>()
                .read()
                .get_with_type_path("jackdaw_bind::types::Bindings")
                .is_some(),
            pickables.len(),
        )
    });
    // The picker falls back to the reflected doc comment for a type with
    // no `EditorDescription`, which makes that doc comment user-facing
    // copy: it is the sentence the user reads before adding a component.
    assert_eq!(
        entry.description, "Declarative connections from this widget to game state and events.",
        "`Bindings`'s picker description has to describe bindings to the user, \
         not explain the type's reflection plumbing to a maintainer",
    );
}

#[test]
fn bind_context_is_not_pickable() {
    let mut app = util::editor_test_app();
    assert!(
        app.world()
            .resource::<AppTypeRegistry>()
            .read()
            .get_with_type_path("jackdaw_bind::types::BindContext")
            .is_some(),
        "`BindContext` still has to be registered so authored scenes round-trip it",
    );

    let pickables = enumerate(&mut app);
    assert!(
        find(&pickables, "jackdaw_bind::types::BindContext").is_none(),
        "`BindContext` holds an `Entity` with no sensible default; the picker \
         must not offer a component it can only add as a dangling reference",
    );
}

#[test]
fn an_entity_with_bindings_gets_an_inspector_card() {
    let mut app = util::editor_test_app();
    // `selection.select` is the viewport panel extension's operator.
    jackdaw_api_internal::lifecycle::enable_extension(
        app.world_mut(),
        &jackdaw::builtin_extensions::ViewportExtension.id(),
    );
    app.update();
    app.world_mut()
        .spawn(jackdaw::layout::inspector_components_content(default()));
    app.world_mut()
        .spawn((Name::new("Play"), Node::default(), Bindings::default()));
    app.update();

    app.world_mut()
        .operator("selection.select")
        .param("name", "Play")
        .call()
        .expect("selection.select dispatches")
        .assert_finished();
    app.update();

    let labels = component_card_labels(&mut app);
    assert!(
        labels.iter().any(|label| label == "Bindings"),
        "the selected node's `Bindings` has to have a card: {labels:?}",
    );
}

/// Every text drawn inside an inspector component card.
fn component_card_labels(app: &mut App) -> Vec<String> {
    use jackdaw::inspector::ComponentDisplay;

    let cards: Vec<Entity> = app
        .world_mut()
        .query_filtered::<Entity, With<ComponentDisplay>>()
        .iter(app.world())
        .collect();
    let mut labels = Vec::new();
    for card in cards {
        let mut stack = vec![card];
        while let Some(entity) = stack.pop() {
            if let Some(text) = app.world().get::<Text>(entity) {
                labels.push(text.0.clone());
            }
            if let Some(children) = app.world().get::<Children>(entity) {
                stack.extend(children.iter());
            }
        }
    }
    labels
}
