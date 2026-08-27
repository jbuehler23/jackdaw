//! What the applier says when it drops a list element.
//!
//! A list the document spells wrong loses elements rather than taking the
//! editor down, and the log line is the only trace the user gets. The element
//! type alone does not say which list on which component lost what, so these
//! pin the attribution on both drop branches: the list type, the position, and
//! the element type.

use std::sync::Mutex;

use bevy::reflect::{PartialReflect, Reflect, TypeRegistry};
use jackdaw_bsn::{BsnField, BsnStructData, BsnStructFields, BsnValue, bsn_value_to_reflect};

static RECORDS: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Capture;

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        RECORDS.lock().unwrap().push(record.args().to_string());
    }

    fn flush(&self) {}
}

/// Install the capture and take whatever the run logs. The logger is one per
/// process and `cargo test` threads these tests, so the turnstile keeps one
/// run from draining another's lines.
fn logged(run: impl FnOnce()) -> Vec<String> {
    static INSTALL: std::sync::Once = std::sync::Once::new();
    static TURN: Mutex<()> = Mutex::new(());

    INSTALL.call_once(|| {
        log::set_logger(&Capture).expect("no other logger in this test binary");
        log::set_max_level(log::LevelFilter::Warn);
    });
    let _turn = TURN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    RECORDS.lock().unwrap().clear();
    run();
    RECORDS.lock().unwrap().drain(..).collect()
}

#[derive(Reflect, Debug, PartialEq)]
enum Choice {
    Sized { size: f32 },
    Plain,
}

fn registry() -> TypeRegistry {
    let mut registry = TypeRegistry::new();
    registry.register::<Choice>();
    registry.register::<Vec<Choice>>();
    registry.register::<f32>();
    registry
}

fn convert(items: BsnValue, registry: &TypeRegistry) -> Vec<Choice> {
    let reflected = bsn_value_to_reflect(
        &items,
        std::any::TypeId::of::<Vec<Choice>>(),
        registry,
        None,
    )
    .expect("a list of a registered enum converts");
    let mut target: Vec<Choice> = Vec::new();
    target.apply(reflected.as_ref());
    target
}

/// The element converts to something reflected, but the concrete type refuses
/// it: a variant field of the wrong shape.
#[test]
fn an_element_that_does_not_fit_names_the_list_and_the_position() {
    let registry = registry();
    let items = BsnValue::List(vec![
        BsnValue::Type("Choice::Plain".into()),
        BsnValue::Struct(BsnStructData {
            type_path: "Choice::Sized".into(),
            fields: BsnStructFields(vec![BsnField {
                name: "size".into(),
                value: BsnValue::String("wide".into()),
            }]),
        }),
    ]);

    let mut kept = Vec::new();
    let lines = logged(|| kept = convert(items, &registry));

    assert_eq!(kept, vec![Choice::Plain], "the element that fits survives");
    let line = lines
        .iter()
        .find(|line| line.contains("does not fit"))
        .unwrap_or_else(|| panic!("the drop is reported: {lines:?}"));
    assert!(
        line.contains("alloc::vec::Vec<") && line.contains("[1]") && line.contains("Choice"),
        "the line names the list, the position and the element type: {line}",
    );
}

/// The element does not convert at all: a variant name no enum carries.
#[test]
fn an_element_that_converts_to_nothing_is_reported_too() {
    let registry = registry();
    let items = BsnValue::List(vec![
        BsnValue::Type("Choice::Absent".into()),
        BsnValue::Type("Choice::Plain".into()),
    ]);

    let mut kept = Vec::new();
    let lines = logged(|| kept = convert(items, &registry));

    assert_eq!(kept, vec![Choice::Plain], "the element that fits survives");
    let line = lines
        .iter()
        .find(|line| line.contains("is not a"))
        .unwrap_or_else(|| panic!("the silent drop is reported: {lines:?}"));
    assert!(
        line.contains("alloc::vec::Vec<") && line.contains("[0]") && line.contains("Choice"),
        "the line names the list, the position and the element type: {line}",
    );
}
