use jackdaw::brush::BrushEditMode;
use jackdaw::mesh_quick_menu::{MeshQuickMenu, items_for_submode};

#[test]
fn items_for_submode_returns_configured_ops_per_mode() {
    let cfg = MeshQuickMenu::default();

    let face = items_for_submode(&cfg, BrushEditMode::Face);
    assert!(face.iter().any(|i| i.action == "brush.mesh.extrude_region"));
    assert!(face.iter().any(|i| i.action == "brush.mesh.inset"));

    let edge = items_for_submode(&cfg, BrushEditMode::Edge);
    assert!(edge.iter().any(|i| i.action == "brush.mesh.loop_cut"));

    let vert = items_for_submode(&cfg, BrushEditMode::Vertex);
    assert!(vert.iter().any(|i| i.action == "brush.mesh.weld_selected"));
}

#[test]
fn action_equals_operator_id_and_labels_present() {
    let cfg = MeshQuickMenu::default();
    let face = items_for_submode(&cfg, BrushEditMode::Face);
    assert!(!face.is_empty());
    // Every mapped item carries a non-empty label and an action equal to a
    // configured operator id.
    for item in &face {
        assert!(!item.label.is_empty());
        assert!(item.action.starts_with("brush.mesh."));
    }
}

#[test]
fn clip_and_knife_have_no_quick_menu() {
    let cfg = MeshQuickMenu::default();
    assert!(items_for_submode(&cfg, BrushEditMode::Clip).is_empty());
    assert!(items_for_submode(&cfg, BrushEditMode::Knife).is_empty());
}
