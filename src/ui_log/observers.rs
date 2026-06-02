use bevy::prelude::*;
use jackdaw_feathers::dialog::OpenDialogEvent;
use super::core::{
    LogLevelFilter, LogsDialogOpen, LogsDialogMarker, LogsListContainer, UiLogs,
};
use super::systems::populate_container_with_logs;

pub fn on_log_area_clicked(_click: On<Pointer<Click>>, mut commands: Commands) {
    commands.trigger(
        OpenDialogEvent::new("System Logs", "Close")
            .without_cancel()
            .with_max_width(Val::Px(600.0))
            .without_content_padding(),
    );

    commands.insert_resource(LogsDialogOpen);
}

pub fn on_filter_button_clicked(
    click: On<Pointer<Click>>,
    query: Query<&super::core::LogFilterButton>,
    mut ui_logs: ResMut<UiLogs>,
) {
    if let Ok(btn) = query.get(click.entity)
        && ui_logs.level_filter != btn.0
    {
        ui_logs.level_filter = btn.0;
    }
}

pub fn on_logs_list_container_added(
    trigger: On<Add, LogsListContainer>,
    mut commands: Commands,
    ui_logs: Res<UiLogs>,
    editor_font: Res<jackdaw_feathers::icons::EditorFont>,
) {
    populate_container_with_logs(
        &mut commands,
        trigger.entity,
        &ui_logs,
        &editor_font.0,
    );
}

pub fn on_logs_dialog_removed(
    _: On<Remove, LogsDialogMarker>,
    mut commands: Commands,
    mut ui_logs: ResMut<UiLogs>,
) {
    commands.remove_resource::<LogsDialogOpen>();
    ui_logs.search_query = String::new();
    ui_logs.level_filter = LogLevelFilter::All;
}
