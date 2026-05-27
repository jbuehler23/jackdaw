use bevy::prelude::*;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Mutex, OnceLock};
use tracing::Subscriber;
use tracing_subscriber::Layer;

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub level: tracing::Level,
    pub message: String,
    pub timestamp: String,
}

static LOG_SENDER: OnceLock<SyncSender<LogEntry>> = OnceLock::new();
static LOG_RECEIVER: OnceLock<Mutex<Option<Receiver<LogEntry>>>> = OnceLock::new();

fn get_sender() -> SyncSender<LogEntry> {
    LOG_SENDER
        .get_or_init(|| {
            let (tx, rx) = sync_channel(10000);
            let _ = LOG_RECEIVER.set(Mutex::new(Some(rx)));
            tx
        })
        .clone()
}

pub fn take_receiver() -> Option<Receiver<LogEntry>> {
    // Ensure Init
    let _ = get_sender();
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
        if field.name() != "message" || !self.message.is_empty() {
            return;
        }

        let formatted = format!("{value:?}");
        self.message = formatted
            .strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or(&formatted)
            .to_string();
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

pub trait LogLevelExt {
    fn icon_and_color(&self) -> (jackdaw_feathers::icons::Icon, Color);
}

impl LogLevelExt for tracing::Level {
    fn icon_and_color(&self) -> (jackdaw_feathers::icons::Icon, Color) {
        use jackdaw_feathers::icons::Icon;
        use jackdaw_feathers::tokens;
        match *self {
            tracing::Level::ERROR => (Icon::CircleAlert, tokens::COLOR_ERROR),
            tracing::Level::WARN => (Icon::TriangleAlert, tokens::COLOR_WARN),
            tracing::Level::INFO => (Icon::Info, tokens::COLOR_INFO),
            _ => (Icon::Terminal, tokens::TEXT_SECONDARY),
        }
    }
}

pub fn log_entry_row(entry: &LogEntry, font: &Handle<Font>) -> impl Bundle {
    use jackdaw_feathers::tokens;
    let (level_str, level_color) = match entry.level {
        tracing::Level::ERROR => ("ERROR", tokens::COLOR_ERROR),
        tracing::Level::WARN => ("WARN", tokens::COLOR_WARN),
        tracing::Level::INFO => ("INFO", tokens::COLOR_INFO),
        tracing::Level::DEBUG => ("DEBUG", tokens::COLOR_DEBUG),
        tracing::Level::TRACE => ("TRACE", tokens::COLOR_TRACE),
    };

    (
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexStart,
            column_gap: Val::Px(tokens::SPACING_SM),
            width: Val::Percent(100.0),
            padding: UiRect::axes(Val::Px(tokens::SPACING_SM), Val::Px(4.0)),
            ..Default::default()
        },
        children![
            (
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
            ),
            (
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
            ),
            (
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
            ),
        ],
    )
}

pub fn logs_filter_row() -> impl Bundle {
    use jackdaw_feathers::{
        button::{ButtonProps, ButtonVariant, button},
        text_edit::{TextEditProps, text_edit},
        tokens,
    };

    let mut filter_props = TextEditProps::default()
        .with_placeholder("Search logs...")
        .allow_empty();
    filter_props.grow = true;

    (
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(tokens::SPACING_MD),
            width: Val::Percent(100.0),
            ..Default::default()
        },
        children![
            (
                Node {
                    flex_grow: 1.0,
                    ..Default::default()
                },
                children![(LogsFilterInput, text_edit(filter_props),)],
            ),
            (
                LogFilterButton(LogLevelFilter::All),
                button(ButtonProps::new("All").with_variant(ButtonVariant::Ghost)),
            ),
            (
                LogFilterButton(LogLevelFilter::Info),
                button(ButtonProps::new("Info").with_variant(ButtonVariant::Ghost)),
            ),
            (
                LogFilterButton(LogLevelFilter::Warn),
                button(ButtonProps::new("Warn").with_variant(ButtonVariant::Ghost)),
            ),
            (
                LogFilterButton(LogLevelFilter::Error),
                button(ButtonProps::new("Error").with_variant(ButtonVariant::Ghost)),
            ),
        ],
    )
}
