//! Suggestions on request: Suggest lists blocks and objects of the sheet
//! without changing the tileset, and each row shows, dismisses, lays out or
//! adds its suggestion only when pressed.

use crate::tileset_dock::{control, opened, press, settle, texts, view};
use crate::tileset_edit_ops::{call, entries, on_disk};
use crate::tileset_ops::{SHEET, import, project_app, warnings};

use bevy::prelude::*;
use jackdaw::tileset_dock::{BitsOverlay, DockAction, DockControl, TilesetDockViews};
use jackdaw::tileset_layout::LayoutPopover;
use jackdaw::tileset_suggest::{SuggestionLabel, SuggestionOutline};
use jackdaw_tilemap::{LayoutTemplate, Suggestion, SuggestionKind};

const GRASS: u8 = 1;
const DIRT: u8 = 2;
const DIRT_BLOCK: UVec2 = UVec2::new(10, 0);

/// The overworld sheet imported with grass, dirt and water made from their
/// plain tiles.
fn named() -> (App, tempfile::TempDir) {
    let (mut app, dir) = project_app();
    import(&mut app, &[("paths", SHEET)]);
    for (name, icon) in [("grass", "0:0"), ("dirt", "0:38"), ("water", "0:305")] {
        call(
            &mut app,
            "tileset.terrain.add",
            &[("name", name), ("icon", icon)],
        );
    }
    assert!(warnings(&app).is_empty(), "{:?}", warnings(&app));
    settle(&mut app);
    (app, dir)
}

fn listed(app: &App) -> Vec<Suggestion> {
    view(app).suggestions.unwrap_or_default()
}

fn rows(app: &mut App, root: Entity) -> usize {
    (app.world_mut().query::<&DockControl>().iter(app.world()))
        .filter(|c| c.root == root && matches!(c.action, DockAction::SuggestionDismiss(_)))
        .count()
}

fn is_block(s: &Suggestion) -> bool {
    matches!(s.kind, SuggestionKind::Block { .. })
}

fn position(found: &[Suggestion], hit: impl Fn(&Suggestion) -> bool) -> usize {
    found.iter().position(hit).expect("a suggestion")
}

fn outlines(app: &mut App, root: Entity) -> Vec<SuggestionOutline> {
    (app.world_mut()
        .query::<&SuggestionOutline>()
        .iter(app.world()))
    .filter(|o| o.root == root)
    .copied()
    .collect()
}

#[test]
fn nothing_is_suggested_on_import_or_open() {
    let (mut app, _dir) = named();
    let (_, root) = opened(&mut app);
    assert!(view(&app).suggestions.is_none());
    assert_eq!(rows(&mut app, root), 0);
    control(&mut app, root, DockAction::Suggest);
}

#[test]
fn suggest_lists_blocks_and_objects_without_changing_the_tileset() {
    let (mut app, dir) = named();
    let (_, root) = opened(&mut app);
    let (before, history) = (on_disk(&dir), entries(&app));
    press(&mut app, root, DockAction::Suggest);
    let found = listed(&app);
    let dirt = position(&found, |s| s.origin == DIRT_BLOCK);
    let SuggestionKind::Block {
        inner,
        outer,
        layouts,
    } = &found[dirt].kind
    else {
        panic!("a block: {:?}", found[dirt]);
    };
    assert_eq!((*inner, *outer), (Some(DIRT), Some(GRASS)));
    assert_eq!(layouts[0].template, LayoutTemplate::Minimal3x3Inner);
    assert!(
        found
            .iter()
            .any(|s| matches!(s.kind, SuggestionKind::Object { .. }))
    );
    let objects = found.iter().filter(|s| !is_block(s)).count();
    assert_eq!(
        rows(&mut app, root),
        found.len() - objects,
        "objects start folded"
    );
    assert_eq!(on_disk(&dir), before, "the file is unchanged");
    assert_eq!(entries(&app), history, "no history entry");

    call(&mut app, "tileset.suggest", &[]);
    assert_eq!(listed(&app), found, "the operator lists the same");
    assert_eq!(entries(&app), history);
}

#[test]
fn show_outlines_a_suggestion_and_dismiss_drops_it_for_the_session() {
    let (mut app, _dir) = named();
    let (_, root) = opened(&mut app);
    press(&mut app, root, DockAction::Suggest);
    assert!(outlines(&mut app, root).is_empty());
    let found = listed(&app);
    let dirt = position(&found, |s| s.origin == DIRT_BLOCK);
    press(&mut app, root, DockAction::SuggestionShow(dirt));
    let shown = outlines(&mut app, root);
    assert_eq!(shown.len(), 1);
    assert_eq!(
        (shown[0].origin, shown[0].size),
        (DIRT_BLOCK, found[dirt].size)
    );

    press(&mut app, root, DockAction::SuggestionDismiss(dirt));
    let left = listed(&app);
    assert_eq!(left.len(), found.len() - 1);
    assert!(!left.contains(&found[dirt]));
    assert!(outlines(&mut app, root).is_empty(), "its outline goes too");
    press(&mut app, root, DockAction::Suggest);
    assert!(
        !listed(&app).contains(&found[dirt]),
        "dismissed stays dismissed"
    );
}

fn draft(app: &App) -> jackdaw::tileset_layout::LayoutDraft {
    view(app).layout.expect("a draft")
}

fn popovers(app: &mut App, root: Entity) -> usize {
    (app.world_mut().query::<&LayoutPopover>().iter(app.world()))
        .filter(|p| p.root == root)
        .count()
}

#[test]
fn lay_out_opens_the_popover_with_the_suggestion_preselected() {
    let (mut app, dir) = named();
    let (_, root) = opened(&mut app);
    press(&mut app, root, DockAction::Suggest);
    let found = listed(&app);
    let dirt = position(&found, |s| s.origin == DIRT_BLOCK);
    let before = on_disk(&dir);
    press(&mut app, root, DockAction::SuggestionLayOut(dirt));
    assert_eq!(popovers(&mut app, root), 1);
    let d = draft(&app);
    assert_eq!((d.origin, d.size), (DIRT_BLOCK, found[dirt].size));
    assert_eq!((d.inner, d.outer), (DIRT, GRASS));
    assert_eq!(d.pick, None, "a 3x3 with inner corners still asks");
    assert_eq!(on_disk(&dir), before, "nothing applied");

    press(&mut app, root, DockAction::LayoutCancel);
    let found = listed(&app);
    let sixteen = position(&found, |s| match &s.kind {
        SuggestionKind::Block { layouts, .. } => {
            s.named() && layouts[0].template.shape() == UVec2::splat(4)
        }
        SuggestionKind::Object { .. } => false,
    });
    press(&mut app, root, DockAction::SuggestionLayOut(sixteen));
    let SuggestionKind::Block {
        layouts,
        inner,
        outer,
    } = &found[sixteen].kind
    else {
        unreachable!()
    };
    let d = draft(&app);
    assert_eq!(d.pick, Some((layouts[0].template, layouts[0].placement)));
    assert_eq!((Some(d.inner), Some(d.outer)), (*inner, *outer));
}

#[test]
fn add_object_adds_the_suggested_object() {
    let (mut app, dir) = named();
    let (_, root) = opened(&mut app);
    press(&mut app, root, DockAction::Suggest);
    let found = listed(&app);
    let object = position(&found, |s| matches!(s.kind, SuggestionKind::Object { .. }));
    press(&mut app, root, DockAction::SuggestionObjects);
    let rows_before = rows(&mut app, root);
    press(&mut app, root, DockAction::SuggestionShow(object));
    assert_eq!(outlines(&mut app, root).len(), 1);
    press(&mut app, root, DockAction::SuggestionAddObject(object));
    let objects = on_disk(&dir).objects;
    assert_eq!(objects.len(), 1);
    assert_eq!(
        (objects[0].origin, objects[0].size),
        (found[object].origin, found[object].size)
    );
    assert_eq!(rows(&mut app, root), rows_before - 1, "its row goes");
    assert!(outlines(&mut app, root).is_empty(), "and its outline");
    assert_eq!(view(&app).shown, None);
}

#[test]
fn a_block_of_two_placements_asks_which_one() {
    let (mut app, _dir) = named();
    let (_, root) = opened(&mut app);
    press(&mut app, root, DockAction::Suggest);
    let found = listed(&app);
    let corners = position(&found, |s| match &s.kind {
        SuggestionKind::Block { layouts, .. } => layouts[0].template == LayoutTemplate::Corner2x2,
        SuggestionKind::Object { .. } => false,
    });
    press(&mut app, root, DockAction::SuggestionLayOut(corners));
    assert_eq!(popovers(&mut app, root), 1);
    assert_eq!(draft(&app).pick, None, "a 2x2 asks ground or walls");
}

#[test]
fn a_suggestion_on_a_missing_sheet_is_dropped() {
    let (mut app, _dir) = named();
    let (_, root) = opened(&mut app);
    press(&mut app, root, DockAction::Suggest);
    let stale = Suggestion {
        sheet: 7,
        ..listed(&app)[0].clone()
    };
    {
        let mut views = app.world_mut().resource_mut::<TilesetDockViews>();
        let view = views.0.values_mut().next().expect("a view");
        view.suggestions
            .as_mut()
            .expect("a list")
            .push(stale.clone());
    }
    press(&mut app, root, DockAction::Bits(BitsOverlay::Off));
    assert!(!listed(&app).contains(&stale));
}

#[test]
fn objects_are_one_folded_group() {
    let (mut app, _dir) = named();
    let (_, root) = opened(&mut app);
    press(&mut app, root, DockAction::Suggest);
    let found = listed(&app);
    let objects: Vec<usize> = (0..found.len()).filter(|i| !is_block(&found[*i])).collect();
    assert!(!objects.is_empty());
    let heading = format!("{} objects", objects.len());
    assert!(texts(&mut app, root).contains(&heading), "{heading}");
    let shown = |app: &mut App| {
        (app.world_mut().query::<&DockControl>().iter(app.world()))
            .filter(|c| c.root == root)
            .filter(|c| matches!(c.action, DockAction::SuggestionAddObject(_)))
            .count()
    };
    assert_eq!(shown(&mut app), 0, "folded at first");
    press(&mut app, root, DockAction::SuggestionObjects);
    assert_eq!(shown(&mut app), objects.len());
    press(&mut app, root, DockAction::SuggestionObjects);
    assert_eq!(shown(&mut app), 0);
}

#[test]
fn rows_are_one_short_line() {
    let (mut app, _dir) = named();
    let (_, root) = opened(&mut app);
    press(&mut app, root, DockAction::Suggest);
    let shown = texts(&mut app, root);
    assert!(
        shown.iter().any(|t| t == "Dirt over grass, 3x3 + corners"),
        "{shown:?}"
    );
    let labels: Vec<(String, LineBreak)> = (app.world_mut())
        .query::<(&Text, &TextLayout, &SuggestionLabel)>()
        .iter(app.world())
        .map(|(t, layout, _)| (t.0.clone(), layout.linebreak))
        .collect();
    assert!(!labels.is_empty());
    for (text, wrap) in labels {
        assert!(!text.contains(" at "), "{text}");
        assert_eq!(wrap, LineBreak::NoWrap, "{text}");
    }
}
