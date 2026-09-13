//! The binary twin of a BSN document: round-trips, refusals, and conversion.

use std::path::Path;

use jackdaw_bsn::binary::{self, BinaryError, MAGIC, VERSION};
use jackdaw_bsn::{
    ASSET_HEADER, BsnField, BsnPatch, BsnStructData, BsnStructFields, BsnValue, DocumentError,
    DocumentForm, SceneBsnAst, convert_to_binary, convert_to_text, document_as_text, emit_scene,
    export_binary, leading_comments, parse_bsn_text, read_asset_header, read_document,
    read_document_text, with_asset_header, write_document_text,
};

const SCENE: &str = r#"#Root
bevy_transform::components::transform::Transform { translation: bevy_math::Vec3 { x: 1.0, y: -2.5, z: 0.0 } }
bevy_ecs::hierarchy::Children [
    #Child
    bevy_core::name::Name("a child"),
    :"prefabs/tree.bsn"
    my_game::Tags { names: ["one", "two"], counts: map[(1, true), (2, false)] }
]
"#;

fn round_tripped(text: &str) -> String {
    let ast = parse_bsn_text(text).expect("the document parses");
    let bytes = binary::encode(&ast, None);
    let decoded = binary::decode(&bytes).expect("the binary document reads back");
    emit_scene(&decoded.ast)
}

#[test]
fn a_document_round_trips_text_to_binary_to_text_unchanged() {
    let ast = parse_bsn_text(SCENE).expect("the document parses");

    assert_eq!(round_tripped(SCENE), emit_scene(&ast));
}

#[test]
fn every_value_shape_survives_the_binary_form() {
    let text = r#"my_game::Every {
    float: 1.5,
    int: -9000000000000000000000,
    flag: true,
    text: "words",
    unit: my_game::Kind::One,
    nested: my_game::Inner { a: 1.0 },
    tuple: my_game::Wrap(1, "two"),
    list: [1, 2, 3],
    pairs: map[("a", 1), ("b", 2)]
}
"#;

    assert_eq!(
        round_tripped(text),
        emit_scene(&parse_bsn_text(text).unwrap())
    );
}

#[test]
fn a_string_holding_nul_bytes_survives_the_binary_form() {
    let mut ast = SceneBsnAst::default();
    let node = ast.create_entity_node(vec![BsnPatch::Struct(BsnStructData {
        type_path: "my_game::Label".to_string(),
        fields: BsnStructFields(vec![BsnField {
            name: "text".to_string(),
            value: BsnValue::String("a\0b\u{1}c".to_string()),
        }]),
    })]);
    ast.add_to_roots(node);

    let decoded = binary::decode(&binary::encode(&ast, None)).expect("it reads back");

    assert_eq!(emit_scene(&decoded.ast), emit_scene(&ast));
    assert!(emit_scene(&decoded.ast).contains('\0'));
}

#[test]
fn a_binary_document_keeps_its_header_type() {
    let text = with_asset_header(
        "my_game::content::ItemDef",
        "my_game::content::ItemDef { damage: 3.0 }\n",
    );
    let ast = parse_bsn_text(&text).expect("it parses");

    let bytes = binary::encode(&ast, leading_comments(&text).as_deref());
    let decoded = binary::decode(&bytes).expect("it reads back");

    assert_eq!(
        decoded.preamble.as_deref().and_then(read_asset_header),
        Some("my_game::content::ItemDef".to_string())
    );
}

#[test]
fn a_version_stamp_survives_the_binary_form() {
    let stamped = format!("// jackdaw 0.19.0 | bevy 0.19\n{SCENE}");

    let bytes = binary::encode(
        &parse_bsn_text(&stamped).expect("it parses"),
        leading_comments(&stamped).as_deref(),
    );

    let decoded = binary::decode(&bytes).expect("it reads back");

    assert_eq!(
        document_as_text(&decoded.ast, decoded.preamble.as_deref()),
        format!(
            "// jackdaw 0.19.0 | bevy 0.19\n{}",
            emit_scene(&parse_bsn_text(SCENE).expect("it parses"))
        ),
        "the stamp the editor wrote is still the first line"
    );
}

#[test]
fn converting_a_document_whose_twin_is_already_on_disk_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let text_path = dir.path().join("thing.bsn");
    let binary_path = dir.path().join("thing.bsb");
    write_document_text(&text_path, SCENE).expect("written");
    write_document_text(&binary_path, "my_game::Older\n").expect("written");
    let before = std::fs::read_to_string(&text_path).expect("read back");

    let refused = convert_to_text(&binary_path).err();

    assert!(
        matches!(refused, Some(DocumentError::TwinExists(..))),
        "got {refused:?}"
    );
    assert_eq!(
        std::fs::read_to_string(&text_path).expect("read back"),
        before,
        "the text the repository holds is untouched"
    );
    assert!(binary_path.exists());
    assert!(matches!(
        convert_to_binary(&text_path).err(),
        Some(DocumentError::TwinExists(..))
    ));
}

#[test]
fn a_binary_document_is_known_by_its_first_bytes() {
    let bytes = binary::encode(&parse_bsn_text(SCENE).unwrap(), None);

    assert!(binary::is_binary(&bytes));
    assert!(!binary::is_binary(SCENE.as_bytes()));
}

#[test]
fn no_text_document_can_open_with_the_magic() {
    assert!(std::str::from_utf8(&MAGIC).is_err());

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("odd.bsn");
    std::fs::write(&path, "BSB::Marker\n").expect("the file is written");

    assert_eq!(
        read_document(&path).expect("it reads").form,
        DocumentForm::Text
    );
}

#[test]
fn the_form_is_read_from_the_bytes_rather_than_the_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("misnamed.bsn");
    std::fs::write(&path, binary::encode(&parse_bsn_text(SCENE).unwrap(), None))
        .expect("the file is written");

    let document = read_document(&path).expect("it reads");

    assert_eq!(document.form, DocumentForm::Binary);
    assert_eq!(
        emit_scene(&document.ast),
        emit_scene(&parse_bsn_text(SCENE).unwrap())
    );
}

#[test]
fn a_corrupt_binary_document_is_refused_with_a_clear_error() {
    let bytes = binary::encode(&parse_bsn_text(SCENE).unwrap(), None);

    let mut bad_magic = bytes.clone();
    bad_magic[..4].copy_from_slice(b"XXXX");
    assert!(matches!(
        binary::decode(&bad_magic),
        Err(BinaryError::BadMagic)
    ));

    for cut in 4..bytes.len() {
        let refused = binary::decode(&bytes[..cut]);
        assert!(
            refused.is_err(),
            "{cut} of {} bytes decoded as a whole document",
            bytes.len()
        );
    }
}

#[test]
fn a_document_from_a_later_version_is_refused_by_name() {
    let mut bytes = binary::encode(&parse_bsn_text(SCENE).unwrap(), None);
    bytes[4..6].copy_from_slice(&(VERSION + 1).to_le_bytes());

    let refused = binary::decode(&bytes).err();

    assert!(
        matches!(refused, Some(BinaryError::UnsupportedVersion(version)) if version == VERSION + 1),
        "got {refused:?}"
    );
}

#[test]
fn a_length_longer_than_the_document_is_refused_rather_than_reserved() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&u64::MAX.to_le_bytes());

    let refused = binary::decode(&bytes).err();
    assert!(
        matches!(refused, Some(BinaryError::Truncated)),
        "got {refused:?}"
    );

    bytes.truncate(7);
    bytes.extend_from_slice(&u64::MAX.to_le_bytes());
    assert!(matches!(
        binary::decode(&bytes).err(),
        Some(BinaryError::Truncated)
    ));
}

#[test]
fn a_deeply_nested_document_round_trips() {
    let depth = 60;
    let mut text = String::new();
    for _ in 0..depth {
        text.push_str("bevy_ecs::hierarchy::Children [\n");
    }
    text.push_str("my_game::Leaf\n");
    for _ in 0..depth {
        text.push_str("]\n");
    }

    assert_eq!(
        round_tripped(&text),
        emit_scene(&parse_bsn_text(&text).unwrap())
    );
}

#[test]
fn a_scene_saved_as_binary_loads_identically() {
    let dir = tempfile::tempdir().expect("tempdir");
    let text_path = dir.path().join("scene.bsn");
    let binary_path = dir.path().join("scene.bsb");
    write_document_text(&text_path, SCENE).expect("the text scene is written");
    write_document_text(&binary_path, SCENE).expect("the binary scene is written");

    let from_text = read_document(&text_path).expect("the text scene reads");
    let from_binary = read_document(&binary_path).expect("the binary scene reads");

    assert_eq!(from_binary.form, DocumentForm::Binary);
    assert_eq!(emit_scene(&from_binary.ast), emit_scene(&from_text.ast));
}

#[test]
fn an_asset_file_saved_as_binary_keeps_its_header_type() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("torch.bsb");
    let text = with_asset_header(
        "my_game::content::ItemDef",
        "#torch\nmy_game::content::ItemDef { damage: 3.0 }\n",
    );
    write_document_text(&path, &text).expect("the asset is written");

    let document = read_document(&path).expect("the asset reads");

    assert_eq!(
        document.header(),
        Some("my_game::content::ItemDef".to_string())
    );
    assert_eq!(
        jackdaw_bsn::asset_file_type(&path).as_deref(),
        Some("my_game::content::ItemDef")
    );
    assert!(read_document_text(&path).unwrap().starts_with(ASSET_HEADER));
}

#[test]
fn a_document_resolves_to_its_binary_twin_when_the_text_file_is_absent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let text_path = dir.path().join("materials/grass.bsn");
    std::fs::create_dir_all(text_path.parent().unwrap()).expect("the folder is made");
    write_document_text(&text_path, "my_game::Material { rough: 1.0 }\n").expect("written");

    assert_eq!(
        jackdaw_bsn::existing_form(&text_path).as_deref(),
        Some(text_path.as_path())
    );

    let binary_path = convert_to_binary(&text_path).expect("it converts");

    assert_eq!(binary_path, dir.path().join("materials/grass.bsb"));
    assert_eq!(
        jackdaw_bsn::existing_form(&text_path).as_deref(),
        Some(binary_path.as_path())
    );
}

#[test]
fn a_walk_lists_a_document_held_in_both_forms_once_as_its_text_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let both = dir.path().join("both.bsn");
    write_document_text(&both, "my_game::Marker\n").expect("written");
    write_document_text(&dir.path().join("both.bsb"), "my_game::Marker\n").expect("written");
    let lone = dir.path().join("lone.bsb");
    write_document_text(&lone, "my_game::Marker\n").expect("written");

    let found = jackdaw_bsn::walk_document_files(dir.path());

    assert_eq!(found, vec![both, lone]);
}

#[test]
fn converting_in_place_removes_the_other_form() {
    let dir = tempfile::tempdir().expect("tempdir");
    let text_path = dir.path().join("thing.bsn");
    write_document_text(&text_path, SCENE).expect("written");

    let binary_path = convert_to_binary(&text_path).expect("it converts to binary");
    assert!(!text_path.exists());
    assert!(binary_path.exists());

    let back = convert_to_text(&binary_path).expect("it converts back");
    assert_eq!(back, text_path);
    assert!(!binary_path.exists());
    assert_eq!(
        std::fs::read_to_string(&back).unwrap(),
        emit_scene(&parse_bsn_text(SCENE).unwrap())
    );
}

#[test]
fn converting_to_binary_and_back_leaves_the_bytes_the_repository_held() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("torch.bsn");
    let held = format!(
        "// jackdaw 0.19.0 | bevy 0.19\n{}",
        with_asset_header(
            "my_game::content::ItemDef",
            &emit_scene(
                &parse_bsn_text("#torch\nmy_game::content::ItemDef { damage: 3.0 }\n")
                    .expect("it parses")
            ),
        )
    );
    std::fs::write(&path, &held).expect("written");

    let binary = convert_to_binary(&path).expect("it converts to binary");
    let back = convert_to_text(&binary).expect("it converts back");

    assert_eq!(back, path);
    assert_eq!(
        std::fs::read_to_string(&back).expect("read back"),
        held,
        "the stamp, the header and the body all came back"
    );
}

#[test]
fn a_failed_conversion_leaves_the_file_it_was_given() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("broken.bsn");
    std::fs::write(&path, "my_game::Broken {{{").expect("written");

    let refused = convert_to_binary(&path).err();

    assert!(
        matches!(refused, Some(DocumentError::Parse(..))),
        "got {refused:?}"
    );
    assert!(path.exists());
    assert!(!dir.path().join("broken.bsb").exists());
}

#[test]
fn export_binary_converts_a_tree_and_leaves_other_files_alone() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = dir.path().join("assets");
    std::fs::create_dir_all(source.join("materials")).expect("the folder is made");
    write_document_text(&source.join("scene.bsn"), SCENE).expect("written");
    write_document_text(
        &source.join("materials/grass.bsn"),
        "my_game::Material { rough: 1.0 }\n",
    )
    .expect("written");
    std::fs::write(source.join("materials/grass.png"), [1, 2, 3]).expect("written");

    let destination = dir.path().join("export");
    let converted = export_binary(&source, &destination).expect("the tree exports");

    assert_eq!(converted, 2);
    assert!(destination.join("scene.bsb").exists());
    assert!(!destination.join("scene.bsn").exists());
    assert_eq!(
        std::fs::read(destination.join("materials/grass.png")).unwrap(),
        vec![1, 2, 3]
    );
    assert!(source.join("scene.bsn").exists());

    let exported = read_document(&destination.join("scene.bsb")).expect("the export reads");
    assert_eq!(
        emit_scene(&exported.ast),
        emit_scene(&parse_bsn_text(SCENE).unwrap())
    );
}

#[test]
fn export_binary_refuses_a_destination_inside_the_tree_it_reads() {
    let dir = tempfile::tempdir().expect("tempdir");
    let source = dir.path().join("assets");
    std::fs::create_dir_all(source.join("materials")).expect("the folder is made");
    write_document_text(&source.join("scene.bsn"), SCENE).expect("written");

    for destination in [
        source.clone(),
        source.join("export"),
        source.join("materials/../export"),
    ] {
        let refused = export_binary(&source, &destination).err();
        assert!(
            matches!(refused, Some(DocumentError::DestinationInsideSource(..))),
            "{} was accepted: got {refused:?}",
            destination.display()
        );
    }

    assert!(source.join("scene.bsn").exists());
    assert!(!source.join("scene.bsb").exists());
}

#[test]
fn a_file_that_is_not_a_document_is_refused_by_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("picture.bsn");
    std::fs::write(&path, [0xff, 0xfe, 0xfd]).expect("written");

    let refused = read_document(&path).err();

    assert!(
        matches!(refused, Some(DocumentError::NotUtf8(_))),
        "got {refused:?}"
    );
    assert!(read_document(Path::new("nowhere/at/all.bsn")).is_err());
}
