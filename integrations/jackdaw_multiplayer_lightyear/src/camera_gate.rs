use bevy::prelude::*;
use jackdaw_camera_rig::{ActiveCameraRig, CameraRig};
use lightyear::prelude::Controlled;

/// Keep `ActiveCameraRig` on the camera rig owned by the locally-controlled actor, and
/// off every other. The rig is a descendant of its actor; walk up `ChildOf` to find the
/// owning actor and check `Controlled`.
pub fn sync_active_camera(
    mut commands: Commands,
    rigs: Query<(Entity, Has<ActiveCameraRig>), With<CameraRig>>,
    parents: Query<&ChildOf>,
    controlled: Query<(), With<Controlled>>,
) {
    for (rig, is_active) in &rigs {
        let mut node = rig;
        let mut owned_by_local = controlled.get(node).is_ok();
        while let Ok(child_of) = parents.get(node) {
            node = child_of.0;
            if controlled.get(node).is_ok() {
                owned_by_local = true;
                break;
            }
        }
        if owned_by_local && !is_active {
            // try_insert: the avatar (and its rig) can despawn the same frame.
            commands.entity(rig).try_insert(ActiveCameraRig);
        } else if !owned_by_local && is_active {
            commands.entity(rig).remove::<ActiveCameraRig>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(app: &mut App) {
        app.world_mut()
            .run_system_cached(sync_active_camera)
            .unwrap();
    }

    #[test]
    fn marks_rig_under_controlled_actor_only() {
        let mut app = App::new();
        let local_actor = app.world_mut().spawn(Controlled).id();
        let local_body = app.world_mut().spawn(ChildOf(local_actor)).id();
        let local_rig = app
            .world_mut()
            .spawn((CameraRig::default(), ChildOf(local_body)))
            .id();
        let remote_actor = app.world_mut().spawn_empty().id();
        let remote_body = app.world_mut().spawn(ChildOf(remote_actor)).id();
        let remote_rig = app
            .world_mut()
            .spawn((CameraRig::default(), ChildOf(remote_body)))
            .id();

        run(&mut app);
        assert!(app.world().entity(local_rig).contains::<ActiveCameraRig>());
        assert!(!app.world().entity(remote_rig).contains::<ActiveCameraRig>());

        app.world_mut()
            .entity_mut(local_actor)
            .remove::<Controlled>();
        run(&mut app);
        assert!(!app.world().entity(local_rig).contains::<ActiveCameraRig>());
    }
}
