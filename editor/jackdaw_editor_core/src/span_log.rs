//! Span and slow-frame timings as text, for timing work with no profiler
//! attached. Set `JACKDAW_SPAN_LOG=<file>` to turn it on; unset, nothing is
//! registered.
//!
//! Each line is one of:
//! - `span <thread> <start_us> <duration_us> <name>`
//! - `frame <start_us> <interval_us> <update_us>`, where `interval_us` runs to
//!   the next frame's start and `update_us` to the end of this frame's `Last`.

use std::fs::File;
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use bevy::log::BoxedLayer;
use bevy::log::tracing::Subscriber;
use bevy::log::tracing::span::Id;
use bevy::log::tracing_subscriber::layer::{Context, Layer};
use bevy::log::tracing_subscriber::registry::LookupSpan;
use bevy::prelude::*;

/// A frame shorter than this is not logged.
const SLOW_FRAME_US: u128 = 16_000;

struct SpanFile {
    file: Mutex<File>,
    epoch: Instant,
}

impl SpanFile {
    fn write_line(&self, line: std::fmt::Arguments) {
        if let Ok(mut file) = self.file.lock() {
            let _ = file.write_fmt(format_args!("{line}\n"));
        }
    }
}

static SPAN_FILE: OnceLock<Option<SpanFile>> = OnceLock::new();

fn span_file() -> Option<&'static SpanFile> {
    SPAN_FILE
        .get_or_init(|| {
            let path = std::env::var_os("JACKDAW_SPAN_LOG")?;
            let file = File::create(path).ok()?;
            Some(SpanFile {
                file: Mutex::new(file),
                epoch: Instant::now(),
            })
        })
        .as_ref()
}

/// The layer for [`bevy::log::LogPlugin::custom_layer`], when asked for.
pub(crate) fn layer() -> Option<BoxedLayer> {
    span_file().map(|file| Box::new(SpanLogLayer { file }) as BoxedLayer)
}

struct SpanLogLayer {
    file: &'static SpanFile,
}

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for SpanLogLayer {
    fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id)
            && span.metadata().target().starts_with("jackdaw")
        {
            span.extensions_mut().replace(Instant::now());
        }
    }

    fn on_exit(&self, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else {
            return;
        };
        let Some(start) = span.extensions_mut().remove::<Instant>() else {
            return;
        };
        self.file.write_line(format_args!(
            "span {:?} {} {} {}",
            std::thread::current().id(),
            start.duration_since(self.file.epoch).as_micros(),
            start.elapsed().as_micros(),
            span.name(),
        ));
    }
}

/// Logs frames slower than [`SLOW_FRAME_US`], when the log is on.
pub(crate) struct SpanLogPlugin;

impl Plugin for SpanLogPlugin {
    fn build(&self, app: &mut App) {
        if span_file().is_some() {
            app.init_resource::<FrameMarks>()
                .add_systems(First, frame_started)
                .add_systems(Last, frame_updated);
        } else if let Some(path) = std::env::var_os("JACKDAW_SPAN_LOG") {
            warn!("Could not open the span log at {}", path.to_string_lossy());
        }
    }
}

#[derive(Resource, Default)]
struct FrameMarks {
    start: Option<Instant>,
    update_us: u128,
}

fn frame_started(mut marks: ResMut<FrameMarks>) {
    let now = Instant::now();
    if let Some(start) = marks.start
        && let Some(file) = span_file()
    {
        let interval_us = now.duration_since(start).as_micros();
        if interval_us >= SLOW_FRAME_US {
            file.write_line(format_args!(
                "frame {} {interval_us} {}",
                start.duration_since(file.epoch).as_micros(),
                marks.update_us,
            ));
        }
    }
    marks.start = Some(now);
}

fn frame_updated(mut marks: ResMut<FrameMarks>) {
    if let Some(start) = marks.start {
        marks.update_us = start.elapsed().as_micros();
    }
}
