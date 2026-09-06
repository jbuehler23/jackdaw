//! What the applier does with a patch it cannot honour.
//!
//! A hand-edited document is free to name a variant the enum does not declare
//! or a type nothing registered. Neither takes the editor down, and neither
//! passes without a log line naming what was dropped.

use std::sync::Mutex;

use bevy::ecs::name::Name;
use bevy::ecs::reflect::AppTypeRegistry;
use bevy::ecs::world::World;
use bevy::prelude::{Component, ReflectComponent, ReflectDefault};
use bevy::reflect::{Reflect, TypePath};
use bevy::transform::components::Transform;

use jackdaw_bsn::{apply_dirty_ast_patches, parse_bsn_text, spawn_from_ast};

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

/// Installs the capture and takes whatever the run logs. The logger is one per
/// process, so the turnstile keeps one threaded test from draining another's
/// lines.
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

#[derive(Component, Reflect, Default, Debug, PartialEq)]
#[reflect(Component, Default)]
enum Mode {
    #[default]
    Idle,
    Running,
    Loud(f32),
    Sized {
        w: f32,
        h: f32,
    },
}

/// A component holding an enum in a field, where a variant reaches reflection
/// as a value rather than as a patch of its own.
#[derive(Component, Reflect, Default, Debug, PartialEq)]
#[reflect(Component, Default)]
struct Holder {
    mode: Mode,
}

/// An enum inside a newtype, where the write goes through the tuple-struct
/// patch loop rather than the struct one.
#[derive(Component, Reflect, Default, Debug, PartialEq)]
#[reflect(Component, Default)]
struct Wrapper(Mode);

/// A component whose default holds a non-empty list, which a refused write
/// leaves intact.
#[derive(Component, Reflect, Debug, PartialEq)]
#[reflect(Component, Default)]
struct Listy {
    items: Vec<f32>,
}

impl Default for Listy {
    fn default() -> Self {
        Self {
            items: vec![1.0, 2.0, 3.0],
        }
    }
}

/// A braced value that is not a list, for offering to the list field.
#[derive(Reflect, Default, Debug, PartialEq)]
struct Pair {
    x: f32,
}

/// Loads a document into a fresh world and returns it with whatever the applier
/// logged.
fn loaded(text: &str) -> (World, Vec<String>) {
    let mut world = World::new();
    let registry = AppTypeRegistry::default();
    {
        let mut registry = registry.write();
        registry.register::<Mode>();
        registry.register::<Holder>();
        registry.register::<Wrapper>();
        registry.register::<Listy>();
        registry.register::<Pair>();
        registry.register::<Vec<f32>>();
        registry.register::<f32>();
        registry.register::<Transform>();
    }
    world.insert_resource(registry);
    let ast = parse_bsn_text(text).expect("bsn should parse");
    world.insert_resource(ast);

    let records = logged(|| {
        spawn_from_ast(&mut world);
        apply_dirty_ast_patches(&mut world);
    });
    (world, records)
}

fn root(world: &mut World) -> bevy::ecs::entity::Entity {
    world
        .query::<(bevy::ecs::entity::Entity, &Name)>()
        .iter(world)
        .find(|(_, name)| name.as_str() == "Root")
        .map(|(entity, _)| entity)
        .expect("the root spawned")
}

/// A variant the enum does not declare reaches `PartialReflect::apply`, which
/// panics rather than refusing. The document opens anyway, with the bad patch
/// skipped and named in the log.
#[test]
fn a_variant_the_enum_does_not_have_is_refused_rather_than_fatal() {
    let text = format!(
        "\
#Root
{mode}::Ultraviolet
bevy_transform::components::transform::Transform {{
    translation: glam::Vec3 {{ x: 5.0 }},
}}
",
        mode = Mode::type_path(),
    );

    let (mut world, records) = loaded(&text);
    let root = root(&mut world);

    assert!(
        world.get::<Mode>(root).is_none(),
        "the patch naming a variant that does not exist is skipped, not guessed at",
    );
    let transform = world
        .get::<Transform>(root)
        .expect("the sibling patch still applies");
    assert!((transform.translation.x - 5.0).abs() < f32::EPSILON);

    let line = records
        .iter()
        .find(|line| line.contains("Ultraviolet"))
        .unwrap_or_else(|| panic!("a line names the missing variant; got {records:?}"));
    assert!(line.contains(Mode::type_path()), "the line names the type");
    assert!(
        line.contains("Idle") && line.contains("Running"),
        "the line says which variants the type does have: {line}",
    );
}

/// The other half: a variant the enum does declare still lands.
#[test]
fn a_variant_the_enum_does_have_still_applies() {
    let text = format!("#Root\n{mode}::Running\n", mode = Mode::type_path());

    let (mut world, _) = loaded(&text);
    let root = root(&mut world);

    assert_eq!(world.get::<Mode>(root), Some(&Mode::Running));
}

/// A tuple-struct patch naming an unregistered type is reported rather than
/// skipped in silence.
#[test]
fn a_tuple_struct_patch_on_an_unregistered_type_says_so() {
    let text = "#Root\nsome::other::crate_name::Caption(\"Start\")\n";

    let (_, records) = loaded(text);

    let line = records
        .iter()
        .find(|line| line.contains("some::other::crate_name::Caption"))
        .unwrap_or_else(|| panic!("a line names the unregistered type; got {records:?}"));
    assert!(
        line.contains("registry"),
        "the line says the type is not registered: {line}",
    );
}

/// A variant named bare, or half-filled, is the shape a hand-authored document
/// carries, and each such spelling reaches the panicking `apply`.
#[test]
fn a_variant_named_without_the_fields_it_carries_is_refused() {
    let text = format!(
        "\
#Root
{mode}::Loud
bevy_transform::components::transform::Transform {{
    translation: glam::Vec3 {{ x: 5.0 }},
}}
",
        mode = Mode::type_path(),
    );

    let (mut world, records) = loaded(&text);
    let root = root(&mut world);

    assert!(
        world.get::<Mode>(root).is_none(),
        "a tuple variant with no value for its field is skipped",
    );
    assert!(
        world.get::<Transform>(root).is_some(),
        "and the sibling patch still applies",
    );
    assert!(
        records.iter().any(|line| line.contains(Mode::type_path())),
        "a line names the type it could not apply: {records:?}",
    );
}

/// The same through the other patch shape: a struct variant the document gives
/// only some of the fields of.
#[test]
fn a_struct_variant_missing_a_field_is_refused() {
    let text = format!(
        "\
#Root
{mode}::Sized {{ w: 3.0 }}
bevy_transform::components::transform::Transform {{
    translation: glam::Vec3 {{ x: 5.0 }},
}}
",
        mode = Mode::type_path(),
    );

    let (mut world, records) = loaded(&text);
    let root = root(&mut world);

    assert_eq!(
        world.get::<Mode>(root),
        None,
        "half a variant is not a variant",
    );
    assert!(
        world.get::<Transform>(root).is_some(),
        "and the sibling patch still applies",
    );
    assert!(
        records.iter().any(|line| line.contains(Mode::type_path())),
        "a line names the type it could not apply: {records:?}",
    );
}

/// The third route, where the variant is a field's value rather than a patch
/// of its own.
#[test]
fn a_field_value_naming_an_impossible_variant_is_refused() {
    let text = format!(
        "\
#Root
{holder} {{ mode: {mode}::Loud }}
bevy_transform::components::transform::Transform {{
    translation: glam::Vec3 {{ x: 5.0 }},
}}
",
        holder = Holder::type_path(),
        mode = Mode::type_path(),
    );

    let (mut world, records) = loaded(&text);
    let root = root(&mut world);

    assert_eq!(
        world.get::<Holder>(root),
        Some(&Holder { mode: Mode::Idle }),
        "the component still lands; the field it could not read keeps its default",
    );
    assert!(
        world.get::<Transform>(root).is_some(),
        "and the sibling patch still applies",
    );
    assert!(
        records.iter().any(|line| line.contains(Mode::type_path())),
        "a line names the type it could not apply: {records:?}",
    );
}

/// The same shape inside a newtype, where the write lands in the tuple-struct
/// patch loop and reaches reflection's last `apply`.
#[test]
fn a_half_filled_variant_inside_a_newtype_is_refused() {
    let text = format!(
        "\
#Root
{wrapper}({mode}::Sized {{ w: 3.0 }})
bevy_transform::components::transform::Transform {{
    translation: glam::Vec3 {{ x: 5.0 }},
}}
",
        wrapper = Wrapper::type_path(),
        mode = Mode::type_path(),
    );

    let (mut world, records) = loaded(&text);
    let root = root(&mut world);

    assert_eq!(
        world.get::<Wrapper>(root),
        Some(&Wrapper(Mode::Idle)),
        "the newtype still lands; the value it could not read keeps its default",
    );
    assert!(
        world.get::<Transform>(root).is_some(),
        "and the sibling patch still applies",
    );
    let line = records
        .iter()
        .find(|line| line.contains("Sized"))
        .unwrap_or_else(|| panic!("a line names the variant; got {records:?}"));
    assert!(
        line.contains(Mode::type_path()),
        "and blames the document's own type, not the field it did not fill: {line}",
    );
}

/// The same shape on a struct component's enum field, which takes the merge
/// path.
#[test]
fn a_half_filled_variant_on_a_struct_field_is_refused_out_loud() {
    let text = format!(
        "\
#Root
{holder} {{ mode: {mode}::Sized {{ w: 3.0 }} }}
",
        holder = Holder::type_path(),
        mode = Mode::type_path(),
    );

    let (mut world, records) = loaded(&text);
    let root = root(&mut world);

    assert_eq!(
        world.get::<Holder>(root),
        Some(&Holder { mode: Mode::Idle }),
        "the field keeps its default rather than half a variant",
    );
    assert!(
        records.iter().any(|line| line.contains("Sized")),
        "and says so: a value dropped in silence is the bug this file is about; \
         got {records:?}",
    );
}

/// The other half of the merge path: a variant the document fills completely is
/// a value the field takes.
#[test]
fn a_whole_variant_on_a_struct_field_lands() {
    let text = format!(
        "\
#Root
{holder} {{ mode: {mode}::Sized {{ w: 3.0, h: 4.0 }} }}
",
        holder = Holder::type_path(),
        mode = Mode::type_path(),
    );

    let (mut world, _) = loaded(&text);
    let root = root(&mut world);

    assert_eq!(
        world.get::<Holder>(root),
        Some(&Holder {
            mode: Mode::Sized { w: 3.0, h: 4.0 },
        }),
    );
}

/// Replacing a list clears it first, so a refused write restores what the
/// field was holding rather than leaving it empty.
#[test]
fn a_refused_write_leaves_a_list_holding_what_it_had() {
    let text = format!(
        "\
#Root
{listy} {{ items: {pair} {{ x: 1.0 }} }}
",
        listy = Listy::type_path(),
        pair = Pair::type_path(),
    );

    let (mut world, records) = loaded(&text);
    let root = root(&mut world);

    assert_eq!(
        world.get::<Listy>(root),
        Some(&Listy::default()),
        "the list keeps the three it had rather than being emptied for a value it refused",
    );
    assert!(
        records.iter().any(|line| line.contains("Vec<f32>")),
        "and the refusal is on the record: {records:?}",
    );
}

/// A list the document fills is replaced outright rather than merged over the
/// default.
#[test]
fn an_authored_list_replaces_the_one_it_found() {
    let text = format!(
        "\
#Root
{listy} {{ items: [9.0] }}
",
        listy = Listy::type_path(),
    );

    let (mut world, _) = loaded(&text);
    let root = root(&mut world);

    assert_eq!(
        world.get::<Listy>(root),
        Some(&Listy { items: vec![9.0] }),
        "one element in, one element out: no tail left from the default",
    );
}

/// A data variant is written `Enum::Variant(value)`, which reaches the
/// tuple-patch arm and lands.
#[test]
fn a_tuple_variant_patch_applies() {
    let text = format!("#Root\n{mode}::Loud(2.5)\n", mode = Mode::type_path());

    let (mut world, _) = loaded(&text);
    let root = root(&mut world);

    assert_eq!(world.get::<Mode>(root), Some(&Mode::Loud(2.5)));
}

/// A value of the wrong type converts to something of the wrong type rather
/// than to nothing, so only `FromReflect` can tell and the insert would
/// otherwise fall through to `apply` and die.
#[test]
fn a_tuple_variant_value_of_the_wrong_type_is_refused_rather_than_fatal() {
    let text = format!(
        "\
#Root
{mode}::Loud(\"nope\")
bevy_transform::components::transform::Transform {{
    translation: glam::Vec3 {{ x: 5.0 }},
}}
",
        mode = Mode::type_path(),
    );

    let (mut world, records) = loaded(&text);
    let root = root(&mut world);

    assert!(
        world.get::<Mode>(root).is_none(),
        "a value the variant cannot take is skipped, not forced in",
    );
    let transform = world
        .get::<Transform>(root)
        .expect("the sibling patch still applies");
    assert!((transform.translation.x - 5.0).abs() < f32::EPSILON);

    assert!(
        records.iter().any(|line| line.contains("Loud")),
        "a line names the variant that was dropped; got {records:?}",
    );
}

/// The variant exists but takes no values in parentheses, which nothing further
/// down could attribute to the right spelling.
#[test]
fn a_unit_variant_written_with_a_value_says_which_shape_it_takes() {
    let text = format!("#Root\n{mode}::Idle(1.0)\n", mode = Mode::type_path());

    let (mut world, records) = loaded(&text);
    let root = root(&mut world);

    assert!(
        world.get::<Mode>(root).is_none(),
        "a unit variant handed a value is skipped",
    );
    let line = records
        .iter()
        .find(|line| line.contains("Idle"))
        .unwrap_or_else(|| panic!("a line names the variant; got {records:?}"));
    assert!(
        line.contains("not a tuple variant"),
        "the line says what is wrong with the spelling: {line}",
    );
    assert!(line.contains(Mode::type_path()), "the line names the type");
}
