//! Creates a new window chock-full of all our widgets.

use bevy::prelude::*;
use jackdaw::prelude::*;

fn main() -> AppExit {
    App::new()
        // log errors instead of panicking
        .set_error_handler(bevy::ecs::error::error)
        .add_plugins((
            DefaultPlugins,
            EditorPlugins::default()
                .set(ExtensionPlugin::default().with_extension::<ModalExampleExtension>()),
        ))
        .run()
}

#[derive(Default)]
pub struct ModalExampleExtension;

impl JackdawExtension for ModalExampleExtension {
    fn id(&self) -> String {
        "modal_example".to_string()
    }

    fn label(&self) -> String {
        "Modal Example".to_string()
    }

    fn description(&self) -> String {
        "Adds modal operator that prints the entity the mouse is hovering over".to_string()
    }

    fn register(&self, ctx: &mut ExtensionContext) {
        // Since this extension does not set up UI, press F3 and search for "Print hovered entity" to enable it.
        ctx.register_operator::<PrintHoveredEntityOp>()
            // For fetching the hovered entity, we use Bevy's built-in mesh picking, which will communicate with the
            // active modal via the `HoveredEntityModalState` resource.
            .add_observer(update_hovered_entity)
            .init_resource::<HoveredEntityModalState>();
    }
}

#[operator(
    id = "widget_showcase.button_operator",
    label = "Print hovered entity",
    description = "Prints the entity the mouse is hovering over while the modal is active.",
    modal = true
)]
fn print_hovered_entity(
    _: In<OperatorParameters>,
    active: ActiveModalQuery,
    mut state: ResMut<HoveredEntityModalState>,
    names: Query<NameOrEntity>,
) -> OperatorResult {
    if !active.is_modal_running() {
        // Only one modal can run at a time. So, if no modal is running, this operator just started running.
        // We use this branch here to do initial setup

        info!("The modal just started running!");

        // A modal returning `Running` keeps the operator active and the dispatcher re-runs it every frame,
        // until it returns `Finished` or `Cancelled`.
        return OperatorResult::Running;
    }

    let Some(entity) = state.entity.take() else {
        return OperatorResult::Running;
    };
    let name = names.get(entity).unwrap();

    info!("Hovered entity: {name}");

    // We could return OperatorResult::Finished here to stop the modal.
    // But all modals are already automatically cancelled when the user presses Escape,
    // so let's leave it at that for this modal.
    OperatorResult::Running
}

/// It's very common for modals to have a state component attached to the active window.
/// Use it to communicate events from observers (e.g. mesh picking, collision events, BEI actions, etc.)
/// with the running modal operator.
#[derive(Resource, Default)]
struct HoveredEntityModalState {
    entity: Option<Entity>,
}

fn update_hovered_entity(mut over: On<Pointer<Over>>, mut state: ResMut<HoveredEntityModalState>) {
    over.propagate(false);
    state.entity = Some(over.entity);
}
