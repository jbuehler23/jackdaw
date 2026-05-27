use bevy::prelude::*;
use jackdaw_feathers::icons::Icon;
use jackdaw_feathers::{
    button::ButtonVariant, dialog::DialogChildrenSlot, text_edit::TextEditValue, tokens,
};

use super::core::{
    LogEntry, LogFilterButton, LogLevelExt, LogsDialogMarker, LogsDialogPopulated, LogsFilterInput,
    LogsListContainer, StatusBarLogArea, StatusBarLogIcon, StatusBarLogText, UiLogReceiver, UiLogs,
};
use super::observers::{on_filter_button_clicked, on_log_area_clicked};

pub fn poll_global_logs(receiver: Res<UiLogReceiver>, mut ui_logs: ResMut<UiLogs>) {
    let Ok(rx) = receiver.0.lock() else {
        return;
    };

    let entries: Vec<LogEntry> = rx.try_iter().collect();
    if entries.is_empty() {
        return;
    }

    if let Some(latest) = entries.iter().rfind(|e| {
        matches!(
            e.level,
            tracing::Level::INFO | tracing::Level::WARN | tracing::Level::ERROR
        )
    }) {
        ui_logs.latest_log = Some(latest.clone());
        ui_logs.latest_log_timer = Some(5.0);
    }

    ui_logs.logs.extend(entries);

    if ui_logs.logs.len() > 1000 {
        let drain_count = ui_logs.logs.len() - 1000;
        ui_logs.logs.drain(0..drain_count);
    }
}

pub fn tick_latest_log(time: Res<Time>, mut ui_logs: ResMut<UiLogs>) {
    if let Some(ref mut timer) = ui_logs.latest_log_timer {
        *timer -= time.delta_secs();
        if *timer <= 0.0 {
            ui_logs.latest_log_timer = None;
            ui_logs.latest_log = None;
        }
    }
}

pub fn update_status_bar_ui(
    ui_logs: Res<UiLogs>,
    mut icon_query: Query<
        (&mut Text, &mut TextColor),
        (With<StatusBarLogIcon>, Without<StatusBarLogText>),
    >,
    mut text_query: Query<&mut Text, (With<StatusBarLogText>, Without<StatusBarLogIcon>)>,
) {
    let Ok((mut icon_text, mut icon_color)) = icon_query.single_mut() else {
        return;
    };
    let Ok(mut log_text) = text_query.single_mut() else {
        return;
    };

    if let Some(ref latest) = ui_logs.latest_log {
        let (icon, color) = latest.level.icon_and_color();

        icon_text.0 = String::from(icon.unicode());
        icon_color.0 = color;

        let max_len = 80;
        let mut msg = latest.message.replace('\n', " ");
        if msg.len() > max_len {
            msg.truncate(max_len - 3);
            msg.push_str("...");
        }
        log_text.0 = format!(" [{}] {}", latest.timestamp, msg);
    } else {
        icon_text.0 = String::from(Icon::Terminal.unicode());
        icon_color.0 = tokens::TEXT_MUTED_COLOR.into();
        log_text.0 = String::new();
    }
}

pub fn update_log_area_background(
    mut query: Query<(&Interaction, &mut BackgroundColor), With<StatusBarLogArea>>,
) {
    for (interaction, mut bg) in &mut query {
        *bg = match interaction {
            Interaction::Pressed => BackgroundColor(tokens::ACTIVE_BG),
            Interaction::Hovered => BackgroundColor(tokens::HOVER_BG),
            Interaction::None => BackgroundColor(Color::NONE),
        };
    }
}

pub fn attach_log_area_click_observer(
    mut commands: Commands,
    query: Query<Entity, Added<StatusBarLogArea>>,
) {
    for entity in &query {
        commands.entity(entity).observe(on_log_area_clicked);
    }
}

pub fn populate_logs_dialog(
    mut commands: Commands,
    slots: Query<Entity, (With<DialogChildrenSlot>, Added<DialogChildrenSlot>)>,
    populated: Query<(), With<LogsDialogPopulated>>,
) {
    if !populated.is_empty() {
        return;
    }

    for slot_entity in &slots {
        commands
            .entity(slot_entity)
            .insert((LogsDialogMarker, LogsDialogPopulated));

        let wrapper = commands
            .spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    width: Val::Percent(100.0),
                    row_gap: Val::Px(tokens::SPACING_MD),
                    padding: UiRect::all(Val::Px(tokens::SPACING_LG)),
                    ..Default::default()
                },
                ChildOf(slot_entity),
            ))
            .id();

        commands.spawn((super::core::logs_filter_row(), ChildOf(wrapper)));

        let scroll_wrapper = commands
            .spawn((
                Node {
                    position_type: PositionType::Relative,
                    width: Val::Percent(100.0),
                    height: Val::Px(400.0),
                    ..Default::default()
                },
                ChildOf(wrapper),
            ))
            .id();

        let scroll = commands
            .spawn((
                LogsListContainer,
                ScrollPosition::default(),
                Node {
                    flex_direction: FlexDirection::Column,
                    height: Val::Percent(100.0),
                    overflow: Overflow::scroll_y(),
                    width: Val::Percent(100.0),
                    border: UiRect::all(Val::Px(1.0)),
                    border_radius: BorderRadius::all(Val::Px(4.0)),
                    padding: UiRect::all(Val::Px(tokens::SPACING_SM)),
                    row_gap: Val::Px(2.0),
                    ..Default::default()
                },
                BackgroundColor(tokens::BACKGROUND_COLOR.into()),
                BorderColor::all(tokens::BORDER_SUBTLE),
                ChildOf(scroll_wrapper),
            ))
            .id();

        commands.spawn((
            jackdaw_feathers::scroll::scrollbar(scroll),
            ChildOf(scroll_wrapper),
        ));
    }
}

pub fn apply_logs_filter(
    filter_input: Query<&TextEditValue, (With<LogsFilterInput>, Changed<TextEditValue>)>,
    mut ui_logs: ResMut<UiLogs>,
) {
    for val in &filter_input {
        let trimmed = val.0.trim().to_string();
        if ui_logs.search_query != trimmed {
            ui_logs.search_query = trimmed;
        }
    }
}

pub fn attach_filter_button_observers(
    mut commands: Commands,
    query: Query<Entity, Added<LogFilterButton>>,
) {
    for entity in &query {
        commands.entity(entity).observe(on_filter_button_clicked);
    }
}

pub fn update_filter_button_styles(
    ui_logs: Res<UiLogs>,
    mut buttons: Query<(&LogFilterButton, &mut ButtonVariant)>,
) {
    if !ui_logs.is_changed() {
        return;
    }
    for (btn, mut variant) in &mut buttons {
        let expected = if ui_logs.level_filter == btn.0 {
            ButtonVariant::Primary
        } else {
            ButtonVariant::Ghost
        };
        if *variant != expected {
            *variant = expected;
        }
    }
}

pub fn populate_container_with_logs(
    commands: &mut Commands,
    container_entity: Entity,
    ui_logs: &UiLogs,
    font: &Handle<Font>,
) {
    commands.entity(container_entity).despawn_children();

    let filter = ui_logs.search_query.trim().to_lowercase();
    let level_filter = ui_logs.level_filter;

    let mut logs_to_show: Vec<&LogEntry> = ui_logs
        .logs
        .iter()
        .rev()
        .filter(|entry| {
            if !level_filter.matches(entry.level) {
                return false;
            }
            if !filter.is_empty() {
                let msg = entry.message.to_lowercase();
                if !msg.contains(&filter) {
                    return false;
                }
            }
            true
        })
        .take(200)
        .collect();

    logs_to_show.reverse();

    if logs_to_show.is_empty() {
        commands.spawn((
            Text::new("No matching log messages."),
            TextFont {
                font: font.clone(),
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_MUTED_COLOR.into()),
            Node {
                padding: UiRect::all(Val::Px(tokens::SPACING_MD)),
                ..Default::default()
            },
            ChildOf(container_entity),
        ));
        return;
    }

    for entry in logs_to_show {
        commands.spawn((
            super::core::log_entry_row(entry, font),
            ChildOf(container_entity),
        ));
    }
}

pub fn update_logs_list_ui(
    ui_logs: Res<UiLogs>,
    mut commands: Commands,
    container_query: Query<Entity, With<LogsListContainer>>,
    editor_font: Res<jackdaw_feathers::icons::EditorFont>,
) {
    if !ui_logs.is_changed() {
        return;
    }
    let Ok(container_entity) = container_query.single() else {
        return;
    };
    populate_container_with_logs(&mut commands, container_entity, &ui_logs, &editor_font.0);
}
