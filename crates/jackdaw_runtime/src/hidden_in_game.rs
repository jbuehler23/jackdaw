//! Hide [`HiddenInGame`] entities in a standalone game.
//!
//! [`hide_on_add`] writes `Visibility::Hidden` and
//! [`InheritedVisibility::HIDDEN`] the moment the tag is inserted, so scene
//! load has no flash and children spawned afterward inherit the hide.

use bevy::prelude::*;
use jackdaw_scene_types::HiddenInGame;

/// Hide an entity the moment it is tagged [`HiddenInGame`]. Play-in-editor
/// leaves tagged volumes drawn so they can be placed against the running
/// game.
pub(crate) fn hide_on_add(
    add: On<Add, HiddenInGame>,
    mut vis: Query<(&mut Visibility, &mut InheritedVisibility)>,
    mut commands: Commands,
) {
    #[cfg(feature = "pie")]
    if crate::pie::pie_config().is_some() {
        return;
    }
    let entity = add.entity;
    if let Ok((mut visibility, mut inherited)) = vis.get_mut(entity) {
        *visibility = Visibility::Hidden;
        *inherited = InheritedVisibility::HIDDEN;
    } else {
        commands.entity(entity).insert(Visibility::Hidden);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_in_game_hides_visibility_when_added_later() {
        let mut app = App::new();
        app.add_observer(hide_on_add);
        let entity = app
            .world_mut()
            .spawn((Visibility::Inherited, InheritedVisibility::VISIBLE))
            .id();

        app.world_mut().entity_mut(entity).insert(HiddenInGame);

        assert_eq!(
            app.world().get::<Visibility>(entity).copied(),
            Some(Visibility::Hidden),
            "adding the tag after spawn still hides the entity"
        );
        assert_eq!(
            app.world().get::<InheritedVisibility>(entity).copied(),
            Some(InheritedVisibility::HIDDEN),
            "children spawned afterward inherit the hide"
        );
    }
}
