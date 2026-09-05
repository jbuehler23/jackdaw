//! Material editing widgets: preview, texture-slot row, scalar row, checkbox row, section
//! and action header. Shared by the Materials window, the inspector and the terrain panel;
//! nothing here knows which surface it is on.
//!
//! Widgets do not self-manage their own state, so a commit observer reflects the new value
//! back onto the widget before writing it to the asset. `MaterialCheckboxBinding` and
//! `MaterialFieldBinding` ride on the widget entity, and the global observers self-filter by
//! looking the binding up on the event source.

use std::ops::Range;

use bevy::{
    asset::{AssetEvent, AssetEventSystems, AssetId},
    feathers::controls::{
        ButtonVariant as FeathersButtonVariant, FeathersButton, FeathersCheckbox, FeathersMenu,
        FeathersMenuButton, FeathersMenuItem, FeathersMenuPopup,
    },
    feathers::theme::ThemedText,
    input::mouse::{MouseScrollUnit, MouseWheel},
    input_focus::{InputFocus, tab_navigation::TabIndex},
    picking::hover::Hovered,
    prelude::*,
    text::FontSource,
    ui::Checked,
    ui_widgets::{Activate, SliderDragState, SliderValue, ValueChange},
};
use jackdaw_api::op::{Operator as _, OperatorCommandsExt as _};
use jackdaw_feathers::{
    button::{ButtonOperatorCall, ButtonVariant, IconButtonProps, icon_button},
    field_row::{FieldRowProps, spawn_field_row},
    icons::Icon,
    panel_card::{PanelCard, PanelCardCollapseState, PanelCardProps, spawn_panel_card},
    slider_row::{FieldKind, SliderRowProps, spawn_slider_row},
    swatch_row::{SwatchRowProps, spawn_swatch_row},
    tokens,
    tooltip::Tooltip,
};

use crate::material_preview::{MaterialPreviewState, PreviewShape};

pub(crate) fn plugin(app: &mut App) {
    app.add_observer(on_material_checkbox_commit)
        .add_observer(on_material_slider_commit)
        .add_observer(on_preview_shape_button_click)
        .add_systems(
            Update,
            (
                refresh_preview_shape_buttons,
                preview_zoom_from_scroll,
                fit_preview_shape_strip,
                gate_collapsed_color_picker_focus,
                flush_material_slider_drag,
            )
                .run_if(in_state(crate::AppState::Editor)),
        )
        .add_systems(
            PostUpdate,
            refresh_material_rows
                .after(AssetEventSystems)
                .run_if(in_state(crate::AppState::Editor)),
        );
}

// ---------------------------------------------------------------------------
// Texture slots
// ---------------------------------------------------------------------------

/// A texture slot on `StandardMaterial`, in the order surfaces show them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TextureSlot {
    BaseColorTexture,
    NormalMapTexture,
    MetallicRoughnessTexture,
    OcclusionTexture,
    EmissiveTexture,
    DepthMap,
}

impl TextureSlot {
    pub(crate) const ALL: [TextureSlot; 6] = [
        TextureSlot::BaseColorTexture,
        TextureSlot::NormalMapTexture,
        TextureSlot::MetallicRoughnessTexture,
        TextureSlot::OcclusionTexture,
        TextureSlot::EmissiveTexture,
        TextureSlot::DepthMap,
    ];

    /// The field name, for the file dialog title and for logs.
    pub(crate) fn field(self) -> &'static str {
        match self {
            TextureSlot::BaseColorTexture => "base_color_texture",
            TextureSlot::NormalMapTexture => "normal_map_texture",
            TextureSlot::MetallicRoughnessTexture => "metallic_roughness_texture",
            TextureSlot::EmissiveTexture => "emissive_texture",
            TextureSlot::OcclusionTexture => "occlusion_texture",
            TextureSlot::DepthMap => "depth_map",
        }
    }

    /// The row label.
    pub(crate) fn label(self) -> &'static str {
        match self {
            TextureSlot::BaseColorTexture => "Base Color",
            TextureSlot::NormalMapTexture => "Normal",
            TextureSlot::MetallicRoughnessTexture => "Metallic/Rough",
            TextureSlot::EmissiveTexture => "Emissive",
            TextureSlot::OcclusionTexture => "Occlusion",
            TextureSlot::DepthMap => "Height",
        }
    }

    pub(crate) fn is_srgb(self) -> bool {
        matches!(
            self,
            TextureSlot::BaseColorTexture | TextureSlot::EmissiveTexture
        )
    }

    pub(crate) fn get_from(self, mat: &StandardMaterial) -> Option<Handle<Image>> {
        match self {
            TextureSlot::BaseColorTexture => mat.base_color_texture.clone(),
            TextureSlot::NormalMapTexture => mat.normal_map_texture.clone(),
            TextureSlot::MetallicRoughnessTexture => mat.metallic_roughness_texture.clone(),
            TextureSlot::EmissiveTexture => mat.emissive_texture.clone(),
            TextureSlot::OcclusionTexture => mat.occlusion_texture.clone(),
            TextureSlot::DepthMap => mat.depth_map.clone(),
        }
    }

    pub(crate) fn set_on(self, mat: &mut StandardMaterial, handle: Option<Handle<Image>>) {
        match self {
            TextureSlot::BaseColorTexture => mat.base_color_texture = handle,
            TextureSlot::NormalMapTexture => mat.normal_map_texture = handle,
            TextureSlot::MetallicRoughnessTexture => {
                mat.metallic_roughness_texture = handle;
                if mat.metallic_roughness_texture.is_some() {
                    // The scalars multiply the bound texture, so 1.0 uses it as authored.
                    mat.metallic = 1.0;
                    mat.perceptual_roughness = 1.0;
                }
            }
            TextureSlot::EmissiveTexture => mat.emissive_texture = handle,
            TextureSlot::OcclusionTexture => mat.occlusion_texture = handle,
            TextureSlot::DepthMap => {
                let has_depth = handle.is_some();
                mat.depth_map = handle;
                if has_depth {
                    if mat.parallax_depth_scale == 0.0 {
                        mat.parallax_depth_scale = 0.05;
                    }
                    if mat.max_parallax_layer_count == 0.0 {
                        mat.max_parallax_layer_count = 32.0;
                    }
                    mat.parallax_mapping_method = bevy::pbr::ParallaxMappingMethod::Occlusion;
                } else {
                    mat.parallax_depth_scale = 0.0;
                    mat.max_parallax_layer_count = 0.0;
                }
            }
        }
    }
}

/// Marker on the row container of each texture slot.
#[derive(Component)]
pub(crate) struct MaterialTextureSlotRow;

/// Marker for material field composites that carry no binding of their own (the colour
/// picker root, a combobox menu).
#[derive(Component)]
pub(crate) struct MaterialFieldMarker;

/// A texture slot row: square swatch, the bound file's name, then assign and clear.
///
/// Assign and clear route the slot and target material through
/// [`crate::material_browser::PendingTextureSlot`] before dispatching the operators, because
/// a material handle is not a `PropertyValue`.
pub(crate) fn spawn_texture_slot_row(
    commands: &mut Commands,
    parent: Entity,
    slot: TextureSlot,
    current: Option<Handle<Image>>,
    handle: Handle<StandardMaterial>,
    icon_font: &Handle<Font>,
) -> Entity {
    use crate::material_browser::{
        MaterialBrowseTextureSlotOp, MaterialClearTextureSlotOp, PendingTextureSlot,
    };

    let name = current
        .as_ref()
        .and_then(Handle::path)
        .and_then(|p| {
            p.path()
                .file_name()
                .map(|f| f.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "None".to_string());

    let props = SwatchRowProps::new(slot.label());
    let props = match current.clone() {
        Some(image) => props.bound(image, name),
        None => props.placeholder(name),
    };
    let row = spawn_swatch_row(commands, parent, props);

    commands
        .entity(row.row)
        .insert((MaterialTextureSlotRow, Hovered::default()))
        .insert(Tooltip::title(slot.label()).with_description(slot.field()));

    let browse_handle = handle.clone();
    commands
        .spawn((
            icon_button(
                IconButtonProps::new(Icon::FolderOpen).variant(ButtonVariant::Ghost),
                icon_font,
            ),
            ChildOf(row.actions),
        ))
        .observe(
            move |_: On<Pointer<Click>>,
                  mut pending: ResMut<PendingTextureSlot>,
                  mut commands: Commands| {
                pending.slot = Some(slot);
                pending.material_handle = Some(browse_handle.clone());
                commands.operator(MaterialBrowseTextureSlotOp::ID).call();
            },
        );

    if current.is_some() {
        let clear_handle = handle;
        commands
            .spawn((
                icon_button(
                    IconButtonProps::new(Icon::X).variant(ButtonVariant::Ghost),
                    icon_font,
                ),
                ChildOf(row.actions),
            ))
            .observe(
                move |_: On<Pointer<Click>>,
                      mut pending: ResMut<PendingTextureSlot>,
                      mut commands: Commands| {
                    pending.slot = Some(slot);
                    pending.material_handle = Some(clear_handle.clone());
                    commands.operator(MaterialClearTextureSlotOp::ID).call();
                },
            );
    }

    row.row
}

// ---------------------------------------------------------------------------
// Scalar rows
// ---------------------------------------------------------------------------

/// Links a material slider to its asset and the field it writes.
///
/// `read_fn` reads the same field `apply_fn` writes, so a row can be put back to what the
/// asset holds without knowing which field it is on. `shown` is the last value this field is
/// known to display, letting a refresh tell an edit made elsewhere from the row's own commit
/// coming back round.
#[derive(Component)]
pub(crate) struct MaterialFieldBinding {
    pub(crate) material_handle: Handle<StandardMaterial>,
    pub(crate) read_fn: fn(&StandardMaterial) -> f64,
    pub(crate) apply_fn: fn(&mut StandardMaterial, f64),
    pub(crate) shown: f64,
    /// Whether this row's slider was being dragged as of the last frame.
    ///
    /// The end of a drag schedules the material's file to be rewritten.
    /// `flush_material_slider_drag` detects the end from this rather than from an event, so a
    /// drag that ends without emitting one still persists.
    pub(crate) dragging: bool,
}

/// Range for the `StandardMaterial` fields that are normalised 0-1 fractions: metallic,
/// roughness, reflectance, and the alpha-mask cutoff.
pub(crate) const UNIT_RANGE: Range<f32> = 0.0..1.0;

/// Index of refraction, from vacuum to the densest gemstones. Below 1.0 is not physical.
const IOR_RANGE: Range<f32> = 1.0..4.0;

/// Parallax depth, in tangent-space units. The slot seeds at 0.05; past half a unit the
/// effect reads as a smear rather than as depth.
const PARALLAX_DEPTH_RANGE: Range<f32> = 0.0..0.5;

/// Parallax ray-march steps. The slot seeds at 32; past 64 the cost rises with no visible
/// difference.
const PARALLAX_LAYERS_RANGE: Range<f32> = 0.0..64.0;

/// Depth bias, in the units the depth prepass compares. Symmetric: a decal is pulled in
/// front of its surface, co-planar geometry is pushed behind it.
pub(crate) const DEPTH_BIAS_RANGE: Range<f32> = -10.0..10.0;

/// A labeled [`jackdaw_feathers::slider_row`] bound to one `StandardMaterial` field. The
/// value shown is read through `read_fn`, so seeding and refreshing cannot drift apart.
///
/// `range` bounds what this widget can produce; `apply_fn` applies whatever hard bounds the
/// field has, leaving scripted paths unaffected by the range a row offers.
pub(crate) fn spawn_scalar_row(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    indent: u8,
    range: Range<f32>,
    kind: FieldKind,
    material: &StandardMaterial,
    material_handle: Handle<StandardMaterial>,
    read_fn: fn(&StandardMaterial) -> f64,
    apply_fn: fn(&mut StandardMaterial, f64),
) -> Entity {
    let value = read_fn(material);
    spawn_slider_row(
        commands,
        parent,
        SliderRowProps::new(label, value as f32, range)
            .indented(indent)
            .with_kind(kind),
        MaterialFieldBinding {
            material_handle,
            read_fn,
            apply_fn,
            shown: value,
            dragging: false,
        },
    )
    .row
}

/// Write a slider commit into the asset, reflecting it back onto the slider first:
/// `FeathersSlider` does not self-manage `SliderValue`, so without this the thumb would stand
/// still under the drag.
///
/// The asset is written on every event so the preview and viewport follow the drag. The
/// catalog is flagged only on `is_final`, since dirtying it schedules this material's file
/// and `catalog.bsn` to be rewritten and a drag emits an event per frame.
pub(crate) fn on_material_slider_commit(
    event: On<ValueChange<f32>>,
    mut bindings: Query<&mut MaterialFieldBinding>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    catalog: Option<ResMut<crate::asset_catalog::AssetCatalog>>,
    mut commands: Commands,
) {
    let slider = event.source;
    let Ok(mut binding) = bindings.get_mut(slider) else {
        return;
    };
    let value = f64::from(event.value);
    commands.entity(slider).insert(SliderValue(event.value));
    if let Some(mut material) = materials.get_mut(&binding.material_handle) {
        (binding.apply_fn)(&mut material, value);
    }
    binding.shown = value;
    if event.is_final {
        mark_catalog_dirty(catalog);
    }
}

/// Put every row bound to a changed material back to the value the asset holds.
///
/// One material can be open on several surfaces at once and a row seeds its value only when
/// it is built, so a stale row would write its old value back on its next commit and undo the
/// edit made on the other surface. Values only: nothing is despawned and no section is
/// rebuilt, so the preview's drag observer survives.
///
/// The asset is read immutably: `Assets::get_mut` raises `Modified` again and this system
/// would feed itself.
fn refresh_material_rows(
    mut events: MessageReader<AssetEvent<StandardMaterial>>,
    materials: Res<Assets<StandardMaterial>>,
    focus: Res<InputFocus>,
    mut fields: Query<(Entity, &mut MaterialFieldBinding, Option<&SliderDragState>)>,
    faces: Query<(&ColorRowFace, &MaterialColorBinding)>,
    child_of: Query<&ChildOf>,
    mut commands: Commands,
) {
    let modified: Vec<AssetId<StandardMaterial>> = events
        .read()
        .filter_map(|event| match event {
            AssetEvent::Modified { id } => Some(*id),
            _ => None,
        })
        .collect();
    if modified.is_empty() {
        return;
    }

    for (slider, mut binding, drag) in &mut fields {
        if !modified.contains(&binding.material_handle.id()) {
            continue;
        }
        // Re-seeding a row being dragged or arrow-keyed would jump the thumb out from under
        // the gesture; the commit handler writes the drag's own value back every event.
        if drag.is_some_and(|drag| drag.dragging) || holds_focus(slider, &focus, &child_of) {
            continue;
        }
        let Some(material) = materials.get(&binding.material_handle) else {
            continue;
        };
        let value = (binding.read_fn)(material);
        if value == binding.shown {
            continue;
        }
        binding.shown = value;
        // The slider's value text is a child that reads `SliderValue`, so one insert moves
        // the fill and redraws the digits.
        commands.entity(slider).insert(SliderValue(value as f32));
    }

    for (face, binding) in &faces {
        if !modified.contains(&binding.material_handle.id()) {
            continue;
        }
        let Some(material) = materials.get(&binding.material_handle) else {
            continue;
        };
        let rgba = (binding.read_fn)(material);
        let face = *face;
        commands.queue(move |world: &mut World| repaint_color_row_face(world, face, rgba));
    }
}

/// Whether keyboard focus is on `entity` or a descendant: a slider takes focus itself, a
/// composite control takes it on a child.
fn holds_focus(entity: Entity, focus: &InputFocus, child_of: &Query<&ChildOf>) -> bool {
    let Some(focused) = focus.get() else {
        return false;
    };
    focused == entity
        || child_of
            .iter_ancestors(focused)
            .any(|ancestor| ancestor == entity)
}

/// Flag the catalog once a material slider's drag has ended, however it ended.
///
/// [`on_material_slider_commit`] persists on the final `ValueChange` that closes an ordinary
/// drag. A drag that ends any other way (the pointer leaving the window, the widget disabled
/// mid-gesture, the gesture cancelled) emits no final event, so the edit would sit in memory
/// until something else dirties the catalog. The end is detected from the drag state, since
/// there is no event to read.
fn flush_material_slider_drag(
    mut fields: Query<(&mut MaterialFieldBinding, Option<&SliderDragState>)>,
    catalog: Option<ResMut<crate::asset_catalog::AssetCatalog>>,
) {
    let mut ended = false;
    for (mut binding, drag) in &mut fields {
        let dragging = drag.is_some_and(|drag| drag.dragging);
        if binding.dragging == dragging {
            continue;
        }
        ended |= binding.dragging;
        binding.dragging = dragging;
    }
    if ended {
        mark_catalog_dirty(catalog);
    }
}

/// Schedule the catalog and the edited material's file to be rewritten.
fn mark_catalog_dirty(catalog: Option<ResMut<crate::asset_catalog::AssetCatalog>>) {
    if let Some(mut catalog) = catalog {
        catalog.dirty = true;
    }
}

// ---------------------------------------------------------------------------
// Checkbox rows
// ---------------------------------------------------------------------------

/// Links a material checkbox to its asset and the field it writes.
#[derive(Component)]
pub(crate) struct MaterialCheckboxBinding {
    pub(crate) material_handle: Handle<StandardMaterial>,
    pub(crate) apply_fn: fn(&mut StandardMaterial, bool),
}

/// A labeled checkbox row bound to one `StandardMaterial` bool field.
pub(crate) fn spawn_checkbox_row(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    indent: u8,
    value: bool,
    handle: Handle<StandardMaterial>,
    write: fn(&mut StandardMaterial, bool),
) -> Entity {
    let field = spawn_field_row(commands, parent, FieldRowProps::new(label).indented(indent));
    let mut cb = commands.spawn_scene(bsn! { @FeathersCheckbox });
    cb.insert((
        MaterialCheckboxBinding {
            material_handle: handle,
            apply_fn: write,
        },
        ChildOf(field.control),
    ));
    // The checkbox does not self-manage `Checked`; seed the initial state.
    if value {
        cb.insert(Checked);
    }
    field.row
}

/// Reflect a checkbox commit back onto the box, then write it to the asset. This global
/// observer sees every checkbox change; the binding lookup on `event.source` self-filters to
/// material fields.
pub(crate) fn on_material_checkbox_commit(
    event: On<ValueChange<bool>>,
    bindings: Query<&MaterialCheckboxBinding>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    catalog: Option<ResMut<crate::asset_catalog::AssetCatalog>>,
    mut commands: Commands,
) {
    let target = event.source;
    let Ok(binding) = bindings.get(target) else {
        return;
    };
    let checked = event.value;
    if checked {
        commands.entity(target).insert(Checked);
    } else {
        commands.entity(target).remove::<Checked>();
    }
    if let Some(mut material) = materials.get_mut(&binding.material_handle) {
        (binding.apply_fn)(&mut material, checked);
    }
    mark_catalog_dirty(catalog);
}

// ---------------------------------------------------------------------------
// Colour and choice rows
// ---------------------------------------------------------------------------

/// The swatch that opens a colour row, and the hex beside it. Both are repainted from the
/// picker's commits.
#[derive(Component, Clone, Copy)]
struct ColorRowFace {
    swatch: Entity,
    hex: Entity,
}

/// Which colour the accompanying face draws, so an edit made on another surface can repaint
/// it.
#[derive(Component)]
struct MaterialColorBinding {
    material_handle: Handle<StandardMaterial>,
    read_fn: fn(&StandardMaterial) -> [f32; 4],
}

/// The container the collapsed picker hides in.
#[derive(Component)]
struct ColorPickerBody;

/// A labeled colour row bound to a `StandardMaterial` colour field.
///
/// Closed it is a one-row-high field row of label, swatch and hex; clicking the swatch opens
/// the 200px picker under it.
///
/// `read` answers sRGB and `write` receives sRGB, whatever the underlying field stores.
pub(crate) fn spawn_color_row(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    material: &StandardMaterial,
    handle: Handle<StandardMaterial>,
    read: fn(&StandardMaterial) -> [f32; 4],
    write: fn(&mut StandardMaterial, [f32; 4]),
) -> Entity {
    let rgba = read(material);
    let field = spawn_field_row(commands, parent, FieldRowProps::new(label));

    let swatch = commands
        .spawn((
            Node {
                width: Val::Px(tokens::SWATCH_SIZE),
                height: Val::Px(tokens::SWATCH_SIZE),
                flex_shrink: 0.0,
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_SM)),
                ..Default::default()
            },
            BackgroundColor(Color::srgba(rgba[0], rgba[1], rgba[2], rgba[3])),
            BorderColor::all(tokens::BORDER_SUBTLE),
            Hovered::default(),
            Tooltip::title("Edit colour"),
            ChildOf(field.control),
        ))
        .id();

    let hex = commands
        .spawn((
            Text::new(hex_of(rgba)),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_TERTIARY),
            Node {
                flex_grow: 1.0,
                flex_shrink: 1.0,
                min_width: Val::Px(0.0),
                overflow: Overflow::clip(),
                ..Default::default()
            },
            ChildOf(field.control),
        ))
        .id();

    // The picker lives in its own container under the row so opening it pushes the rows
    // below down rather than overlapping them.
    let body = commands
        .spawn((
            ColorPickerBody,
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                display: Display::None,
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();

    let face = ColorRowFace { swatch, hex };
    let write_handle = handle.clone();
    let root = crate::inspector::reflect_fields::spawn_color_picker(
        commands,
        body,
        rgba,
        label,
        0.0,
        move |world, rgba, _is_final| {
            if let Some(mut material) = world
                .resource_mut::<Assets<StandardMaterial>>()
                .get_mut(&write_handle)
            {
                write(&mut material, rgba);
            }
            repaint_color_row_face(world, face, rgba);
        },
    );
    commands.entity(root).insert(MaterialFieldMarker);
    commands.entity(field.row).insert((
        face,
        MaterialColorBinding {
            material_handle: handle,
            read_fn: read,
        },
    ));

    commands
        .entity(swatch)
        .observe(move |_: On<Pointer<Click>>, mut nodes: Query<&mut Node>| {
            if let Ok(mut node) = nodes.get_mut(body) {
                node.display = if node.display == Display::None {
                    Display::Flex
                } else {
                    Display::None
                };
            }
        });

    field.row
}

/// The tab index restored to a gated control when its picker opens again.
#[derive(Component)]
struct StowedTabIndex(i32);

/// Keep a collapsed picker out of the tab order.
///
/// `TabNavigation::gather_focusable` collects by `TabIndex` alone and never looks at
/// `Node::display`, so the hex entry and the four sliders of a closed picker would stay
/// tabbable into a subtree nothing draws. The walk skips negative indices.
fn gate_collapsed_color_picker_focus(
    bodies: Query<(Entity, &Node), With<ColorPickerBody>>,
    children: Query<&Children>,
    mut indices: Query<(&mut TabIndex, Option<&StowedTabIndex>)>,
    mut commands: Commands,
) {
    for (body, node) in &bodies {
        let collapsed = node.display == Display::None;
        for descendant in children.iter_descendants(body) {
            let Ok((mut index, stowed)) = indices.get_mut(descendant) else {
                continue;
            };
            match (collapsed, stowed) {
                (true, None) => {
                    commands.entity(descendant).insert(StowedTabIndex(index.0));
                    index.0 = -1;
                }
                (false, Some(stowed)) => {
                    index.0 = stowed.0;
                    commands.entity(descendant).remove::<StowedTabIndex>();
                }
                _ => {}
            }
        }
    }
}

/// Repaint a colour row's swatch and hex to `rgba`.
fn repaint_color_row_face(world: &mut World, face: ColorRowFace, rgba: [f32; 4]) {
    if let Ok(mut swatch) = world.get_entity_mut(face.swatch) {
        swatch.insert(BackgroundColor(Color::srgba(
            rgba[0], rgba[1], rgba[2], rgba[3],
        )));
    }
    if let Ok(mut hex) = world.get_entity_mut(face.hex) {
        hex.insert(Text::new(hex_of(rgba)));
    }
}

/// The sRGB hex a colour reads as. Alpha is omitted; the swatch beside it draws that.
fn hex_of(rgba: [f32; 4]) -> String {
    let byte = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02X}{:02X}{:02X}",
        byte(rgba[0]),
        byte(rgba[1]),
        byte(rgba[2])
    )
}

/// The selected option index on a material combobox menu, read back by item observers so a
/// repeated pick does not re-fire the write.
#[derive(Component)]
pub(crate) struct MaterialComboBoxSelection(pub(crate) usize);

/// A labeled choice row. `on_select(world, handle, index)` applies the pick to the asset; the
/// button caption tracks the current option.
pub(crate) fn spawn_combobox_row(
    commands: &mut Commands,
    parent: Entity,
    label: &str,
    options: Vec<&'static str>,
    selected: usize,
    handle: Handle<StandardMaterial>,
    on_select: fn(&mut World, &Handle<StandardMaterial>, usize),
) -> Entity {
    let field = spawn_field_row(commands, parent, FieldRowProps::new(label));
    let current_caption = options.get(selected).copied().unwrap_or("").to_string();

    let menu = commands
        .spawn_scene(bsn! { @FeathersMenu })
        .insert((
            MaterialFieldMarker,
            MaterialComboBoxSelection(selected),
            ChildOf(field.control),
        ))
        .id();

    let button = commands
        .spawn_scene(bsn! {
            @FeathersMenuButton {
                @caption: bsn! { Text({current_caption}) ThemedText },
            }
        })
        .insert(ChildOf(menu))
        .id();

    let popup = commands
        .spawn_scene(bsn! { @FeathersMenuPopup })
        .insert(ChildOf(menu))
        .id();

    for (idx, option) in options.into_iter().enumerate() {
        let handle = handle.clone();
        commands
            .spawn_scene(bsn! {
                @FeathersMenuItem {
                    @caption: bsn! { Text({option.to_string()}) ThemedText },
                }
            })
            .insert(ChildOf(popup))
            .observe(move |_activate: On<Activate>, mut commands: Commands| {
                let handle = handle.clone();
                commands.queue(move |world: &mut World| {
                    if let Some(sel) = world.get::<MaterialComboBoxSelection>(menu)
                        && sel.0 == idx
                    {
                        return;
                    }
                    if let Some(mut sel) = world.get_mut::<MaterialComboBoxSelection>(menu) {
                        sel.0 = idx;
                    }
                    set_menu_button_caption(world, button, option);
                    on_select(world, &handle, idx);
                });
            });
    }
    field.row
}

fn set_menu_button_caption(world: &mut World, button: Entity, text: &str) {
    let mut descendants: Vec<Entity> = Vec::new();
    if let Ok(children) = world.query::<&Children>().get(world, button) {
        descendants.extend(children.iter());
    }
    while let Some(entity) = descendants.pop() {
        if world.get::<Text>(entity).is_some() {
            world.entity_mut(entity).insert(Text::new(text.to_string()));
            return;
        }
        if let Ok(children) = world.query::<&Children>().get(world, entity) {
            descendants.extend(children.iter());
        }
    }
}

// ---------------------------------------------------------------------------
// Sections
// ---------------------------------------------------------------------------

/// One collapsible titled section on a material surface.
///
/// `default_collapsed` is stated per surface: the Materials window opens Textures, the
/// terrain panel keeps it shut behind the slot controls. `key` is what the user's own toggle
/// is remembered under, so a rebuild does not reopen what they closed.
pub(crate) struct MaterialSection {
    pub(crate) title: &'static str,
    pub(crate) icon: Icon,
    pub(crate) key: &'static str,
    pub(crate) default_collapsed: bool,
}

impl MaterialSection {
    pub(crate) const fn new(
        title: &'static str,
        icon: Icon,
        key: &'static str,
        default_collapsed: bool,
    ) -> Self {
        Self {
            title,
            icon,
            key,
            default_collapsed,
        }
    }
}

/// Spawn a collapsible titled section for a group of material fields.
pub(crate) fn spawn_section(
    commands: &mut Commands,
    parent: Entity,
    section: MaterialSection,
    icon_font: &Handle<Font>,
    collapse: &PanelCardCollapseState,
) -> PanelCard {
    spawn_panel_card(
        commands,
        parent,
        PanelCardProps::new(section.title)
            .with_icon(section.icon)
            .default_collapsed(section.default_collapsed)
            .remembered_as(section.key),
        icon_font,
        collapse,
    )
}

// ---------------------------------------------------------------------------
// Action header
// ---------------------------------------------------------------------------

/// One button in a material action header.
pub(crate) struct HeaderAction {
    pub(crate) icon: Icon,
    pub(crate) tooltip: &'static str,
    pub(crate) description: &'static str,
    pub(crate) call: ButtonOperatorCall,
}

impl HeaderAction {
    pub(crate) fn new(
        icon: Icon,
        tooltip: &'static str,
        description: &'static str,
        call: ButtonOperatorCall,
    ) -> Self {
        Self {
            icon,
            tooltip,
            description,
            call,
        }
    }
}

/// New / Save / Delete, aimed at whichever material the surface is previewing.
pub(crate) fn library_actions() -> Vec<HeaderAction> {
    vec![
        HeaderAction::new(
            Icon::Plus,
            "New Material",
            "Create a fresh empty material.",
            ButtonOperatorCall::new(crate::material_browser::MaterialCreateOp::ID),
        ),
        HeaderAction::new(
            Icon::Save,
            "Save Material",
            "Write this material to assets/materials as a reusable asset.",
            ButtonOperatorCall::new(crate::material_assets::MaterialSaveOp::ID),
        ),
        HeaderAction::new(
            Icon::Trash2,
            "Delete Material",
            "Delete this material's file and remove it from the project.",
            ButtonOperatorCall::new(crate::material_assets::MaterialDeleteOp::ID),
        ),
    ]
}

/// A material surface header: the material's name on the left, marked when it has no file
/// behind it, and the actions on the right.
pub(crate) struct ActionHeaderProps<'a> {
    pub(crate) name: String,
    /// A material with no file is named in italic and marked "(unsaved)".
    pub(crate) saved: bool,
    pub(crate) italic_font: &'a Handle<Font>,
    pub(crate) icon_font: &'a Handle<Font>,
    pub(crate) actions: Vec<HeaderAction>,
}

pub(crate) fn spawn_action_header(
    commands: &mut Commands,
    parent: Entity,
    props: ActionHeaderProps,
) -> Entity {
    let header = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                flex_wrap: FlexWrap::Wrap,
                align_items: AlignItems::Center,
                justify_content: JustifyContent::SpaceBetween,
                column_gap: Val::Px(tokens::SPACING_SM),
                row_gap: Val::Px(tokens::SPACING_XS),
                width: Val::Percent(100.0),
                min_height: Val::Px(tokens::ROW_HEIGHT),
                flex_shrink: 0.0,
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();

    commands.spawn((
        Text::new(unsaved_marked(&props.name, props.saved)),
        TextFont {
            font: if props.saved {
                FontSource::default()
            } else {
                FontSource::Handle(props.italic_font.clone())
            },
            font_size: tokens::TEXT_SIZE_SM,
            ..Default::default()
        },
        TextColor(if props.saved {
            tokens::TEXT_PRIMARY
        } else {
            tokens::TEXT_SECONDARY
        }),
        Node {
            flex_grow: 1.0,
            flex_shrink: 1.0,
            min_width: Val::Px(tokens::FIELD_LABEL_WIDTH),
            overflow: Overflow::clip(),
            ..Default::default()
        },
        ChildOf(header),
    ));

    let actions = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(tokens::SPACING_XS),
                flex_shrink: 0.0,
                ..Default::default()
            },
            ChildOf(header),
        ))
        .id();

    for action in props.actions {
        commands.spawn((
            icon_button(
                IconButtonProps::new(action.icon).variant(ButtonVariant::Ghost),
                props.icon_font,
            ),
            action.call,
            Tooltip::title(action.tooltip).with_description(action.description),
            ChildOf(actions),
        ));
    }

    header
}

/// A material's name, marked "(unsaved)" when it has no file behind it.
pub(crate) fn unsaved_marked(name: &str, saved: bool) -> String {
    if saved {
        name.to_string()
    } else {
        format!("{name} (unsaved)")
    }
}

// ---------------------------------------------------------------------------
// Preview
// ---------------------------------------------------------------------------

/// Marker on the render-to-texture image node. Orbit and zoom target it.
#[derive(Component)]
pub(crate) struct MaterialPreviewView;

/// Marker on each shape selector button, carrying the shape it selects.
#[derive(Component)]
pub(crate) struct PreviewShapeButton(pub(crate) PreviewShape);

/// Marker on the strip holding the three shape buttons.
#[derive(Component)]
pub(crate) struct PreviewShapeStrip;

/// Width below which the shape strip stacks its buttons: three captions at `TEXT_SIZE_SM`
/// plus their button padding and the gaps between them.
const SHAPE_STRIP_MIN_ROW_WIDTH: f32 = 168.0;

/// Lay the shape strip out as three columns or three rows, from its measured width. Only
/// writes on a change, so the layout does not thrash.
pub(crate) fn fit_preview_shape_strip(
    mut strips: Query<(&ComputedNode, &mut Node), With<PreviewShapeStrip>>,
) {
    for (computed, mut node) in &mut strips {
        let width = computed.size().x * computed.inverse_scale_factor();
        // A strip that has not been laid out yet reports zero; leave it at the row default
        // rather than stacking on a meaningless measurement.
        let wanted = if width > 0.0 && width < SHAPE_STRIP_MIN_ROW_WIDTH {
            FlexDirection::Column
        } else {
            FlexDirection::Row
        };
        if node.flex_direction != wanted {
            node.flex_direction = wanted;
        }
    }
}

/// The render-to-texture preview surface with the shape switcher beside it. Dragging orbits,
/// the wheel zooms.
///
/// The caller points [`MaterialPreviewState`] at the material to show before calling this.
pub(crate) fn spawn_preview(
    commands: &mut Commands,
    parent: Entity,
    image: Handle<Image>,
) -> Entity {
    let row = commands
        .spawn((
            Node {
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: Val::Px(tokens::SPACING_XS),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(parent),
        ))
        .id();

    let view = commands
        .spawn((
            MaterialPreviewView,
            ImageNode::new(image),
            Node {
                width: Val::Px(tokens::PREVIEW_IMAGE_SIZE),
                height: Val::Px(tokens::PREVIEW_IMAGE_SIZE),
                flex_shrink: 0.0,
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                ..Default::default()
            },
            Hovered::default(),
            ChildOf(row),
        ))
        .id();

    commands.entity(view).observe(
        |event: On<Pointer<Drag>>, mut state: ResMut<MaterialPreviewState>| {
            let delta = event.delta;
            state.orbit_yaw += delta.x * 0.01;
            state.orbit_pitch = (state.orbit_pitch + delta.y * 0.01).clamp(-1.4, 1.4);
        },
    );

    // The shape switcher sits directly under the preview it changes, as three equal columns
    // or three stacked rows; `fit_preview_shape_strip` picks from the measured width. It
    // never wraps, since a wrapped row of three leaves one button centred under two.
    let shapes = commands
        .spawn((
            PreviewShapeStrip,
            Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(tokens::SPACING_XS),
                row_gap: Val::Px(tokens::SPACING_XS),
                width: Val::Percent(100.0),
                ..Default::default()
            },
            ChildOf(row),
        ))
        .id();

    for shape in PreviewShape::ALL {
        let label = match shape {
            PreviewShape::Sphere => "Sphere",
            PreviewShape::Cube => "Cube",
            PreviewShape::Plane => "Plane",
        };
        commands
            .spawn_scene(bsn! {
                @FeathersButton {
                    @caption: bsn! { Text({label.to_string()}) ThemedText },
                }
                // Equal shares of the strip, so the three buttons match in either
                // arrangement. Patched onto the button's own `Node` rather than replacing
                // it: `FeathersButton` carries the padding, minimum height and centring.
                Node {
                    flex_grow: 1.0,
                    flex_shrink: 1.0,
                    flex_basis: px(0.0),
                    min_width: px(0.0),
                    overflow: {Overflow::clip()},
                }
            })
            .insert((PreviewShapeButton(shape), ChildOf(shapes)));
    }

    row
}

/// Activating a shape button selects that shape.
pub(crate) fn on_preview_shape_button_click(
    event: On<Activate>,
    buttons: Query<&PreviewShapeButton>,
    mut state: ResMut<MaterialPreviewState>,
) {
    let Ok(btn) = buttons.get(event.entity) else {
        return;
    };
    state.preview_shape = btn.0;
}

/// Mark the button whose shape is showing. Only writes on a change, so theming does not
/// thrash.
pub(crate) fn refresh_preview_shape_buttons(
    state: Res<MaterialPreviewState>,
    mut buttons: Query<(&PreviewShapeButton, &mut FeathersButtonVariant)>,
) {
    if !state.is_changed() {
        return;
    }
    for (btn, mut variant) in &mut buttons {
        let wanted = if btn.0 == state.preview_shape {
            FeathersButtonVariant::Primary
        } else {
            FeathersButtonVariant::Normal
        };
        if *variant == wanted {
            continue;
        }
        *variant = wanted;
    }
}

/// Wheel over a preview adjusts its distance.
pub(crate) fn preview_zoom_from_scroll(
    mut wheel: MessageReader<MouseWheel>,
    views: Query<&Hovered, With<MaterialPreviewView>>,
    mut state: ResMut<MaterialPreviewState>,
) {
    if !views.iter().any(Hovered::get) {
        return;
    }
    for event in wheel.read() {
        let lines = match event.unit {
            MouseScrollUnit::Line => event.y,
            MouseScrollUnit::Pixel => event.y / 24.0,
        };
        state.zoom_distance = (state.zoom_distance - lines * 0.3).clamp(1.5, 8.0);
    }
}

// ---------------------------------------------------------------------------
// Shared section bodies
// ---------------------------------------------------------------------------

/// The texture section: one row per slot, with Flip Normal Y indented under the normal map
/// it flips and the parallax scalars indented under the height map they need.
pub(crate) fn fill_texture_rows(
    commands: &mut Commands,
    body: Entity,
    material: &StandardMaterial,
    handle: &Handle<StandardMaterial>,
    icon_font: &Handle<Font>,
) {
    for slot in TextureSlot::ALL {
        spawn_texture_slot_row(
            commands,
            body,
            slot,
            slot.get_from(material),
            handle.clone(),
            icon_font,
        );
        match slot {
            TextureSlot::NormalMapTexture => {
                spawn_checkbox_row(
                    commands,
                    body,
                    "Flip Normal Y",
                    1,
                    material.flip_normal_map_y,
                    handle.clone(),
                    |m, v| m.flip_normal_map_y = v,
                );
            }
            TextureSlot::DepthMap if material.depth_map.is_some() => {
                spawn_scalar_row(
                    commands,
                    body,
                    "Depth Scale",
                    1,
                    PARALLAX_DEPTH_RANGE,
                    FieldKind::Continuous,
                    material,
                    handle.clone(),
                    |m| m.parallax_depth_scale as f64,
                    |m, v| m.parallax_depth_scale = v as f32,
                );
                spawn_scalar_row(
                    commands,
                    body,
                    "Max Layers",
                    1,
                    PARALLAX_LAYERS_RANGE,
                    FieldKind::Count,
                    material,
                    handle.clone(),
                    |m| m.max_parallax_layer_count as f64,
                    |m, v| m.max_parallax_layer_count = v as f32,
                );
            }
            _ => {}
        }
    }
}

/// `emissive` is the one colour field on `StandardMaterial` held as linear rather than sRGB.
/// Colour rows edit sRGB, so this pair converts at the binding.
fn base_color_srgb(material: &StandardMaterial) -> [f32; 4] {
    let c = material.base_color.to_srgba();
    [c.red, c.green, c.blue, c.alpha]
}

fn emissive_srgb(material: &StandardMaterial) -> [f32; 4] {
    let e = Color::LinearRgba(material.emissive).to_srgba();
    [e.red, e.green, e.blue, e.alpha]
}

fn set_emissive_srgb(material: &mut StandardMaterial, c: [f32; 4]) {
    material.emissive = Color::srgba(c[0], c[1], c[2], c[3]).to_linear();
}

/// The surface section: the core PBR values.
pub(crate) fn fill_surface_rows(
    commands: &mut Commands,
    body: Entity,
    material: &StandardMaterial,
    handle: &Handle<StandardMaterial>,
) {
    spawn_color_row(
        commands,
        body,
        "Base Color",
        material,
        handle.clone(),
        base_color_srgb,
        |m, c| m.base_color = Color::srgba(c[0], c[1], c[2], c[3]),
    );
    spawn_scalar_row(
        commands,
        body,
        "Metallic",
        0,
        UNIT_RANGE,
        FieldKind::Continuous,
        material,
        handle.clone(),
        |m| m.metallic as f64,
        |m, v| m.metallic = v.clamp(0.0, 1.0) as f32,
    );
    spawn_scalar_row(
        commands,
        body,
        "Roughness",
        0,
        UNIT_RANGE,
        FieldKind::Continuous,
        material,
        handle.clone(),
        |m| m.perceptual_roughness as f64,
        |m, v| m.perceptual_roughness = v.clamp(0.0, 1.0) as f32,
    );
    spawn_scalar_row(
        commands,
        body,
        "Reflectance",
        0,
        UNIT_RANGE,
        FieldKind::Continuous,
        material,
        handle.clone(),
        |m| m.reflectance as f64,
        |m, v| m.reflectance = v.clamp(0.0, 1.0) as f32,
    );
    spawn_scalar_row(
        commands,
        body,
        "IOR",
        0,
        IOR_RANGE,
        FieldKind::Continuous,
        material,
        handle.clone(),
        |m| m.ior as f64,
        |m, v| m.ior = v as f32,
    );
    spawn_color_row(
        commands,
        body,
        "Emissive",
        material,
        handle.clone(),
        emissive_srgb,
        set_emissive_srgb,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Binding a height map has to leave the parallax scalars usable rather than at zero.
    #[test]
    fn binding_a_height_map_turns_parallax_on_and_clearing_it_turns_it_off() {
        let mut material = StandardMaterial::default();
        TextureSlot::DepthMap.set_on(&mut material, Some(Handle::default()));
        assert!(material.parallax_depth_scale > 0.0);
        assert!(material.max_parallax_layer_count > 0.0);

        TextureSlot::DepthMap.set_on(&mut material, None);
        assert_eq!(material.parallax_depth_scale, 0.0);
        assert_eq!(material.max_parallax_layer_count, 0.0);
    }

    /// Colour data decodes as sRGB and everything else linearly; a slot that answered the
    /// wrong way would bind a washed-out normal map.
    #[test]
    fn only_the_color_carrying_slots_decode_as_srgb() {
        for slot in TextureSlot::ALL {
            let expected = matches!(
                slot,
                TextureSlot::BaseColorTexture | TextureSlot::EmissiveTexture
            );
            assert_eq!(slot.is_srgb(), expected, "{slot:?}");
        }
    }

    #[test]
    fn an_unsaved_name_says_so_and_a_saved_one_is_left_alone() {
        assert_eq!(unsaved_marked("grass", true), "grass");
        assert_eq!(unsaved_marked("grass", false), "grass (unsaved)");
    }

    #[test]
    fn emissive_reads_back_the_srgb_it_was_given() {
        let mut material = StandardMaterial::default();
        let wanted = [0.25, 0.5, 0.75, 1.0];
        set_emissive_srgb(&mut material, wanted);

        let read = emissive_srgb(&material);
        for (got, want) in read.iter().zip(wanted.iter()) {
            assert!((got - want).abs() < 1e-4, "{read:?} != {wanted:?}");
        }
    }

    #[test]
    fn emissive_is_stored_linear_under_the_srgb_the_row_shows() {
        let mut material = StandardMaterial::default();
        set_emissive_srgb(&mut material, [0.5, 0.5, 0.5, 1.0]);
        assert!(
            material.emissive.red < 0.45,
            "mid sRGB stores darker in linear, got {}",
            material.emissive.red
        );
    }

    #[test]
    fn hex_names_the_colour_the_swatch_draws() {
        assert_eq!(hex_of([0.0, 0.0, 0.0, 1.0]), "#000000");
        assert_eq!(hex_of([1.0, 1.0, 1.0, 1.0]), "#FFFFFF");
        // Out-of-gamut values clamp rather than wrapping.
        assert_eq!(hex_of([2.0, -1.0, 0.5, 1.0]), "#FF0080");
    }
}

#[cfg(test)]
mod scalar_seed_tests {
    use super::*;

    /// A row seeded from anything but the asset commits an edit nobody made the moment it is
    /// touched.
    #[test]
    fn every_surface_row_opens_on_the_value_the_asset_holds() {
        let mut app = App::new();
        app.add_plugins((
            MinimalPlugins,
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
        ));
        app.init_asset::<StandardMaterial>();
        app.init_asset::<Font>();

        let material = StandardMaterial {
            metallic: 0.25,
            perceptual_roughness: 0.9,
            reflectance: 0.5,
            ior: 1.5,
            ..Default::default()
        };
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(material.clone());

        let body = app.world_mut().spawn(Node::default()).id();
        let id = app.world_mut().register_system(
            move |mut commands: Commands, materials: Res<Assets<StandardMaterial>>| {
                let material = materials.get(&handle).expect("material").clone();
                fill_surface_rows(&mut commands, body, &material, &handle);
            },
        );
        app.world_mut().run_system(id).expect("fill");
        app.world_mut().flush();

        let mut seeded: Vec<(f64, f32)> = app
            .world_mut()
            .query::<(&MaterialFieldBinding, &SliderValue)>()
            .iter(app.world())
            .map(|(binding, value)| (binding.shown, value.0))
            .collect();
        seeded.sort_by(|a, b| a.0.partial_cmp(&b.0).expect("ordered"));

        let wanted = [0.25f32, 0.5, 0.9, 1.5];
        assert_eq!(seeded.len(), wanted.len(), "one row per scalar field");
        for ((shown, value), want) in seeded.iter().zip(wanted.iter()) {
            assert!(
                (*value - want).abs() < 1e-5,
                "slider seeded at {value}, asset holds {want}",
            );
            assert!(
                (*shown as f32 - want).abs() < 1e-5,
                "binding remembers {shown}, asset holds {want}",
            );
        }
    }
}

#[cfg(test)]
mod scalar_commit_tests {
    use super::*;
    use bevy::asset::AssetPlugin;

    fn commit_app() -> App {
        let mut app = App::new();
        app.add_plugins((bevy::app::TaskPoolPlugin::default(), AssetPlugin::default()));
        app.init_asset::<StandardMaterial>();
        app.init_resource::<crate::asset_catalog::AssetCatalog>();
        app.add_observer(on_material_slider_commit);
        app
    }

    fn catalog_is_dirty(app: &App) -> bool {
        app.world()
            .resource::<crate::asset_catalog::AssetCatalog>()
            .dirty
    }

    /// A slider entity carrying the binding a spawned row would have.
    fn bound_slider(app: &mut App, handle: Handle<StandardMaterial>) -> Entity {
        app.world_mut()
            .spawn((
                MaterialFieldBinding {
                    material_handle: handle,
                    read_fn: |m| m.metallic as f64,
                    apply_fn: |m, v| m.metallic = v as f32,
                    shown: 0.25,
                    dragging: false,
                },
                SliderValue(0.25),
            ))
            .id()
    }

    fn drag_to(app: &mut App, slider: Entity, value: f32, is_final: bool) {
        app.world_mut().trigger(ValueChange::<f32> {
            source: slider,
            value,
            is_final,
        });
        app.world_mut().flush();
    }

    #[test]
    fn a_slider_value_reaches_the_material() {
        let mut app = commit_app();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let slider = bound_slider(&mut app, handle.clone());

        drag_to(&mut app, slider, 0.75, true);

        let material = app
            .world()
            .resource::<Assets<StandardMaterial>>()
            .get(&handle)
            .unwrap();
        assert!((material.metallic - 0.75).abs() < 1e-5);
        assert!(
            (app.world()
                .get::<MaterialFieldBinding>(slider)
                .unwrap()
                .shown
                - 0.75)
                .abs()
                < 1e-5,
            "the field remembers what it now shows",
        );
    }

    /// `FeathersSlider` does not self-manage `SliderValue`. Without the commit reflecting the
    /// value back the thumb would stand still under the drag.
    #[test]
    fn a_commit_moves_the_thumb_it_came_from() {
        let mut app = commit_app();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let slider = bound_slider(&mut app, handle);

        drag_to(&mut app, slider, 0.75, true);

        assert!((app.world().get::<SliderValue>(slider).expect("value").0 - 0.75).abs() < 1e-5,);
    }

    /// A drag writes every event, so the preview and viewport follow the gesture rather than
    /// jumping on release.
    #[test]
    fn a_value_mid_drag_reaches_the_material_too() {
        let mut app = commit_app();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let slider = bound_slider(&mut app, handle.clone());

        drag_to(&mut app, slider, 0.4, false);

        let metallic = app
            .world()
            .resource::<Assets<StandardMaterial>>()
            .get(&handle)
            .unwrap()
            .metallic;
        assert!((metallic - 0.4).abs() < 1e-5, "got {metallic}");
    }

    /// Dirtying the catalog schedules the material's file and `catalog.bsn` to be rewritten,
    /// and a drag emits an event per frame.
    #[test]
    fn a_value_mid_drag_leaves_the_catalog_clean() {
        let mut app = commit_app();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let slider = bound_slider(&mut app, handle.clone());

        for value in [0.3f32, 0.4, 0.5] {
            drag_to(&mut app, slider, value, false);
        }

        assert!(
            !catalog_is_dirty(&app),
            "an unfinished drag schedules no write",
        );
        let metallic = app
            .world()
            .resource::<Assets<StandardMaterial>>()
            .get(&handle)
            .unwrap()
            .metallic;
        assert!(
            (metallic - 0.5).abs() < 1e-5,
            "the asset still followed every frame, got {metallic}",
        );
    }

    #[test]
    fn the_end_of_a_drag_dirties_the_catalog() {
        let mut app = commit_app();
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let slider = bound_slider(&mut app, handle);

        drag_to(&mut app, slider, 0.4, false);
        assert!(!catalog_is_dirty(&app));

        drag_to(&mut app, slider, 0.5, true);
        assert!(catalog_is_dirty(&app), "the finished edit persists");
    }

    /// A drag can end without the final event that normally persists it: the pointer leaves
    /// the window, the widget is disabled mid-gesture, the gesture is cancelled.
    #[test]
    fn a_drag_that_ends_without_a_final_event_still_flags_the_catalog() {
        let mut app = commit_app();
        app.add_systems(Update, flush_material_slider_drag);
        let handle = app
            .world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial::default());
        let slider = bound_slider(&mut app, handle);
        app.world_mut()
            .entity_mut(slider)
            .insert(SliderDragState::default());

        set_dragging(&mut app, slider, true);
        drag_to(&mut app, slider, 0.4, false);
        drag_to(&mut app, slider, 0.6, false);
        app.update();
        assert!(
            !catalog_is_dirty(&app),
            "a drag in progress is not worth a pair of disk writes a frame",
        );

        set_dragging(&mut app, slider, false);
        app.update();
        assert!(catalog_is_dirty(&app), "the abandoned edit persists");

        app.world_mut()
            .resource_mut::<crate::asset_catalog::AssetCatalog>()
            .dirty = false;
        app.update();
        app.update();
        assert!(
            !catalog_is_dirty(&app),
            "the flush fires once per drag, not every frame after one",
        );
    }

    fn set_dragging(app: &mut App, slider: Entity, dragging: bool) {
        app.world_mut()
            .entity_mut(slider)
            .get_mut::<SliderDragState>()
            .expect("the slider carries its drag state")
            .dragging = dragging;
    }
}

#[cfg(test)]
mod refresh_tests {
    use super::*;
    use bevy::asset::AssetPlugin;

    fn refresh_app() -> App {
        let mut app = App::new();
        app.add_plugins((bevy::app::TaskPoolPlugin::default(), AssetPlugin::default()));
        app.init_asset::<StandardMaterial>();
        app.init_resource::<InputFocus>();
        app.add_systems(PostUpdate, refresh_material_rows.after(AssetEventSystems));
        app
    }

    fn metallic_material(app: &mut App, metallic: f32) -> Handle<StandardMaterial> {
        app.world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .add(StandardMaterial {
                metallic,
                ..Default::default()
            })
    }

    /// A slider carrying the binding a spawned row would have.
    fn scalar_row(app: &mut App, handle: Handle<StandardMaterial>, shown: f64) -> Entity {
        app.world_mut()
            .spawn((
                MaterialFieldBinding {
                    material_handle: handle,
                    read_fn: |m| m.metallic as f64,
                    apply_fn: |m, v| m.metallic = v as f32,
                    shown,
                    dragging: false,
                },
                SliderValue(shown as f32),
            ))
            .id()
    }

    fn slider_value(app: &App, slider: Entity) -> f32 {
        app.world().get::<SliderValue>(slider).expect("value").0
    }

    fn shown(app: &App, slider: Entity) -> f64 {
        app.world()
            .get::<MaterialFieldBinding>(slider)
            .expect("binding")
            .shown
    }

    /// A colour row entity with the swatch and hex it repaints.
    fn color_row(app: &mut App, handle: Handle<StandardMaterial>) -> (Entity, Entity) {
        let swatch = app
            .world_mut()
            .spawn(BackgroundColor(Color::srgb(0.0, 0.0, 0.0)))
            .id();
        let hex = app.world_mut().spawn(Text::new("#000000")).id();
        app.world_mut().spawn((
            ColorRowFace { swatch, hex },
            MaterialColorBinding {
                material_handle: handle,
                read_fn: base_color_srgb,
            },
        ));
        (swatch, hex)
    }

    /// Edit the material as another surface would.
    fn write_metallic(app: &mut App, handle: &Handle<StandardMaterial>, value: f32) {
        app.world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .get_mut(handle)
            .expect("material")
            .metallic = value;
    }

    /// With the same material open on several surfaces, a stale row would write its value
    /// back on its next commit and revert the edit made on the other surface.
    #[test]
    fn an_edit_made_elsewhere_reaches_an_unfocused_row() {
        let mut app = refresh_app();
        let handle = metallic_material(&mut app, 0.25);
        let slider = scalar_row(&mut app, handle.clone(), 0.25);

        write_metallic(&mut app, &handle, 0.9);
        app.update();

        assert!((slider_value(&app, slider) - 0.9).abs() < 1e-6);
        assert!(
            (shown(&app, slider) - 0.9).abs() < 1e-6,
            "the row remembers what it shows"
        );
    }

    #[test]
    fn an_edit_made_elsewhere_repaints_a_colour_row_face() {
        let mut app = refresh_app();
        let handle = metallic_material(&mut app, 0.25);
        let (swatch, hex) = color_row(&mut app, handle.clone());

        app.world_mut()
            .resource_mut::<Assets<StandardMaterial>>()
            .get_mut(&handle)
            .expect("material")
            .base_color = Color::srgb(1.0, 0.0, 0.5);
        app.update();

        assert_eq!(
            app.world().get::<Text>(hex).expect("hex").0.as_str(),
            "#FF0080",
        );
        let painted = app
            .world()
            .get::<BackgroundColor>(swatch)
            .expect("swatch")
            .0
            .to_srgba();
        assert!((painted.red - 1.0).abs() < 1e-4 && (painted.blue - 0.5).abs() < 1e-4);
    }

    /// Arrow keys move a focused slider; re-seeding it would fight that.
    #[test]
    fn the_row_holding_focus_is_left_alone() {
        let mut app = refresh_app();
        let handle = metallic_material(&mut app, 0.25);
        let slider = scalar_row(&mut app, handle.clone(), 0.25);
        app.world_mut()
            .resource_mut::<InputFocus>()
            .set(slider, bevy::input_focus::FocusCause::Navigated);

        write_metallic(&mut app, &handle, 0.9);
        app.update();

        assert!(
            (slider_value(&app, slider) - 0.25).abs() < 1e-6,
            "a focused row keeps its own value",
        );
        assert!((shown(&app, slider) - 0.25).abs() < 1e-6);
    }

    /// A drag writes the asset every event, raising `Modified` and running this refresh;
    /// answering it would snap the thumb out from under the gesture.
    #[test]
    fn the_row_being_dragged_is_left_alone() {
        let mut app = refresh_app();
        let handle = metallic_material(&mut app, 0.25);
        let slider = scalar_row(&mut app, handle.clone(), 0.25);
        let mut drag = SliderDragState::default();
        drag.dragging = true;
        app.world_mut().entity_mut(slider).insert(drag);

        write_metallic(&mut app, &handle, 0.9);
        app.update();

        assert!(
            (slider_value(&app, slider) - 0.25).abs() < 1e-6,
            "a dragged row is not moved under the gesture",
        );
        assert!((shown(&app, slider) - 0.25).abs() < 1e-6);
    }

    /// The refresh only reads: writing through `get_mut` would raise `Modified` again and the
    /// refresh would feed itself.
    #[test]
    fn a_refresh_writes_nothing_back_and_settles() {
        let mut app = refresh_app();
        let handle = metallic_material(&mut app, 0.25);
        let slider = scalar_row(&mut app, handle.clone(), 0.25);

        write_metallic(&mut app, &handle, 0.9);
        app.update();
        // Move the widget off what the refresh set, so a second refresh would be visible.
        app.world_mut().entity_mut(slider).insert(SliderValue(0.0));
        app.update();

        assert_eq!(
            slider_value(&app, slider),
            0.0,
            "the refresh raised no further event to answer",
        );
        let metallic = app
            .world()
            .resource::<Assets<StandardMaterial>>()
            .get(&handle)
            .expect("material")
            .metallic;
        assert!(
            (metallic - 0.9).abs() < 1e-6,
            "the refresh left the asset alone, got {metallic}",
        );
    }
}

#[cfg(test)]
mod focus_gate_tests {
    use super::*;

    /// Tab navigation gathers by `TabIndex` alone, so the hex entry and sliders of a closed
    /// picker would be tabbable into a subtree nothing draws.
    #[test]
    fn a_collapsed_picker_leaves_the_tab_order_and_rejoins_when_opened() {
        let mut app = App::new();
        app.add_systems(Update, gate_collapsed_color_picker_focus);

        let body = app
            .world_mut()
            .spawn((
                ColorPickerBody,
                Node {
                    display: Display::None,
                    ..Default::default()
                },
            ))
            .id();
        let control = app.world_mut().spawn((TabIndex(0), ChildOf(body))).id();

        app.update();
        assert!(
            app.world().get::<TabIndex>(control).expect("index").0 < 0,
            "a hidden control is skipped by the tab walk",
        );

        app.world_mut().entity_mut(body).insert(Node {
            display: Display::Flex,
            ..Default::default()
        });
        app.update();
        assert_eq!(app.world().get::<TabIndex>(control).expect("index").0, 0);
    }
}
