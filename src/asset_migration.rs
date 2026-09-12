//! Bringing a project written before assets were files at paths up to date.
//!
//! Everything here is already what the editor does as it reads such a
//! project: a bare name resolves through the file whose stem it is, a file
//! with no header is known by its first root, a sidecar older than version 8
//! reads its slots as paths. The operator writes that reading back to disk,
//! once, so the files spell what the editor understands.

use std::path::{Path, PathBuf};

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::operator::report_to_caller;
use jackdaw_bsn::{BsnPatch, BsnValue, SceneBsnAst};
use path_slash::PathExt as _;

use crate::asset_index::{AssetIndex, AssetValue, assets_dir, rescan_asset_index};

/// What one run changed, and what it could not.
#[derive(Default)]
struct MigrationReport {
    rewritten: Vec<String>,
    headers: Vec<String>,
    catalog: Vec<String>,
    sidecars: Vec<String>,
    unresolved: Vec<String>,
}

impl MigrationReport {
    fn did_nothing(&self) -> bool {
        self.rewritten.is_empty()
            && self.headers.is_empty()
            && self.catalog.is_empty()
            && self.sidecars.is_empty()
    }

    fn lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for (what, items) in [
            ("rewrote", &self.rewritten),
            ("wrote out", &self.catalog),
            ("added a header to", &self.headers),
            ("re-encoded", &self.sidecars),
        ] {
            if !items.is_empty() {
                lines.push(format!("{what} {}: {}", items.len(), items.join(", ")));
            }
        }
        if !self.unresolved.is_empty() {
            lines.push(format!(
                "left alone {}: {}",
                self.unresolved.len(),
                self.unresolved.join(", ")
            ));
        }
        if lines.is_empty() {
            lines.insert(0, "nothing to migrate".to_string());
        }
        lines
    }
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ProjectMigrateAssetReferencesOp>();
}

/// Rewrite a project's references, headers and sidecars as the current model
/// spells them.
#[operator(
    id = "project.migrate_asset_references",
    label = "Migrate Asset References",
    description = "Rewrite every name reference as a path, write the catalog's entries out as \
                   files, head every asset file and re-encode every terrain sidecar. Writes \
                   over the project's files and cannot be undone; save what is open first.",
    allows_undo = false
)]
pub fn project_migrate_asset_references(
    _: In<OperatorParameters>,
    mut commands: Commands,
) -> OperatorResult {
    commands.queue(migrate);
    OperatorResult::Finished
}

/// The edit that would be written over by a migration, if there is one.
fn open_edit(world: &World) -> Option<&'static str> {
    if crate::scene_io::is_scene_dirty(world) {
        return Some("the open scene has edits that are not saved");
    }
    if crate::definition_assets::open_card_has_unsaved_edits(world) {
        return Some("an open asset has edits that are not saved");
    }
    if world
        .get_resource::<crate::animation::graph_doc::AnimationGraphDoc>()
        .is_some_and(|doc| doc.dirty)
    {
        return Some("the open animation graph has edits that are not saved");
    }
    None
}

fn migrate(world: &mut World) {
    let Some(assets) = assets_dir(world) else {
        report_to_caller(world, "migrate: no project is open");
        return;
    };
    if !assets.is_dir() {
        report_to_caller(
            world,
            format!("migrate: {} is not a directory", assets.display()),
        );
        return;
    }
    if let Some(reason) = open_edit(world) {
        report_to_caller(world, format!("migrate: refused, {reason}"));
        return;
    }

    let mut report = MigrationReport::default();
    rescan_asset_index(world);
    write_catalog_out(world, &assets, &mut report);
    rescan_asset_index(world);
    head_asset_files(world, &assets, &mut report);
    rewrite_documents(world, &assets, &mut report);
    reencode_sidecars(world, &assets, &mut report);
    rescan_asset_index(world);

    let did_nothing = report.did_nothing();
    for line in report.lines() {
        report_to_caller(world, format!("migrate: {line}"));
    }
    if !did_nothing {
        report_to_caller(world, "migrate: undo does not reach these files");
    }
}

// -- The catalog ------------------------------------------------------------

/// The folder a catalog entry's kind keeps its files in.
fn folder_for_kind(kind: &AssetKind) -> &str {
    if kind.kind == crate::definition_assets::MATERIAL_KIND {
        crate::material_assets::MATERIALS_DIR
    } else {
        kind.kind.as_str()
    }
}

/// Write every entry `catalog.bsn` holds out as a file of its own, and leave
/// the file holding none.
///
/// The file is read again rather than taken from [`AssetCatalog`](crate::asset_catalog::AssetCatalog), which also
/// holds what the editor named for itself this run.
fn write_catalog_out(world: &mut World, assets: &Path, report: &mut MigrationReport) {
    let catalog_file = assets.join("catalog.bsn");
    let Ok(text) = std::fs::read_to_string(&catalog_file) else {
        return;
    };
    let entries = match jackdaw_bsn::load_bsn_assets(world, &text) {
        Ok(entries) => entries,
        Err(err) => {
            report.unresolved.push(format!("catalog.bsn ({err})"));
            return;
        }
    };
    if entries.is_empty() {
        return;
    }

    for entry in entries {
        let kind = kind_of_handle(world, entry.handle.type_id());
        let Some(kind) = kind else {
            report.unresolved.push(format!(
                "catalog entry '{}' holds a type no kind claims",
                entry.name
            ));
            continue;
        };
        let name = crate::material_assets::sanitize_material_name(&entry.name);
        let relative = PathBuf::from(folder_for_kind(&kind)).join(format!("{name}.bsn"));
        let file = assets.join(&relative);
        if file.exists() {
            report.unresolved.push(format!(
                "catalog entry '{}' already has {}, which wins the name",
                entry.name,
                relative.to_slash_lossy()
            ));
            continue;
        }
        let value = AssetValue::Handle(entry.handle.clone());
        match crate::definition_assets::write_asset_file(world, &name, &value, &file) {
            Ok(_) => {
                crate::asset_index::index_written(world, &file, &kind, value);
                note_written(world, &file);
                world
                    .resource_mut::<crate::asset_catalog::AssetCatalog>()
                    .inline_materials
                    .remove(&entry.name);
                report.catalog.push(relative.to_slash_lossy().into_owned());
            }
            Err(err) => report
                .unresolved
                .push(format!("catalog entry '{}' ({err})", entry.name)),
        }
    }

    if report.catalog.is_empty() {
        return;
    }
    let emptied = "// no catalog entries\n";
    match crate::scene_io::save::write_atomic(&catalog_file, emptied.as_bytes()) {
        Ok(()) => {
            note_written(world, &catalog_file);
            report.catalog.push("catalog.bsn".to_string());
        }
        Err(err) => report.unresolved.push(format!("catalog.bsn ({err})")),
    }
}

/// Record a file the migration itself wrote, so neither the asset index nor
/// the open document reads it back as a change made behind the editor's back.
fn note_written(world: &mut World, file: &Path) {
    crate::asset_index::note_written(world, file);
    if let Ok(bytes) = std::fs::read(file) {
        crate::scenes::external_watch::note_known_content(world, file, &bytes);
    }
}

/// The kind that claims the type an asset id holds.
fn kind_of_handle(world: &World, type_id: std::any::TypeId) -> Option<AssetKind> {
    let registry = world.get_resource::<AppTypeRegistry>()?.read();
    let type_path = registry.get(type_id)?.type_info().type_path();
    world
        .get_resource::<AssetKinds>()?
        .by_type_path(type_path)
        .cloned()
}

// -- Headers ----------------------------------------------------------------

/// Put the header naming its type on every asset file that carries none.
fn head_asset_files(world: &mut World, assets: &Path, report: &mut MigrationReport) {
    let headless: Vec<(PathBuf, String)> = world
        .resource::<AssetIndex>()
        .iter()
        .map(|entry| (entry.path.clone(), entry.type_path.clone()))
        .filter(|(path, _)| {
            std::fs::read_to_string(assets.join(path))
                .is_ok_and(|text| jackdaw_bsn::read_asset_header(&text).is_none())
        })
        .collect();

    for (path, type_path) in headless {
        let file = assets.join(&path);
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let headed = with_header_below_the_stamp(&text, &type_path);
        match crate::scene_io::save::write_atomic(&file, headed.as_bytes()) {
            Ok(()) => {
                note_written(world, &file);
                report.headers.push(path.to_slash_lossy().into_owned());
            }
            Err(err) => report
                .unresolved
                .push(format!("{} ({err})", path.to_slash_lossy())),
        }
    }
}

/// Put the header after the version stamp, where a file the editor writes
/// carries it, and at the top of a file that has no stamp.
fn with_header_below_the_stamp(text: &str, type_path: &str) -> String {
    let Some(rest) = text
        .split_once('\n')
        .filter(|(first, _)| crate::scene_io::stamp::read_stamp(&format!("{first}\n")).is_some())
    else {
        return jackdaw_bsn::with_asset_header(type_path, text);
    };
    format!(
        "{}\n{}",
        rest.0,
        jackdaw_bsn::with_asset_header(type_path, rest.1)
    )
}

// -- References -------------------------------------------------------------

/// Rewrite every name a scene or a prefab spells for an asset as the path of
/// the file that answers to it.
fn rewrite_documents(world: &mut World, assets: &Path, report: &mut MigrationReport) {
    let documents: Vec<PathBuf> = jackdaw_bsn::walk_document_files(assets)
        .into_iter()
        .filter_map(|file| Some(file.strip_prefix(assets).ok()?.to_path_buf()))
        .filter(|path| !crate::asset_index::is_catalog_file(path))
        .filter(|path| world.resource::<AssetIndex>().get(path).is_none())
        .collect();

    for path in documents {
        let file = assets.join(&path);
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let Ok(ast) = jackdaw_bsn::parse_bsn_text(&text) else {
            continue;
        };
        let named = name_references(world, &ast);
        if named.is_empty() {
            continue;
        }
        let mut rewritten = text.clone();
        let mut count = 0usize;
        for (field, reference) in named {
            match resolve_reference(world, &reference) {
                Resolved::Path(target) if target == reference => {}
                Resolved::Path(target) => {
                    let was = format!("{field}: \"{reference}\"");
                    let now = format!("{field}: \"{target}\"");
                    if rewritten.contains(&was) {
                        rewritten = rewritten.replace(&was, &now);
                        count += 1;
                    }
                }
                Resolved::Ambiguous(first, second) => report.unresolved.push(format!(
                    "{} names '{reference}', which is both {first} and {second}",
                    path.to_slash_lossy()
                )),
                Resolved::Missing => report.unresolved.push(format!(
                    "{} names '{reference}', which no file holds",
                    path.to_slash_lossy()
                )),
                Resolved::AlreadyAPath => {}
            }
        }
        if count == 0 {
            continue;
        }
        match crate::scene_io::save::write_atomic(&file, rewritten.as_bytes()) {
            Ok(()) => {
                note_written(world, &file);
                report
                    .rewritten
                    .push(format!("{} ({count})", path.to_slash_lossy()));
            }
            Err(err) => report
                .unresolved
                .push(format!("{} ({err})", path.to_slash_lossy())),
        }
    }
}

/// What a document spells for an asset, as the field holding it and the text
/// it holds.
///
/// Only a field whose type names an asset by path counts, so a string that
/// happens to read like a name is left alone; a string under the `@` sigil
/// counts wherever it sits, since nothing else spells one. Nested values name
/// their own type, so a reference inside a brush face is found the same way as
/// one on a component.
fn name_references(world: &World, ast: &SceneBsnAst) -> Vec<(String, String)> {
    let Some(registry) = world.get_resource::<AppTypeRegistry>() else {
        return Vec::new();
    };
    let registry = registry.read();
    let mut found: Vec<(String, String)> = Vec::new();
    for patch in ast.world.iter_entities() {
        let Some(BsnPatch::Struct(data)) = patch.get::<BsnPatch>() else {
            continue;
        };
        scan_struct(&registry, data, &mut found);
    }
    found
}

/// Read the fields of one struct, by the type it names.
fn scan_struct(
    registry: &bevy::reflect::TypeRegistry,
    data: &jackdaw_bsn::BsnStructData,
    found: &mut Vec<(String, String)>,
) {
    let info = registry
        .get_with_type_path(&data.type_path)
        .map(bevy::reflect::TypeRegistration::type_info);
    for field in &data.fields.0 {
        let field_type = match info {
            Some(bevy::reflect::TypeInfo::Struct(info)) => info
                .field(&field.name)
                .map(bevy::reflect::NamedField::type_id),
            _ => None,
        };
        scan_value(registry, field_type, &field.name, &field.value, found);
    }
}

/// The type the items of a list or an array hold.
fn item_type(
    registry: &bevy::reflect::TypeRegistry,
    type_id: std::any::TypeId,
) -> Option<std::any::TypeId> {
    match registry.get(type_id)?.type_info() {
        bevy::reflect::TypeInfo::List(info) => Some(info.item_ty().id()),
        bevy::reflect::TypeInfo::Array(info) => Some(info.item_ty().id()),
        _ => None,
    }
}

fn scan_value(
    registry: &bevy::reflect::TypeRegistry,
    field_type: Option<std::any::TypeId>,
    field_name: &str,
    value: &BsnValue,
    found: &mut Vec<(String, String)>,
) {
    match value {
        BsnValue::String(reference) => {
            let takes_path = field_type
                .is_some_and(|type_id| crate::typed_values::takes_asset_path(registry, type_id));
            let pair = (field_name.to_string(), reference.clone());
            if (takes_path || reference.starts_with('@'))
                && !reference.is_empty()
                && !found.contains(&pair)
            {
                found.push(pair);
            }
        }
        BsnValue::Struct(data) => scan_struct(registry, data, found),
        BsnValue::TupleStruct(data) => {
            for value in &data.values {
                scan_value(registry, None, field_name, value, found);
            }
        }
        BsnValue::List(values) => {
            let item = field_type.and_then(|type_id| item_type(registry, type_id));
            for value in values {
                scan_value(registry, item, field_name, value, found);
            }
        }
        BsnValue::Map(pairs) => {
            for (_, value) in pairs {
                scan_value(registry, field_type, field_name, value, found);
            }
        }
        _ => {}
    }
}

enum Resolved {
    Path(String),
    Ambiguous(String, String),
    Missing,
    AlreadyAPath,
}

/// The file a reference stands for: the one whose stem it is, when exactly one
/// document carries that stem and it is an asset file.
fn resolve_reference(world: &World, reference: &str) -> Resolved {
    let bare = reference.trim_start_matches('@');
    if reference.starts_with('#') {
        return Resolved::AlreadyAPath;
    }
    if !reference.starts_with('@') && bare.contains('/') {
        return Resolved::AlreadyAPath;
    }
    let index = world.resource::<AssetIndex>();
    let stem = jackdaw_bsn::asset_stem(bare);
    if let Some(path) = index.stems().unique(stem) {
        if index.get(path).is_some() {
            return Resolved::Path(path.to_slash_lossy().into_owned());
        }
        return Resolved::Missing;
    }
    match index.stems().shared(stem) {
        Some((first, second)) => Resolved::Ambiguous(
            first.to_slash_lossy().into_owned(),
            second.to_slash_lossy().into_owned(),
        ),
        None => Resolved::Missing,
    }
}

// -- Sidecars ---------------------------------------------------------------

/// Read every terrain sidecar and write it back at the current version, with
/// its material slots spelling paths.
fn reencode_sidecars(world: &mut World, assets: &Path, report: &mut MigrationReport) {
    let sidecars =
        jackdaw_bsn::walk_files_with_extensions(assets, &[jackdaw_terrain::sidecar::EXTENSION]);
    for file in sidecars {
        let Ok(bytes) = std::fs::read(&file) else {
            continue;
        };
        let relative = file
            .strip_prefix(assets)
            .unwrap_or(&file)
            .to_slash_lossy()
            .into_owned();
        if version_of(&bytes) == Some(jackdaw_terrain::sidecar::VERSION_8) {
            continue;
        }
        let loaded = match jackdaw_terrain::sidecar::load_from(&bytes, Some(assets)) {
            Ok(loaded) => loaded,
            Err(err) => {
                report.unresolved.push(format!("{relative} ({err})"));
                continue;
            }
        };
        let mut data = loaded.data;
        for (slot, name) in loaded.kept_names {
            match resolve_reference(world, &name) {
                Resolved::Path(path) => data.materials[slot].material = path,
                Resolved::Ambiguous(first, second) => report.unresolved.push(format!(
                    "{relative} slot {slot} names '{name}', which is both {first} and {second}"
                )),
                _ => report.unresolved.push(format!(
                    "{relative} slot {slot} names '{name}', which no file holds"
                )),
            }
        }
        let encoded = match jackdaw_terrain::sidecar::save(&data) {
            Ok(encoded) => encoded,
            Err(err) => {
                report.unresolved.push(format!("{relative} ({err})"));
                continue;
            }
        };
        match crate::scene_io::save::write_atomic(&file, &encoded) {
            Ok(()) => {
                note_written(world, &file);
                report.sidecars.push(relative);
            }
            Err(err) => report.unresolved.push(format!("{relative} ({err})")),
        }
    }
}

/// The format version a sidecar's header states.
fn version_of(bytes: &[u8]) -> Option<u16> {
    let magic = jackdaw_terrain::sidecar::MAGIC.len();
    let stated = bytes.get(magic..magic + 2)?;
    Some(u16::from_le_bytes([stated[0], stated[1]]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::AssetPlugin;
    use jackdaw_api_internal::operator::OperatorReports;

    const STANDARD_MATERIAL: &str = "bevy_pbr::pbr_material::StandardMaterial";

    /// A component naming a material, of the shape a scene spells one in.
    #[derive(Component, Reflect, Default)]
    #[reflect(Component, Default)]
    struct Signpost {
        board: Handle<StandardMaterial>,
    }

    fn migration_app() -> (App, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut app = App::new();
        app.add_plugins((bevy::app::TaskPoolPlugin::default(), AssetPlugin::default()));
        app.init_asset::<Image>();
        app.init_asset::<StandardMaterial>();
        app.register_asset_reflect::<Image>();
        app.register_asset_reflect::<StandardMaterial>();
        app.register_type::<StandardMaterial>();
        app.register_type::<Signpost>();
        app.insert_resource(crate::project::ProjectRoot {
            root: tmp.path().to_path_buf(),
            config: crate::project::ProjectConfig::default(),
        });
        app.init_resource::<crate::asset_index::AssetIndex>();
        app.init_resource::<crate::asset_files::AssetKindCache>();
        app.init_resource::<crate::asset_catalog::AssetCatalog>();
        app.init_resource::<crate::material_assets::MaterialRegistry>();
        app.init_resource::<jackdaw_commands::CommandHistory>();
        app.init_resource::<crate::scene_io::SceneDirtyState>();
        app.init_resource::<AssetKinds>();
        app.world_mut()
            .resource_mut::<AssetKinds>()
            .register(AssetKind::compiled(
                crate::definition_assets::MATERIAL_KIND,
                "Material",
                STANDARD_MATERIAL,
            ));
        std::fs::create_dir_all(tmp.path().join("assets")).expect("the assets directory is made");
        (app, tmp)
    }

    fn write(tmp: &tempfile::TempDir, relative: &str, text: &str) {
        let path = tmp.path().join("assets").join(relative);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("the folder is made");
        std::fs::write(path, text).expect("the file is written");
    }

    fn read(tmp: &tempfile::TempDir, relative: &str) -> String {
        std::fs::read_to_string(tmp.path().join("assets").join(relative)).expect("the file is read")
    }

    /// A material file of the shape written before headers, so the migration
    /// has something to head.
    fn headerless_material(tmp: &tempfile::TempDir, relative: &str, name: &str) {
        write(
            tmp,
            relative,
            &format!("#{name}\n{STANDARD_MATERIAL} {{ metallic: 0.25 }}\n"),
        );
    }

    fn run(app: &mut App) -> Vec<String> {
        app.world_mut()
            .get_resource_or_init::<OperatorReports>()
            .0
            .clear();
        crate::asset_index::rescan_asset_index(app.world_mut());
        migrate(app.world_mut());
        app.world().resource::<OperatorReports>().0.to_vec()
    }

    fn said(reports: &[String], fragment: &str) -> bool {
        reports.iter().any(|line| line.contains(fragment))
    }

    #[test]
    fn a_name_a_scene_spells_becomes_the_path_of_the_file_it_names() {
        let (mut app, tmp) = migration_app();
        headerless_material(&tmp, "materials/slate.material.bsn", "slate");
        write(
            &tmp,
            "zones/hedgerow.bsn",
            "#Sign\njackdaw::asset_migration::tests::Signpost { board: \"@slate\" }\n",
        );

        run(&mut app);

        assert!(
            read(&tmp, "zones/hedgerow.bsn").contains("materials/slate.material.bsn"),
            "got {}",
            read(&tmp, "zones/hedgerow.bsn")
        );
    }

    #[test]
    fn a_name_a_prefab_spells_becomes_a_path_too() {
        let (mut app, tmp) = migration_app();
        headerless_material(&tmp, "materials/slate.material.bsn", "slate");
        write(
            &tmp,
            "prefabs/post.bsn",
            "#post\njackdaw::prefab::components::Prefab\n\
             jackdaw::asset_migration::tests::Signpost { board: \"slate\" }\n",
        );

        run(&mut app);

        assert!(read(&tmp, "prefabs/post.bsn").contains("materials/slate.material.bsn"));
    }

    #[test]
    fn a_file_with_no_header_is_given_one() {
        let (mut app, tmp) = migration_app();
        headerless_material(&tmp, "materials/slate.material.bsn", "slate");

        run(&mut app);

        let text = read(&tmp, "materials/slate.material.bsn");
        assert_eq!(
            jackdaw_bsn::read_asset_header(&text).as_deref(),
            Some(STANDARD_MATERIAL),
            "got {text}"
        );
        assert!(text.contains("metallic: 0.25"), "got {text}");
    }

    #[test]
    fn a_catalog_entry_is_written_out_as_a_file_of_its_own() {
        let (mut app, tmp) = migration_app();
        write(
            &tmp,
            "catalog.bsn",
            &format!("#steel\n{STANDARD_MATERIAL} {{ metallic: 0.75 }}\n"),
        );

        run(&mut app);

        let written = read(&tmp, "materials/steel.bsn");
        assert!(written.contains("metallic"), "got {written}");
        assert!(
            !read(&tmp, "catalog.bsn").contains("steel"),
            "the catalog keeps nothing it has written out"
        );
    }

    /// A catalog entry whose name a file already carries would be written over
    /// the file; it is left where it is and named in the report.
    #[test]
    fn a_catalog_entry_that_would_land_on_a_file_is_left_alone() {
        let (mut app, tmp) = migration_app();
        headerless_material(&tmp, "materials/steel.bsn", "steel");
        write(
            &tmp,
            "catalog.bsn",
            &format!("#steel\n{STANDARD_MATERIAL} {{ metallic: 0.75 }}\n"),
        );

        let reports = run(&mut app);

        assert!(
            said(&reports, "already has materials/steel.bsn"),
            "got {reports:?}"
        );
        assert!(read(&tmp, "materials/steel.bsn").contains("metallic: 0.25"));
        assert!(read(&tmp, "catalog.bsn").contains("steel"));
    }

    #[test]
    fn a_name_two_files_carry_is_left_alone_and_both_are_named() {
        let (mut app, tmp) = migration_app();
        headerless_material(&tmp, "materials/slate.material.bsn", "slate");
        headerless_material(&tmp, "zones/slate.bsn", "slate");
        write(
            &tmp,
            "zones/hedgerow.bsn",
            "#Sign\njackdaw::asset_migration::tests::Signpost { board: \"@slate\" }\n",
        );

        let reports = run(&mut app);

        assert!(
            said(&reports, "materials/slate.material.bsn")
                && said(&reports, "zones/slate.bsn")
                && said(&reports, "which is both"),
            "got {reports:?}"
        );
        assert!(
            read(&tmp, "zones/hedgerow.bsn").contains("\"@slate\""),
            "an ambiguous name is left as the file spells it"
        );
    }

    #[test]
    fn a_name_no_file_carries_is_left_alone_and_reported() {
        let (mut app, tmp) = migration_app();
        write(
            &tmp,
            "zones/hedgerow.bsn",
            "#Sign\njackdaw::asset_migration::tests::Signpost { board: \"@ephemeral\" }\n",
        );

        let reports = run(&mut app);

        assert!(
            said(&reports, "'@ephemeral', which no file holds"),
            "got {reports:?}"
        );
        assert!(read(&tmp, "zones/hedgerow.bsn").contains("\"@ephemeral\""));
    }

    /// Version 7 and version 8 lay the same bytes out, so a version-7 file is
    /// this build's bytes under the older version word.
    fn write_version_7_sidecar(tmp: &tempfile::TempDir, relative: &str, slot: &str) {
        let data = jackdaw_terrain::sidecar::RegionTerrainData {
            materials: vec![jackdaw_terrain::sidecar::TerrainMaterialSlot::new(slot)],
            ..Default::default()
        };
        let mut bytes = jackdaw_terrain::sidecar::encode_regions(&data).expect("it encodes");
        bytes[8..10].copy_from_slice(&jackdaw_terrain::sidecar::VERSION_7.to_le_bytes());
        let path = tmp.path().join("assets").join(relative);
        std::fs::create_dir_all(path.parent().expect("a parent")).expect("the folder is made");
        std::fs::write(path, bytes).expect("the sidecar is written");
    }

    fn sidecar_slot(tmp: &tempfile::TempDir, relative: &str) -> (u16, String) {
        let bytes = std::fs::read(tmp.path().join("assets").join(relative)).expect("read");
        let data = jackdaw_terrain::sidecar::load(&bytes).expect("it loads");
        (
            version_of(&bytes).expect("a version"),
            data.materials[0].material.clone(),
        )
    }

    #[test]
    fn a_sidecar_written_before_paths_is_re_encoded_with_its_slots_as_paths() {
        let (mut app, tmp) = migration_app();
        headerless_material(&tmp, "materials/slate.material.bsn", "slate");
        write_version_7_sidecar(&tmp, "zones/hedgerow.terrain-0.jdterrain", "slate");

        run(&mut app);

        assert_eq!(
            sidecar_slot(&tmp, "zones/hedgerow.terrain-0.jdterrain"),
            (
                jackdaw_terrain::sidecar::VERSION_8,
                "materials/slate.material.bsn".to_string()
            )
        );
    }

    #[test]
    fn a_sidecar_slot_naming_no_file_keeps_its_name_and_is_reported() {
        let (mut app, tmp) = migration_app();
        write_version_7_sidecar(&tmp, "zones/hedgerow.terrain-0.jdterrain", "gone");

        let reports = run(&mut app);

        assert!(
            said(&reports, "slot 0 names 'gone', which no file holds"),
            "got {reports:?}"
        );
        assert_eq!(
            sidecar_slot(&tmp, "zones/hedgerow.terrain-0.jdterrain").1,
            "gone",
            "a name with no file behind it still draws what it drew"
        );
    }

    /// A file at the top of the assets is already at the path its name spells,
    /// so a reference to it is finished and a run that rewrote it would never
    /// settle.
    #[test]
    fn a_reference_that_already_spells_its_file_is_left_as_it_is() {
        let (mut app, tmp) = migration_app();
        headerless_material(&tmp, "slate.bsn", "slate");
        write(
            &tmp,
            "zones/hedgerow.bsn",
            "#Sign\njackdaw::asset_migration::tests::Signpost { board: \"slate.bsn\" }\n",
        );

        run(&mut app);
        let again = run(&mut app);

        assert!(said(&again, "nothing to migrate"), "got {again:?}");
    }

    #[test]
    fn a_second_run_has_nothing_to_do() {
        let (mut app, tmp) = migration_app();
        headerless_material(&tmp, "materials/slate.material.bsn", "slate");
        write(
            &tmp,
            "zones/hedgerow.bsn",
            "#Sign\njackdaw::asset_migration::tests::Signpost { board: \"@slate\" }\n",
        );

        run(&mut app);
        let again = run(&mut app);

        assert!(said(&again, "nothing to migrate"), "got {again:?}");
    }

    #[test]
    fn a_project_with_unsaved_edits_open_is_refused() {
        let (mut app, tmp) = migration_app();
        headerless_material(&tmp, "materials/slate.material.bsn", "slate");
        app.world_mut()
            .resource_mut::<crate::scene_io::SceneDirtyState>()
            .undo_len_at_save = 1;

        let reports = run(&mut app);

        assert!(said(&reports, "refused"), "got {reports:?}");
        assert!(
            jackdaw_bsn::read_asset_header(&read(&tmp, "materials/slate.material.bsn")).is_none(),
            "nothing is written while an edit is open"
        );
    }

    #[test]
    fn a_project_with_no_assets_directory_is_said_so_rather_than_walked() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let (mut app, _held) = migration_app();
        app.world_mut()
            .insert_resource(crate::project::ProjectRoot {
                root: tmp.path().to_path_buf(),
                config: crate::project::ProjectConfig::default(),
            });

        let reports = run(&mut app);

        assert!(said(&reports, "is not a directory"), "got {reports:?}");
    }
}
