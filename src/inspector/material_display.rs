use bevy::{prelude::*, render::render_resource::Face};
use jackdaw_feathers::{
    icons::{EditorFontItalic, Icon, IconFont},
    slider_row::FieldKind,
    tokens,
};

use crate::material_ui::{
    ActionHeaderProps, DEPTH_BIAS_RANGE, UNIT_RANGE, fill_surface_rows, fill_texture_rows,
    spawn_action_header, spawn_checkbox_row, spawn_combobox_row, spawn_preview, spawn_scalar_row,
};

/// The material cards shown in the Material inspector tab, in display order.
/// Each maps to one card; the `material_card::` type-path prefix routes them
/// all to the `material` category. Adding a card is one new variant plus a
/// body builder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MaterialCardKind {
    Preview,
    Surface,
    Textures,
    Settings,
}

impl MaterialCardKind {
    pub(crate) const ALL: [MaterialCardKind; 4] = [
        MaterialCardKind::Preview,
        MaterialCardKind::Surface,
        MaterialCardKind::Textures,
        MaterialCardKind::Settings,
    ];

    pub(crate) fn type_path(self) -> &'static str {
        match self {
            MaterialCardKind::Preview => "material_card::preview",
            MaterialCardKind::Surface => "material_card::surface",
            MaterialCardKind::Textures => "material_card::textures",
            MaterialCardKind::Settings => "material_card::settings",
        }
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            MaterialCardKind::Preview => "Preview",
            MaterialCardKind::Surface => "Surface",
            MaterialCardKind::Textures => "Textures",
            MaterialCardKind::Settings => "Settings",
        }
    }

    pub(crate) fn icon(self) -> Icon {
        match self {
            MaterialCardKind::Preview => Icon::Eye,
            MaterialCardKind::Surface => Icon::Palette,
            MaterialCardKind::Textures => Icon::Image,
            MaterialCardKind::Settings => Icon::Settings,
        }
    }

    /// Preview and Surface open by default; Textures and Settings collapsed.
    pub(crate) fn default_collapsed(self) -> bool {
        matches!(
            self,
            MaterialCardKind::Textures | MaterialCardKind::Settings
        )
    }
}

/// Textures card body: the shared slot rows.
pub(super) fn fill_textures_card(
    world: &mut World,
    body: Entity,
    handle: Handle<StandardMaterial>,
) {
    let Some(m) = world
        .resource::<Assets<StandardMaterial>>()
        .get(&handle)
        .cloned()
    else {
        return;
    };
    let icon_font = world
        .get_resource::<IconFont>()
        .map(|f| f.0.clone())
        .unwrap_or_default();
    fill_texture_rows(&mut world.commands(), body, &m, &handle, &icon_font);
    world.flush();
}

/// Surface card body: the core PBR fields, all editable, applied live.
pub(super) fn fill_surface_card(world: &mut World, body: Entity, handle: Handle<StandardMaterial>) {
    let Some(m) = world
        .resource::<Assets<StandardMaterial>>()
        .get(&handle)
        .cloned()
    else {
        return;
    };
    fill_surface_rows(&mut world.commands(), body, &m, &handle);
    world.flush();
}

/// What the clip threshold row shows under any mode but Mask, the only mode that
/// stores one.
const MASK_THRESHOLD_DEFAULT: f64 = 0.5;

/// Settings card body: culling, transparency, and rendering flags.
pub(super) fn fill_settings_card(
    world: &mut World,
    body: Entity,
    handle: Handle<StandardMaterial>,
) {
    let Some(m) = world
        .resource::<Assets<StandardMaterial>>()
        .get(&handle)
        .cloned()
    else {
        return;
    };

    // Culling: None / Front / Back.
    let cull_idx = match m.cull_mode {
        None => 0,
        Some(Face::Front) => 1,
        Some(Face::Back) => 2,
    };
    let alpha_idx = match m.alpha_mode {
        AlphaMode::Opaque => 0,
        AlphaMode::Mask(_) => 1,
        AlphaMode::Blend => 2,
        AlphaMode::Premultiplied => 3,
        AlphaMode::AlphaToCoverage => 4,
        AlphaMode::Add => 5,
        AlphaMode::Multiply => 6,
    };

    let commands = &mut world.commands();
    spawn_combobox_row(
        commands,
        body,
        "Culling",
        vec!["None", "Front", "Back"],
        cull_idx,
        handle.clone(),
        |world, h, i| {
            if let Some(mut m) = world.resource_mut::<Assets<StandardMaterial>>().get_mut(h) {
                m.cull_mode = match i {
                    1 => Some(Face::Front),
                    2 => Some(Face::Back),
                    _ => None,
                };
            }
        },
    );
    spawn_checkbox_row(
        commands,
        body,
        "Double Sided",
        0,
        m.double_sided,
        handle.clone(),
        |m, v| m.double_sided = v,
    );
    spawn_checkbox_row(
        commands,
        body,
        "Unlit",
        0,
        m.unlit,
        handle.clone(),
        |m, v| {
            m.unlit = v;
        },
    );
    spawn_checkbox_row(
        commands,
        body,
        "Fog",
        0,
        m.fog_enabled,
        handle.clone(),
        |m, v| m.fog_enabled = v,
    );
    spawn_scalar_row(
        commands,
        body,
        "Depth Bias",
        0,
        DEPTH_BIAS_RANGE,
        FieldKind::Continuous,
        &m,
        handle.clone(),
        |m| m.depth_bias as f64,
        |m, v| m.depth_bias = v as f32,
    );
    spawn_combobox_row(
        commands,
        body,
        "Alpha Mode",
        vec![
            "Opaque",
            "Mask",
            "Blend",
            "Premultiplied",
            "AlphaToCoverage",
            "Add",
            "Multiply",
        ],
        alpha_idx,
        handle.clone(),
        |world, h, i| {
            if let Some(mut m) = world.resource_mut::<Assets<StandardMaterial>>().get_mut(h) {
                m.alpha_mode = match i {
                    1 => AlphaMode::Mask(0.5),
                    2 => AlphaMode::Blend,
                    3 => AlphaMode::Premultiplied,
                    4 => AlphaMode::AlphaToCoverage,
                    5 => AlphaMode::Add,
                    6 => AlphaMode::Multiply,
                    _ => AlphaMode::Opaque,
                };
            }
        },
    );
    // Reading the threshold is only meaningful under Mask; writing it
    // switches the mode to Mask as a side effect.
    spawn_scalar_row(
        commands,
        body,
        "Clip Threshold",
        1,
        UNIT_RANGE,
        FieldKind::Continuous,
        &m,
        handle,
        |m| match m.alpha_mode {
            AlphaMode::Mask(t) => t as f64,
            _ => MASK_THRESHOLD_DEFAULT,
        },
        |m, v| m.alpha_mode = AlphaMode::Mask(v.clamp(0.0, 1.0) as f32),
    );
    world.flush();
}

/// Inject one card per `MaterialCardKind` for `source` under `inspector_entity`.
/// Shells are spawned synchronously (so all material cards register this frame);
/// bodies are filled via deferred world closures.
pub(crate) fn inject_material_cards(
    commands: &mut Commands,
    source: Entity,
    inspector_entity: Entity,
    icon_font: &Handle<Font>,
    collapse_state: &super::InspectorCollapseState,
) {
    for kind in MaterialCardKind::ALL {
        let collapsed = collapse_state
            .0
            .get(kind.title())
            .copied()
            .unwrap_or(kind.default_collapsed());
        let body = super::material_card_routing::spawn_material_card_shell(
            commands,
            inspector_entity,
            kind.title(),
            kind.icon(),
            kind.type_path(),
            icon_font,
            collapsed,
        );
        commands.queue(move |world: &mut World| {
            fill_material_card_body(world, source, body, kind);
        });
    }
}

/// Fill one material card body. Resolves the material handle for `source`
/// and dispatches to the appropriate card builder.
pub(crate) fn fill_material_card_body(
    world: &mut World,
    source: Entity,
    body: Entity,
    kind: MaterialCardKind,
) {
    if world.get_entity(body).is_err() {
        return;
    }
    let Some(handle) = resolve_material_handle(world, source) else {
        if kind == MaterialCardKind::Surface {
            world.spawn((
                Text::new("No material assigned"),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..default()
                },
                TextColor(tokens::TEXT_SECONDARY),
                ChildOf(body),
            ));
        }
        return;
    };
    match kind {
        MaterialCardKind::Preview => fill_preview_card(world, body, handle),
        MaterialCardKind::Surface => fill_surface_card(world, body, handle),
        MaterialCardKind::Textures => fill_textures_card(world, body, handle),
        MaterialCardKind::Settings => fill_settings_card(world, body, handle),
    }
}

/// Resolve a `Handle<StandardMaterial>` for the given source entity.
/// For brush entities, delegates to the brush face resolution logic.
/// For mesh entities, reads `MeshMaterial3d<StandardMaterial>` directly.
pub(crate) fn resolve_material_handle(
    world: &World,
    source: Entity,
) -> Option<Handle<StandardMaterial>> {
    if world.get::<crate::brush::Brush>(source).is_some() {
        return super::material_card_routing::resolve_brush_material_handle(world, source);
    }
    world
        .get::<MeshMaterial3d<StandardMaterial>>(source)
        .map(|m| m.0.clone())
}

/// Preview card body: the action header, then the shared preview widget.
/// Points the preview at the inspected material while mounted.
pub(super) fn fill_preview_card(world: &mut World, body: Entity, handle: Handle<StandardMaterial>) {
    let image = {
        let mut state = world.resource_mut::<crate::material_preview::MaterialPreviewState>();
        state.active_material = Some(handle.clone());
        state.preview_image.clone()
    };
    let (name, saved) = world
        .get_resource::<crate::material_assets::MaterialRegistry>()
        .and_then(|registry| {
            registry
                .entries
                .iter()
                .find(|e| e.handle == handle)
                .map(|e| (e.name.clone(), e.saved))
        })
        .unwrap_or_else(|| ("Material".to_string(), true));
    let icon_font = world
        .get_resource::<IconFont>()
        .map(|f| f.0.clone())
        .unwrap_or_default();
    let italic_font = world
        .get_resource::<EditorFontItalic>()
        .map(|f| f.0.clone())
        .unwrap_or_default();

    let commands = &mut world.commands();
    spawn_action_header(
        commands,
        body,
        ActionHeaderProps {
            name,
            saved,
            italic_font: &italic_font,
            icon_font: &icon_font,
            actions: crate::material_ui::library_actions(),
        },
    );
    spawn_preview(commands, body, image);
    world.flush();
}

#[cfg(test)]
mod card_kind_tests {
    use super::MaterialCardKind;

    #[test]
    fn kinds_are_in_display_order_with_prefixed_paths() {
        let kinds = MaterialCardKind::ALL;
        assert_eq!(kinds.len(), 4);
        assert_eq!(kinds[0], MaterialCardKind::Preview);
        assert_eq!(kinds[3], MaterialCardKind::Settings);
        for kind in kinds {
            assert!(
                kind.type_path().starts_with("material_card::"),
                "{:?} must use the material_card:: prefix",
                kind
            );
            assert!(!kind.title().is_empty());
        }
        assert!(!MaterialCardKind::Preview.default_collapsed());
        assert!(!MaterialCardKind::Surface.default_collapsed());
        assert!(MaterialCardKind::Textures.default_collapsed());
        assert!(MaterialCardKind::Settings.default_collapsed());
    }
}

#[cfg(test)]
mod surface_card_tests {
    use super::fill_surface_card;
    use crate::material_ui::{MaterialFieldBinding, MaterialFieldMarker};
    use bevy::prelude::*;

    #[test]
    fn surface_card_spawns_rows() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.add_plugins(bevy::scene::ScenePlugin);
        app.init_asset::<StandardMaterial>();
        app.init_asset::<Font>();

        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());

        let body = app.world_mut().spawn(Node::default()).id();

        fill_surface_card(app.world_mut(), body, handle);
        app.world_mut().flush();

        // 4 numeric rows (Metallic, Roughness, Reflectance, IOR) plus 2 colour fields,
        // each a row and the collapsed picker container under it.
        let child_count = app
            .world()
            .get::<Children>(body)
            .map(Children::len)
            .unwrap_or(0);
        assert_eq!(child_count, 8, "expected 8 direct children under the body");

        // Both pickers start closed: each is taller than the rest of the section
        // together.
        let closed = app
            .world()
            .get::<Children>(body)
            .map(|children| {
                children
                    .iter()
                    .filter(|&child| {
                        app.world()
                            .get::<Node>(child)
                            .is_some_and(|node| node.display == Display::None)
                    })
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(closed, 2, "both colour pickers start collapsed");

        // One MaterialFieldMarker per color picker (2 total).
        let marker_count = app
            .world_mut()
            .query::<&MaterialFieldMarker>()
            .iter(app.world())
            .count();
        assert_eq!(marker_count, 2, "expected 2 MaterialFieldMarker components");

        // One MaterialFieldBinding per numeric field (4 total).
        let binding_count = app
            .world_mut()
            .query::<&MaterialFieldBinding>()
            .iter(app.world())
            .count();
        assert_eq!(
            binding_count, 4,
            "expected 4 MaterialFieldBinding components"
        );
    }
}

#[cfg(test)]
mod settings_card_tests {
    use super::fill_settings_card;
    use crate::material_ui::{
        MaterialCheckboxBinding, MaterialComboBoxSelection, MaterialFieldBinding,
        MaterialFieldMarker,
    };
    use bevy::prelude::*;

    #[test]
    fn settings_card_spawns_rows() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.add_plugins(bevy::scene::ScenePlugin);
        app.init_asset::<StandardMaterial>();
        app.init_asset::<Font>();
        app.init_asset::<Image>();

        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());

        let body = app.world_mut().spawn(Node::default()).id();

        fill_settings_card(app.world_mut(), body, handle);
        app.world_mut().flush();

        // 2 menu rows (Culling, Alpha Mode) each carry MaterialComboBoxSelection.
        let combo_count = app
            .world_mut()
            .query::<&MaterialComboBoxSelection>()
            .iter(app.world())
            .count();
        assert_eq!(combo_count, 2, "expected 2 material menu selections");

        // MaterialFieldMarker is also placed on each menu entity.
        let marker_count = app
            .world_mut()
            .query::<&MaterialFieldMarker>()
            .iter(app.world())
            .count();
        assert_eq!(marker_count, 2, "expected 2 MaterialFieldMarker components");

        // 3 checkbox rows (Double Sided, Unlit, Fog).
        let checkbox_count = app
            .world_mut()
            .query::<&MaterialCheckboxBinding>()
            .iter(app.world())
            .count();
        assert_eq!(
            checkbox_count, 3,
            "expected 3 MaterialCheckboxBinding components"
        );

        // 2 numeric rows (Depth Bias, Clip Threshold).
        let binding_count = app
            .world_mut()
            .query::<&MaterialFieldBinding>()
            .iter(app.world())
            .count();
        assert_eq!(
            binding_count, 2,
            "expected 2 MaterialFieldBinding components"
        );
    }
}

#[cfg(test)]
mod textures_card_tests {
    use super::fill_textures_card;
    use crate::material_ui::{MaterialCheckboxBinding, MaterialTextureSlotRow, TextureSlot};
    use bevy::prelude::*;
    use jackdaw_feathers::icons::IconFont;

    #[test]
    fn textures_card_spawns_every_slot_and_the_normal_flip() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.add_plugins(bevy::scene::ScenePlugin);
        app.init_asset::<StandardMaterial>();
        app.init_asset::<Image>();
        app.init_asset::<Font>();

        // The texture-slot icons read IconFont at spawn time. Insert a weak
        // handle so the world satisfies the resource without a full UI stack.
        let icon_font: Handle<Font> = Handle::default();
        app.world_mut().insert_resource(IconFont(icon_font));

        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());

        let body = app.world_mut().spawn(Node::default()).id();

        fill_textures_card(app.world_mut(), body, handle);
        app.world_mut().flush();

        let slot_count = app
            .world_mut()
            .query::<&MaterialTextureSlotRow>()
            .iter(app.world())
            .count();
        assert_eq!(
            slot_count,
            TextureSlot::ALL.len(),
            "every texture slot gets a row"
        );

        // One checkbox binding: Flip Normal Y, under the normal map.
        let checkbox_count = app
            .world_mut()
            .query::<&MaterialCheckboxBinding>()
            .iter(app.world())
            .count();
        assert_eq!(checkbox_count, 1, "expected 1 MaterialCheckboxBinding");
    }
}

#[cfg(test)]
mod preview_card_tests {
    use super::fill_preview_card;
    use crate::material_preview::MaterialPreviewState;
    use crate::material_ui::{
        MaterialPreviewView, PreviewShapeButton, refresh_preview_shape_buttons,
    };
    use bevy::prelude::*;

    fn make_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.add_plugins(bevy::scene::ScenePlugin);
        app.init_asset::<StandardMaterial>();
        app.init_asset::<Image>();
        app.init_asset::<Font>();
        app.init_resource::<MaterialPreviewState>();
        app
    }

    #[test]
    fn fill_preview_card_spawns_view_and_shape_buttons() {
        let mut app = make_app();

        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());

        let body = app.world_mut().spawn(Node::default()).id();

        fill_preview_card(app.world_mut(), body, handle.clone());
        app.world_mut().flush();

        let view_count = app
            .world_mut()
            .query::<&MaterialPreviewView>()
            .iter(app.world())
            .count();
        assert_eq!(view_count, 1, "expected exactly one MaterialPreviewView");

        let shape_count = app
            .world_mut()
            .query::<&PreviewShapeButton>()
            .iter(app.world())
            .count();
        assert_eq!(
            shape_count, 3,
            "expected three PreviewShapeButton components"
        );

        let active = app
            .world()
            .resource::<MaterialPreviewState>()
            .active_material
            .clone();
        assert_eq!(
            active.as_ref(),
            Some(&handle),
            "active_material must be set to the provided handle"
        );
    }

    // Running the system in a schedule catches intra-system query conflicts
    // (Bevy B0001), which crash the editor at startup. Calling the card builder
    // directly does not exercise this, so a builder-only test missed it once.
    #[test]
    fn refresh_preview_shape_buttons_initializes_without_query_conflict() {
        let mut app = make_app();
        app.add_systems(Update, refresh_preview_shape_buttons);
        // Would panic with B0001 at system init if the queries aliased.
        app.update();
    }
}

#[cfg(test)]
mod inject_material_cards_tests {
    use super::{MaterialCardKind, inject_material_cards};
    use crate::inspector::{ComponentDisplayTypePath, InspectorCollapseState};
    use bevy::ecs::system::RunSystemOnce;
    use bevy::prelude::*;
    use jackdaw_feathers::icons::{EditorFont, IconFont};
    use std::collections::HashSet;

    fn make_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.add_plugins(bevy::scene::ScenePlugin);
        app.init_asset::<StandardMaterial>();
        app.init_asset::<Image>();
        app.init_asset::<Font>();

        // Stub font resources (weak handles satisfy the borrow without loading).
        let editor_font: Handle<Font> = Handle::default();
        let icon_font: Handle<Font> = Handle::default();
        app.world_mut().insert_resource(EditorFont(editor_font));
        app.world_mut().insert_resource(IconFont(icon_font));

        // Preview card reads this resource.
        app.init_resource::<crate::material_preview::MaterialPreviewState>();
        app.init_resource::<InspectorCollapseState>();

        app
    }

    #[test]
    fn four_cards_with_expected_type_paths() {
        let mut app = make_app();

        // Source: a mesh entity with a real material handle so
        // resolve_material_handle returns Some and all four body builders run.
        let mat_handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());

        let source = app.world_mut().spawn(MeshMaterial3d(mat_handle)).id();

        let inspector = app.world_mut().spawn_empty().id();

        // Call inject_material_cards through a one-shot system so Commands is
        // available. The system captures all needed values by closure.
        app.world_mut()
            .run_system_once(
                move |mut commands: Commands,
                      collapse_state: Res<InspectorCollapseState>,
                      icon_font: Res<IconFont>| {
                    inject_material_cards(
                        &mut commands,
                        source,
                        inspector,
                        &icon_font.0,
                        &collapse_state,
                    );
                },
            )
            .expect("inject_material_cards system runs");

        // Flush so the queued body-fill closures run.
        app.world_mut().flush();

        // Collect all ComponentDisplayTypePath values in the world.
        let mut type_paths: HashSet<String> = HashSet::new();
        let mut q = app.world_mut().query::<&ComponentDisplayTypePath>();
        for tp in q.iter(app.world()) {
            type_paths.insert(tp.0.clone());
        }

        let expected: HashSet<String> = MaterialCardKind::ALL
            .iter()
            .map(|k| k.type_path().to_string())
            .collect();

        assert_eq!(
            type_paths, expected,
            "expected exactly the four material card type paths, got {type_paths:?}"
        );
    }
}
