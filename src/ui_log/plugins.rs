use bevy::prelude::*;

use super::core::*;
use super::systems::*;
use super::observers::*;

pub struct UiLogPlugin;

impl Plugin for UiLogPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiLogs>();
        if let Some(rx) = take_receiver() {
            app.insert_resource(UiLogReceiver(std::sync::Mutex::new(rx)));
        }
        app.add_systems(
            Update,
            (
                poll_global_logs.run_if(resource_exists::<UiLogReceiver>),
                tick_latest_log,
                update_status_bar_ui,
                update_log_area_background,
                (
                    populate_logs_dialog,
                    apply_logs_filter,
                    update_filter_button_styles,
                    update_logs_list_ui,
                )
                    .run_if(resource_exists::<LogsDialogOpen>),
            )
                .run_if(in_state(crate::AppState::Editor)),
        );
        app.add_systems(
            Update,
            (
                attach_log_area_click_observer,
                attach_filter_button_observers,
            )
                .run_if(in_state(crate::AppState::Editor)),
        );
        app.add_observer(on_logs_list_container_added)
            .add_observer(on_logs_dialog_removed);
    }
}
