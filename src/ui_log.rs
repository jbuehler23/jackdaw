use bevy::prelude::*;
use jackdaw_feathers::icons::Icon;
use std::sync::{Mutex, OnceLock};
use tracing::Subscriber;
use tracing_subscriber::Layer;

use jackdaw_feathers::{
    button::{ButtonProps, ButtonVariant, button},
    dialog::{DialogChildrenSlot, OpenDialogEvent},
    text_edit::{TextEditProps, TextEditValue, text_edit},
    tokens,
};

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub level: tracing::Level,
    pub message: String,
    pub timestamp: String,
}

use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

pub static LOG_SENDER: OnceLock<SyncSender<LogEntry>> = OnceLock::new();
pub static LOG_RECEIVER: OnceLock<Mutex<Option<Receiver<LogEntry>>>> = OnceLock::new();

pub fn get_sender() -> SyncSender<LogEntry> {
    LOG_SENDER
        .get_or_init(|| {
            let (tx, rx) = sync_channel(10000);
            let _ = LOG_RECEIVER.set(Mutex::new(Some(rx)));
            tx
        })
        .clone()
}

pub fn take_receiver() -> Option<Receiver<LogEntry>> {
    let _ = get_sender(); // Ensure channel is initialized
    LOG_RECEIVER.get()?.lock().ok()?.take()
}

#[derive(Resource)]
pub struct UiLogReceiver(pub Mutex<Receiver<LogEntry>>);

pub fn init_logging() {
    use tracing_subscriber::prelude::*;
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        tracing_subscriber::EnvFilter::new("info,wgpu=error,naga=warn,bevy_app=info")
    });
    let ui_log_layer = UiLogLayer;
    let fmt_layer = tracing_subscriber::fmt::layer().with_writer(std::io::stderr);
    let _ = tracing_subscriber::registry()
        .with(env_filter)
        .with(ui_log_layer)
        .with(fmt_layer)
        .try_init();
}

fn get_current_time_string() -> String {
    let now = std::time::SystemTime::now();
    if let Ok(duration) = now.duration_since(std::time::SystemTime::UNIX_EPOCH) {
        // Offset by 2 hours for UTC+2 (user timezone)
        let secs = duration.as_secs() + 7200;
        let hours = (secs / 3600) % 24;
        let minutes = (secs / 60) % 60;
        let seconds = secs % 60;
        format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
    } else {
        "00:00:00".to_string()
    }
}

struct MessageVisitor {
    message: String,
}

impl tracing::field::Visit for MessageVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" && self.message.is_empty() {
            let formatted = format!("{value:?}");
            // Strip leading/trailing quotes if present
            if formatted.starts_with('"') && formatted.ends_with('"') && formatted.len() >= 2 {
                self.message = formatted[1..formatted.len() - 1].to_string();
            } else {
                self.message = formatted;
            }
        }
    }
}

pub struct UiLogLayer;

impl<S: Subscriber> Layer<S> for UiLogLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut visitor = MessageVisitor {
            message: String::new(),
        };
        event.record(&mut visitor);
        if !visitor.message.is_empty() {
            let timestamp = get_current_time_string();
            let entry = LogEntry {
                level: *event.metadata().level(),
                message: visitor.message,
                timestamp,
            };
            let sender = get_sender();
            let _ = sender.try_send(entry);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogLevelFilter {
    #[default]
    All,
    Info,
    Warn,
    Error,
}

impl LogLevelFilter {
    pub fn matches(&self, level: tracing::Level) -> bool {
        match self {
            Self::All => true,
            Self::Info => level == tracing::Level::INFO,
            Self::Warn => level == tracing::Level::WARN,
            Self::Error => level == tracing::Level::ERROR,
        }
    }
}

#[derive(Resource, Default)]
pub struct UiLogs {
    pub logs: Vec<LogEntry>,
    pub search_query: String,
    pub level_filter: LogLevelFilter,
    pub latest_log: Option<LogEntry>,
    pub latest_log_timer: Option<f32>,
}

#[derive(Resource, Default)]
pub struct LogsDialogOpen;

#[derive(Component)]
pub struct StatusBarLogArea;

#[derive(Component)]
pub struct StatusBarLogIcon;

#[derive(Component)]
pub struct StatusBarLogText;

#[derive(Component)]
pub struct LogsDialogMarker;

#[derive(Component)]
pub struct LogsDialogPopulated;

#[derive(Component)]
pub struct LogsFilterInput;

#[derive(Component)]
pub struct LogFilterButton(pub LogLevelFilter);

#[derive(Component)]
pub struct LogsListContainer;

pub struct UiLogPlugin;

impl Plugin for UiLogPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<UiLogs>();
        if let Some(rx) = take_receiver() {
            app.insert_resource(UiLogReceiver(Mutex::new(rx)));
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
        app.add_observer(
            |trigger: On<Add, LogsListContainer>,
             mut commands: Commands,
             ui_logs: Res<UiLogs>,
             editor_font: Res<jackdaw_feathers::icons::EditorFont>| {
                populate_container_with_logs(
                    &mut commands,
                    trigger.entity,
                    &ui_logs,
                    &editor_font.0,
                );
            },
        )
        .add_observer(
            |_: On<Remove, LogsDialogMarker>,
             mut commands: Commands,
             mut ui_logs: ResMut<UiLogs>| {
                commands.remove_resource::<LogsDialogOpen>();
                ui_logs.search_query = String::new();
                ui_logs.level_filter = LogLevelFilter::All;
            },
        );
    }
}

fn poll_global_logs(receiver: Res<UiLogReceiver>, mut ui_logs: ResMut<UiLogs>) {
    let Ok(rx) = receiver.0.lock() else {
        return;
    };

    let entries: Vec<LogEntry> = rx.try_iter().collect();
    if entries.is_empty() {
        return;
    }

    if let Some(latest) = entries.iter().rfind(|e| {
        e.level == tracing::Level::INFO
            || e.level == tracing::Level::WARN
            || e.level == tracing::Level::ERROR
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

fn tick_latest_log(time: Res<Time>, mut ui_logs: ResMut<UiLogs>) {
    if let Some(ref mut timer) = ui_logs.latest_log_timer {
        *timer -= time.delta_secs();
        if *timer <= 0.0 {
            ui_logs.latest_log_timer = None;
            ui_logs.latest_log = None;
        }
    }
}

fn update_status_bar_ui(
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
        let (icon, color) = match latest.level {
            tracing::Level::ERROR => (Icon::CircleAlert, tokens::COLOR_ERROR),
            tracing::Level::WARN => (Icon::TriangleAlert, tokens::COLOR_WARN),
            tracing::Level::INFO => (Icon::Info, tokens::COLOR_INFO),
            _ => (Icon::Terminal, tokens::TEXT_SECONDARY),
        };

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

fn update_log_area_background(
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

fn attach_log_area_click_observer(
    mut commands: Commands,
    query: Query<Entity, Added<StatusBarLogArea>>,
) {
    for entity in &query {
        commands.entity(entity).observe(on_log_area_clicked);
    }
}

fn on_log_area_clicked(_click: On<Pointer<Click>>, mut commands: Commands) {
    commands.trigger(
        OpenDialogEvent::new("System Logs", "Close")
            .without_cancel()
            .with_max_width(Val::Px(600.0))
            .without_content_padding(),
    );

    commands.insert_resource(LogsDialogOpen);
}

fn populate_logs_dialog(
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
            .spawn(Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                row_gap: Val::Px(tokens::SPACING_MD),
                padding: UiRect::all(Val::Px(tokens::SPACING_LG)),
                ..Default::default()
            })
            .id();

        commands.entity(slot_entity).add_child(wrapper);

        let filter_row = commands
            .spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_MD),
                width: Val::Percent(100.0),
                ..Default::default()
            })
            .id();

        commands.entity(wrapper).add_child(filter_row);

        let mut filter_props = TextEditProps::default()
            .with_placeholder("Search logs...")
            .allow_empty();

        filter_props.grow = true;

        let filter_input_wrapper = commands
            .spawn((
                Node {
                    flex_grow: 1.0,
                    ..Default::default()
                },
                children![(LogsFilterInput, text_edit(filter_props),)],
            ))
            .id();

        commands.entity(filter_row).add_child(filter_input_wrapper);

        let categories = [
            (LogLevelFilter::All, "All"),
            (LogLevelFilter::Info, "Info"),
            (LogLevelFilter::Warn, "Warn"),
            (LogLevelFilter::Error, "Error"),
        ];

        for (filter, label) in categories {
            let btn = commands
                .spawn((
                    LogFilterButton(filter),
                    button(ButtonProps::new(label).with_variant(ButtonVariant::Ghost)),
                ))
                .id();

            commands.entity(filter_row).add_child(btn);
        }

        let scroll_wrapper = commands
            .spawn(Node {
                position_type: PositionType::Relative,
                width: Val::Percent(100.0),
                height: Val::Px(400.0),
                ..Default::default()
            })
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
            ))
            .id();

        let scrollbar = commands
            .spawn(jackdaw_feathers::scroll::scrollbar(scroll))
            .id();

        commands.entity(scroll_wrapper).add_child(scroll);
        commands.entity(scroll_wrapper).add_child(scrollbar);

        commands.entity(wrapper).add_child(scroll_wrapper);
    }
}

fn apply_logs_filter(
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

fn attach_filter_button_observers(
    mut commands: Commands,
    query: Query<Entity, Added<LogFilterButton>>,
) {
    for entity in &query {
        commands.entity(entity).observe(on_filter_button_clicked);
    }
}

fn on_filter_button_clicked(
    click: On<Pointer<Click>>,
    query: Query<&LogFilterButton>,
    mut ui_logs: ResMut<UiLogs>,
) {
    if let Ok(btn) = query.get(click.entity)
        && ui_logs.level_filter != btn.0
    {
        ui_logs.level_filter = btn.0;
    }
}

fn update_filter_button_styles(
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

fn populate_container_with_logs(
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
        commands.entity(container_entity).with_child((
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
        ));
        return;
    }

    for entry in logs_to_show {
        let (level_str, level_color) = match entry.level {
            tracing::Level::ERROR => ("ERROR", tokens::COLOR_ERROR),
            tracing::Level::WARN => ("WARN", tokens::COLOR_WARN),
            tracing::Level::INFO => ("INFO", tokens::COLOR_INFO),
            tracing::Level::DEBUG => ("DEBUG", tokens::COLOR_DEBUG),
            tracing::Level::TRACE => ("TRACE", tokens::COLOR_TRACE),
        };

        let row = commands
            .spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::FlexStart,
                column_gap: Val::Px(tokens::SPACING_SM),
                width: Val::Percent(100.0),
                padding: UiRect::axes(Val::Px(tokens::SPACING_SM), Val::Px(4.0)),
                ..Default::default()
            })
            .id();

        let timestamp_node = commands
            .spawn((
                Text::new(entry.timestamp.clone()),
                TextFont {
                    font: font.clone(),
                    font_size: tokens::FONT_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_MUTED_COLOR.into()),
                Node {
                    flex_shrink: 0.0,
                    ..Default::default()
                },
            ))
            .id();

        let level_node = commands
            .spawn((
                Text::new(level_str),
                TextFont {
                    font: font.clone(),
                    font_size: tokens::FONT_SM,
                    weight: FontWeight::BOLD,
                    ..Default::default()
                },
                TextColor(level_color),
                Node {
                    width: Val::Px(50.0),
                    flex_shrink: 0.0,
                    ..Default::default()
                },
            ))
            .id();

        let message_node = commands
            .spawn((
                Text::new(entry.message.clone()),
                TextFont {
                    font: font.clone(),
                    font_size: tokens::FONT_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_PRIMARY),
                Node {
                    flex_grow: 1.0,
                    ..Default::default()
                },
            ))
            .id();

        commands
            .entity(row)
            .add_children(&[timestamp_node, level_node, message_node]);

        commands.entity(container_entity).add_child(row);
    }
}

fn update_logs_list_ui(
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
