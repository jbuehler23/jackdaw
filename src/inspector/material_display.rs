use bevy::{
    input::mouse::{MouseScrollUnit, MouseWheel},
    picking::hover::Hovered,
    prelude::*,
    render::render_resource::Face,
};
use jackdaw_feathers::{
    button::{ButtonProps, ButtonSize, ButtonVariant, button, set_button_variant},
    checkbox::{CheckboxCommitEvent, CheckboxProps, checkbox},
    color_picker::{ColorPickerCommitEvent, ColorPickerProps, color_picker},
    combobox::{ComboBoxChangeEvent, ComboBoxOptionData, combobox_with_selected},
    icons::{EditorFont, Icon, IconFont, icon_colored},
    text_edit::{self, TextEditCommitEvent, TextEditProps},
    tokens,
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

/// Marker for material field UI entities
#[derive(Component)]
struct MaterialFieldMarker;

/// Binding that links a material `text_edit` to a material asset handle and field mutator.
#[derive(Component)]
pub(super) struct MaterialFieldBinding {
    pub(super) material_handle: Handle<StandardMaterial>,
    pub(super) apply_fn: fn(&mut StandardMaterial, f64),
}

// ---------------------------------------------------------------------------
// Shared row-builder helpers
// ---------------------------------------------------------------------------

/// Labeled inline color picker bound to a `StandardMaterial` color field.
/// `rgba` is the current color as `[f32; 4]`; `write` applies a committed color.
pub(super) fn spawn_material_color_field(
    world: &mut World,
    parent: Entity,
    label: &str,
    rgba: [f32; 4],
    handle: Handle<StandardMaterial>,
    write: fn(&mut StandardMaterial, [f32; 4]),
) {
    let col = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(tokens::SPACING_XS),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();

    world.spawn((
        Text::new(format!("{label}:")),
        TextFont {
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(col),
    ));

    let picker = world
        .spawn((
            color_picker(ColorPickerProps::new().with_color(rgba).inline()),
            MaterialFieldMarker,
            ChildOf(col),
        ))
        .id();

    let picker_handle = handle.clone();
    world.entity_mut(picker).observe(
        move |event: On<ColorPickerCommitEvent>,
              mut materials: ResMut<Assets<StandardMaterial>>| {
            if let Some(material) = materials.get_mut(&picker_handle) {
                write(material, event.color);
            }
        },
    );
}

/// Binding component that links a material checkbox to its asset handle and field mutator.
#[derive(Component)]
pub(super) struct MaterialCheckboxBinding {
    pub(super) material_handle: Handle<StandardMaterial>,
    pub(super) apply_fn: fn(&mut StandardMaterial, bool),
}

/// Handle `CheckboxCommitEvent` for material checkbox bindings.
pub(super) fn on_material_checkbox_commit(
    event: On<CheckboxCommitEvent>,
    bindings: Query<&MaterialCheckboxBinding>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Ok(binding) = bindings.get(event.entity) else {
        return;
    };
    if let Some(material) = materials.get_mut(&binding.material_handle) {
        (binding.apply_fn)(material, event.checked);
    }
}

/// Labeled checkbox bound to a `StandardMaterial` bool field.
pub(super) fn spawn_material_checkbox_field(
    world: &mut World,
    parent: Entity,
    label: &str,
    value: bool,
    handle: Handle<StandardMaterial>,
    write: fn(&mut StandardMaterial, bool),
) {
    let editor_font = world.resource::<EditorFont>().0.clone();
    let icon_font = world.resource::<IconFont>().0.clone();

    let row = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_XS),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();

    world.spawn((
        Text::new(format!("{label}:")),
        TextFont {
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(row),
    ));

    world.spawn((
        checkbox(
            CheckboxProps::new("").checked(value),
            &editor_font,
            &icon_font,
        ),
        MaterialCheckboxBinding {
            material_handle: handle,
            apply_fn: write,
        },
        ChildOf(row),
    ));
}

/// Labeled combobox; `options` are labels, `selected` the current index,
/// `on_select(world, handle, index)` applies the choice to the asset.
pub(super) fn spawn_material_combobox_field(
    world: &mut World,
    parent: Entity,
    label: &str,
    options: Vec<&'static str>,
    selected: usize,
    handle: Handle<StandardMaterial>,
    on_select: fn(&mut World, &Handle<StandardMaterial>, usize),
) {
    let row = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_XS),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();

    world.spawn((
        Text::new(format!("{label}:")),
        TextFont {
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        ChildOf(row),
    ));

    let option_data: Vec<ComboBoxOptionData> = options
        .iter()
        .map(|&s| ComboBoxOptionData::new(s))
        .collect();

    let combo = world
        .spawn((
            combobox_with_selected(option_data, selected),
            MaterialFieldMarker,
            ChildOf(row),
        ))
        .id();

    let combo_handle = handle.clone();
    world.entity_mut(combo).observe(
        move |event: On<ComboBoxChangeEvent>, mut commands: Commands| {
            let idx = event.selected;
            let h = combo_handle.clone();
            commands.queue(move |world: &mut World| {
                on_select(world, &h, idx);
            });
        },
    );
}

/// Marker placed on the row container of each texture slot. Tests count
/// instances to verify the expected number of slots were spawned.
#[derive(Component)]
pub(super) struct MaterialTextureSlot;

/// Texture slot row: label, a thumbnail (or "none"), a browse button, and a
/// clear button. Browse/clear dispatch through the existing
/// `material.browse_texture_slot` / `material.clear_texture_slot` operators
/// via [`crate::material_browser::PendingTextureSlot`].
pub(super) fn spawn_material_texture_slot(
    world: &mut World,
    parent: Entity,
    label: &str,
    current: Option<Handle<Image>>,
    handle: Handle<StandardMaterial>,
    slot: crate::material_browser::TextureSlot,
) {
    use crate::material_browser::{
        MaterialBrowseTextureSlotOp, MaterialClearTextureSlotOp, PendingTextureSlot,
    };
    use jackdaw_api::op::{Operator as _, OperatorCommandsExt as _};

    let icon_font = world.resource::<IconFont>().0.clone();

    let row = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_XS),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            MaterialTextureSlot,
            ChildOf(parent),
        ))
        .id();

    // Label
    world.spawn((
        Text::new(format!("{label}:")),
        TextFont {
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Node {
            min_width: Val::Px(120.0),
            flex_shrink: 0.0,
            ..Default::default()
        },
        ChildOf(row),
    ));

    // Thumbnail or "none" indicator
    if let Some(ref img) = current {
        world.spawn((
            ImageNode::new(img.clone()),
            Node {
                width: Val::Px(24.0),
                height: Val::Px(24.0),
                flex_shrink: 0.0,
                ..Default::default()
            },
            ChildOf(row),
        ));
    } else {
        world.spawn((
            Text::new("none"),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_SECONDARY),
            Node {
                width: Val::Px(24.0),
                flex_shrink: 0.0,
                ..Default::default()
            },
            ChildOf(row),
        ));
    }

    // Browse button
    let browse_btn = world
        .spawn((
            icon_colored(
                Icon::FolderOpen,
                tokens::FONT_SM,
                icon_font.clone(),
                tokens::TEXT_SECONDARY,
            ),
            Node {
                padding: UiRect::all(Val::Px(2.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_SM)),
                ..Default::default()
            },
            Button,
            ChildOf(row),
        ))
        .id();

    let browse_handle = handle.clone();
    world.entity_mut(browse_btn).observe(
        move |_: On<Pointer<Click>>,
              mut pending: ResMut<PendingTextureSlot>,
              mut commands: Commands| {
            pending.slot = Some(slot);
            pending.material_handle = Some(browse_handle.clone());
            commands.operator(MaterialBrowseTextureSlotOp::ID).call();
        },
    );

    // Clear button (only if a texture is currently set)
    if current.is_some() {
        let clear_btn = world
            .spawn((
                icon_colored(
                    Icon::X,
                    tokens::FONT_SM,
                    icon_font.clone(),
                    tokens::TEXT_SECONDARY,
                ),
                Node {
                    padding: UiRect::all(Val::Px(2.0)),
                    border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_SM)),
                    ..Default::default()
                },
                Button,
                ChildOf(row),
            ))
            .id();

        let clear_handle = handle.clone();
        world.entity_mut(clear_btn).observe(
            move |_: On<Pointer<Click>>,
                  mut pending: ResMut<PendingTextureSlot>,
                  mut commands: Commands| {
                pending.slot = Some(slot);
                pending.material_handle = Some(clear_handle.clone());
                commands.operator(MaterialClearTextureSlotOp::ID).call();
            },
        );
    }
}

/// Textures card body. Spawns one slot per texture field on `StandardMaterial`.
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
    use crate::material_browser::TextureSlot;
    spawn_material_texture_slot(
        world,
        body,
        "Base Color",
        m.base_color_texture.clone(),
        handle.clone(),
        TextureSlot::BaseColorTexture,
    );
    spawn_material_texture_slot(
        world,
        body,
        "Normal",
        m.normal_map_texture.clone(),
        handle.clone(),
        TextureSlot::NormalMapTexture,
    );
    spawn_material_checkbox_field(
        world,
        body,
        "Flip Normal Y",
        m.flip_normal_map_y,
        handle.clone(),
        |m, v| m.flip_normal_map_y = v,
    );
    spawn_material_texture_slot(
        world,
        body,
        "Metallic/Roughness",
        m.metallic_roughness_texture.clone(),
        handle.clone(),
        TextureSlot::MetallicRoughnessTexture,
    );
    spawn_material_texture_slot(
        world,
        body,
        "Occlusion",
        m.occlusion_texture.clone(),
        handle.clone(),
        TextureSlot::OcclusionTexture,
    );
    spawn_material_texture_slot(
        world,
        body,
        "Emissive",
        m.emissive_texture.clone(),
        handle,
        TextureSlot::EmissiveTexture,
    );
}

/// Handle `TextEditCommitEvent` for material field bindings.
pub(super) fn on_material_text_commit(
    event: On<TextEditCommitEvent>,
    bindings: Query<&MaterialFieldBinding>,
    child_of_query: Query<&ChildOf>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mut current = event.entity;
    for _ in 0..4 {
        let Ok(child_of) = child_of_query.get(current) else {
            break;
        };
        if let Ok(binding) = bindings.get(child_of.parent()) {
            let value: f64 = event.text.parse().unwrap_or(0.0);
            if let Some(material) = materials.get_mut(&binding.material_handle) {
                (binding.apply_fn)(material, value);
            }
            return;
        }
        current = child_of.parent();
    }
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
    let base = m.base_color.to_srgba();
    spawn_material_color_field(
        world,
        body,
        "Base Color",
        [base.red, base.green, base.blue, base.alpha],
        handle.clone(),
        |m, c| m.base_color = Color::srgba(c[0], c[1], c[2], c[3]),
    );
    spawn_material_numeric_field(
        world,
        body,
        "Metallic",
        m.metallic as f64,
        handle.clone(),
        |m, v| m.metallic = v.clamp(0.0, 1.0) as f32,
    );
    spawn_material_numeric_field(
        world,
        body,
        "Roughness",
        m.perceptual_roughness as f64,
        handle.clone(),
        |m, v| m.perceptual_roughness = v.clamp(0.0, 1.0) as f32,
    );
    spawn_material_numeric_field(
        world,
        body,
        "Reflectance",
        m.reflectance as f64,
        handle.clone(),
        |m, v| m.reflectance = v.clamp(0.0, 1.0) as f32,
    );
    spawn_material_numeric_field(world, body, "IOR", m.ior as f64, handle.clone(), |m, v| {
        m.ior = v as f32;
    });
    let e = m.emissive;
    spawn_material_color_field(
        world,
        body,
        "Emissive",
        [e.red, e.green, e.blue, e.alpha],
        handle,
        |m, c| m.emissive = LinearRgba::new(c[0], c[1], c[2], c[3]),
    );
}

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
    spawn_material_combobox_field(
        world,
        body,
        "Culling",
        vec!["None", "Front", "Back"],
        cull_idx,
        handle.clone(),
        |world, h, i| {
            if let Some(m) = world.resource_mut::<Assets<StandardMaterial>>().get_mut(h) {
                m.cull_mode = match i {
                    1 => Some(Face::Front),
                    2 => Some(Face::Back),
                    _ => None,
                };
            }
        },
    );

    spawn_material_checkbox_field(
        world,
        body,
        "Double Sided",
        m.double_sided,
        handle.clone(),
        |m, v| m.double_sided = v,
    );
    spawn_material_checkbox_field(world, body, "Unlit", m.unlit, handle.clone(), |m, v| {
        m.unlit = v;
    });
    spawn_material_checkbox_field(world, body, "Fog", m.fog_enabled, handle.clone(), |m, v| {
        m.fog_enabled = v;
    });
    spawn_material_numeric_field(
        world,
        body,
        "Depth Bias",
        m.depth_bias as f64,
        handle.clone(),
        |m, v| m.depth_bias = v as f32,
    );

    // Alpha mode (Opaque / Mask / Blend / Premultiplied / AlphaToCoverage / Add / Multiply).
    let (alpha_idx, threshold) = match m.alpha_mode {
        AlphaMode::Opaque => (0, 0.5_f64),
        AlphaMode::Mask(t) => (1, t as f64),
        AlphaMode::Blend => (2, 0.5),
        AlphaMode::Premultiplied => (3, 0.5),
        AlphaMode::AlphaToCoverage => (4, 0.5),
        AlphaMode::Add => (5, 0.5),
        AlphaMode::Multiply => (6, 0.5),
    };
    spawn_material_combobox_field(
        world,
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
            if let Some(m) = world.resource_mut::<Assets<StandardMaterial>>().get_mut(h) {
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

    // Clip threshold: always visible. Reading it is only meaningful when alpha
    // mode is Mask; writing it switches the mode to Mask as a side effect.
    spawn_material_numeric_field(world, body, "Clip Threshold", threshold, handle, |m, v| {
        m.alpha_mode = AlphaMode::Mask(v.clamp(0.0, 1.0) as f32);
    });
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
                    font_size: jackdaw_feathers::tokens::FONT_SM,
                    ..default()
                },
                TextColor(jackdaw_feathers::tokens::TEXT_SECONDARY),
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
    use super::{MaterialFieldBinding, MaterialFieldMarker, fill_surface_card};
    use bevy::prelude::*;

    #[test]
    fn surface_card_spawns_rows() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<StandardMaterial>();

        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());

        let body = app.world_mut().spawn(Node::default()).id();

        fill_surface_card(app.world_mut(), body, handle);
        app.world_mut().flush();

        // 2 color fields (Base Color, Emissive) + 4 numeric fields
        // (Metallic, Roughness, Reflectance, IOR) = 6 direct children.
        let child_count = app
            .world()
            .get::<Children>(body)
            .map(Children::len)
            .unwrap_or(0);
        assert_eq!(child_count, 6, "expected 6 field rows under the body");

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
    use super::{
        MaterialCheckboxBinding, MaterialFieldBinding, MaterialFieldMarker, fill_settings_card,
    };
    use bevy::prelude::*;
    use jackdaw_feathers::{
        combobox::EditorComboBox,
        icons::{EditorFont, IconFont},
    };

    #[test]
    fn settings_card_spawns_rows() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<StandardMaterial>();
        app.init_asset::<Font>();

        // The checkbox helper reads EditorFont and IconFont at spawn time.
        // Insert weak handles so the world satisfies the resource requirements
        // without needing a full UI plugin stack.
        let editor_font: Handle<Font> = Handle::default();
        let icon_font: Handle<Font> = Handle::default();
        app.world_mut().insert_resource(EditorFont(editor_font));
        app.world_mut().insert_resource(IconFont(icon_font));

        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());

        let body = app.world_mut().spawn(Node::default()).id();

        fill_settings_card(app.world_mut(), body, handle);
        app.world_mut().flush();

        // 2 combobox rows (Culling, Alpha Mode) carry EditorComboBox + MaterialFieldMarker.
        let combo_count = app
            .world_mut()
            .query::<&EditorComboBox>()
            .iter(app.world())
            .count();
        assert_eq!(combo_count, 2, "expected 2 EditorComboBox components");

        // MaterialFieldMarker is also placed on each combobox entity.
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
    use super::{MaterialCheckboxBinding, MaterialTextureSlot, fill_textures_card};
    use bevy::prelude::*;
    use jackdaw_feathers::icons::{EditorFont, IconFont};

    #[test]
    fn textures_card_spawns_slots_and_checkbox() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<StandardMaterial>();
        app.init_asset::<Font>();

        // The checkbox helper reads EditorFont and IconFont at spawn time.
        // Insert weak handles so the world satisfies the resource requirements
        // without needing a full UI plugin stack.
        let editor_font: Handle<Font> = Handle::default();
        let icon_font: Handle<Font> = Handle::default();
        app.world_mut().insert_resource(EditorFont(editor_font));
        app.world_mut().insert_resource(IconFont(icon_font));

        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());

        let body = app.world_mut().spawn(Node::default()).id();

        fill_textures_card(app.world_mut(), body, handle);
        app.world_mut().flush();

        // Five texture slots: Base Color, Normal, Metallic/Roughness, Occlusion, Emissive.
        let slot_count = app
            .world_mut()
            .query::<&MaterialTextureSlot>()
            .iter(app.world())
            .count();
        assert_eq!(slot_count, 5, "expected 5 MaterialTextureSlot rows");

        // One checkbox binding: Flip Normal Y.
        let checkbox_count = app
            .world_mut()
            .query::<&MaterialCheckboxBinding>()
            .iter(app.world())
            .count();
        assert_eq!(checkbox_count, 1, "expected 1 MaterialCheckboxBinding");
    }
}

// ---------------------------------------------------------------------------
// Preview card
// ---------------------------------------------------------------------------

/// Marker on the render-to-texture image node inside the Preview card.
/// Orbit and zoom systems target entities with this component.
#[derive(Component)]
pub(super) struct MaterialPreviewView;

/// Marker on each shape selector button. Carries the shape it represents so
/// the click observer and the highlight system can act without a stale capture.
#[derive(Component)]
pub(super) struct PreviewShapeButton(pub(super) crate::material_preview::PreviewShape);

/// Preview card body: the render-to-texture surface plus a shape strip.
/// Sets the preview to follow the inspected material while mounted.
pub(super) fn fill_preview_card(world: &mut World, body: Entity, handle: Handle<StandardMaterial>) {
    let image = {
        let mut state = world.resource_mut::<crate::material_preview::MaterialPreviewState>();
        state.active_material = Some(handle);
        state.preview_image.clone()
    };

    // Preview image (fixed square). `Hovered` is inserted so pointer events
    // are tracked; the per-entity drag observer drives orbit.
    let view = world
        .spawn((
            MaterialPreviewView,
            ImageNode::new(image),
            Node {
                width: Val::Px(200.0),
                height: Val::Px(200.0),
                flex_shrink: 0.0,
                ..Default::default()
            },
            Hovered::default(),
            ChildOf(body),
        ))
        .id();

    // Per-entity observer: pointer drag over the preview adjusts orbit.
    world.entity_mut(view).observe(
        |event: On<Pointer<Drag>>,
         mut state: ResMut<crate::material_preview::MaterialPreviewState>| {
            let delta = event.delta;
            state.orbit_yaw += delta.x * 0.01;
            state.orbit_pitch = (state.orbit_pitch + delta.y * 0.01).clamp(-1.4, 1.4);
        },
    );

    // Shape strip under the preview image.
    let row = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(tokens::SPACING_XS),
                ..Default::default()
            },
            ChildOf(body),
        ))
        .id();

    for shape in crate::material_preview::PreviewShape::ALL {
        let label = match shape {
            crate::material_preview::PreviewShape::Sphere => "Sphere",
            crate::material_preview::PreviewShape::Cube => "Cube",
            crate::material_preview::PreviewShape::Plane => "Plane",
        };
        world.spawn((
            button(ButtonProps::new(label).with_size(ButtonSize::MD)),
            PreviewShapeButton(shape),
            ChildOf(row),
        ));
    }
}

/// Observer registered in `InspectorPlugin`: clicking a `PreviewShapeButton`
/// sets `MaterialPreviewState.preview_shape` to the button's shape.
pub(super) fn on_preview_shape_button_click(
    event: On<jackdaw_feathers::button::ButtonClickEvent>,
    buttons: Query<&PreviewShapeButton>,
    mut state: ResMut<crate::material_preview::MaterialPreviewState>,
) {
    let Ok(btn) = buttons.get(event.entity) else {
        return;
    };
    state.preview_shape = btn.0;
}

/// Each frame: set each shape button's visual variant to `Active` when its
/// shape matches `MaterialPreviewState.preview_shape`, else `Default`.
/// Only writes when a change is detected to avoid thrashing the render data.
pub(super) fn refresh_preview_shape_buttons(
    state: Res<crate::material_preview::MaterialPreviewState>,
    mut buttons: Query<(
        &PreviewShapeButton,
        &mut ButtonVariant,
        &mut BackgroundColor,
        &mut BorderColor,
    )>,
) {
    if !state.is_changed() {
        return;
    }
    for (btn, mut variant, mut bg, mut border) in &mut buttons {
        let wanted = if btn.0 == state.preview_shape {
            ButtonVariant::Active
        } else {
            ButtonVariant::Default
        };
        if *variant == wanted {
            continue;
        }
        *variant = wanted;
        set_button_variant(wanted, &mut bg, &mut border);
    }
}

/// Each frame: while the mouse wheel moves over a `MaterialPreviewView` entity,
/// adjust `MaterialPreviewState.zoom_distance`.
pub(super) fn preview_zoom_from_scroll(
    mut wheel: MessageReader<MouseWheel>,
    views: Query<&Hovered, With<MaterialPreviewView>>,
    mut state: ResMut<crate::material_preview::MaterialPreviewState>,
) -> Result<(), BevyError> {
    let any_hovered = views.iter().any(Hovered::get);
    if !any_hovered {
        return Ok(());
    }
    for event in wheel.read() {
        let lines = match event.unit {
            MouseScrollUnit::Line => event.y,
            MouseScrollUnit::Pixel => event.y / 24.0,
        };
        state.zoom_distance = (state.zoom_distance - lines * 0.3).clamp(1.5, 8.0);
    }
    Ok(())
}

fn spawn_material_numeric_field(
    world: &mut World,
    parent: Entity,
    label: &str,
    value: f64,
    material_handle: Handle<StandardMaterial>,
    apply_fn: fn(&mut StandardMaterial, f64),
) {
    let row = world
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_XS),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();

    world.spawn((
        Text::new(format!("{label}:")),
        TextFont {
            font_size: tokens::FONT_SM,
            ..Default::default()
        },
        TextColor(tokens::TEXT_SECONDARY),
        Node {
            min_width: Val::Px(20.0),
            flex_shrink: 0.0,
            ..Default::default()
        },
        ChildOf(row),
    ));

    world.spawn((
        text_edit::text_edit(
            TextEditProps::default()
                .numeric_f32()
                .grow()
                .with_default_value(value.to_string()),
        ),
        MaterialFieldBinding {
            material_handle,
            apply_fn,
        },
        ChildOf(row),
    ));
}

#[cfg(test)]
mod preview_card_tests {
    use super::{
        MaterialPreviewView, PreviewShapeButton, fill_preview_card, refresh_preview_shape_buttons,
    };
    use crate::material_preview::MaterialPreviewState;
    use bevy::prelude::*;

    fn make_app() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.add_plugins(bevy::asset::AssetPlugin::default());
        app.init_asset::<StandardMaterial>();
        app.init_asset::<Image>();
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

        // EditorFont and IconFont are also read by card body helpers.
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
