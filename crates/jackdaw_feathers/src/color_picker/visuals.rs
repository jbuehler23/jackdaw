use bevy_asset::prelude::*;
use bevy_color::prelude::*;
use bevy_ecs::prelude::*;
use bevy_math::prelude::*;
use bevy_ui::prelude::*;
use bevy_ui_render::prelude::*;

use super::color_math::hsv_to_rgb;
use super::materials::{AlphaSliderMaterial, CheckerboardMaterial, HsvRectMaterial};
use super::{
    AlphaHandle, AlphaHandleMaterial, AlphaMaterialNode, AlphaSlider, ColorPickerState,
    HANDLE_SIZE, HsvRectHandle, HsvRectMaterialNode, HsvRectangle, HueHandle, HueSlider,
    PreviewSwatchMaterial, TriggerLabel, TriggerSwatchMaterial,
};

pub(super) fn update_color_picker_visuals(
    changed_pickers: Query<Entity, Changed<ColorPickerState>>,
    all_pickers: Query<&ColorPickerState>,
    mut hsv_rect_handles: Query<
        (&HsvRectHandle, &mut Node, &mut BackgroundColor),
        (Without<HueHandle>, Without<AlphaHandle>),
    >,
    mut hue_handles: Query<
        (&HueHandle, &mut Node, &mut BackgroundColor),
        (Without<HsvRectHandle>, Without<AlphaHandle>),
    >,
    mut alpha_handles: Query<
        (&AlphaHandle, &mut Node),
        (Without<HsvRectHandle>, Without<HueHandle>),
    >,
    alpha_handle_material_nodes: Query<(&AlphaHandleMaterial, &MaterialNode<CheckerboardMaterial>)>,
    hsv_rect_material_nodes: Query<(&HsvRectMaterialNode, &MaterialNode<HsvRectMaterial>)>,
    alpha_material_nodes: Query<(&AlphaMaterialNode, &MaterialNode<AlphaSliderMaterial>)>,
    preview_swatch_nodes: Query<(&PreviewSwatchMaterial, &MaterialNode<CheckerboardMaterial>)>,
    hsv_rects: Query<(&HsvRectangle, &ComputedNode), Changed<ComputedNode>>,
    all_hsv_rects: Query<(&HsvRectangle, &ComputedNode)>,
    hue_sliders: Query<(&HueSlider, &ComputedNode)>,
    alpha_sliders: Query<(&AlphaSlider, &ComputedNode)>,
    mut hsv_rect_materials: ResMut<Assets<HsvRectMaterial>>,
    mut alpha_materials: ResMut<Assets<AlphaSliderMaterial>>,
    mut checkerboard_materials: ResMut<Assets<CheckerboardMaterial>>,
) {
    let mut needs_update = Vec::new();
    for entity in &changed_pickers {
        if let Ok(state) = all_pickers.get(entity) {
            needs_update.push((entity, state));
        }
    }
    for (rect, _) in &hsv_rects {
        if !needs_update.iter().any(|(e, _)| *e == rect.0)
            && let Ok(state) = all_pickers.get(rect.0)
        {
            needs_update.push((rect.0, state));
        }
    }

    for (picker_entity, state) in needs_update {
        let clamped_rgba = state.to_rgba().map(|c| c.clamp(0.0, 1.0));
        let current_color = Srgba::new(
            clamped_rgba[0],
            clamped_rgba[1],
            clamped_rgba[2],
            clamped_rgba[3],
        );

        let hsv_size = all_hsv_rects
            .iter()
            .find(|(r, _)| r.0 == picker_entity)
            .map(|(_, c)| c.size() * c.inverse_scale_factor());
        let hue_size = hue_sliders
            .iter()
            .find(|(s, _)| s.0 == picker_entity)
            .map(|(_, c)| c.size() * c.inverse_scale_factor());
        let alpha_size = alpha_sliders
            .iter()
            .find(|(s, _)| s.0 == picker_entity)
            .map(|(_, c)| c.size() * c.inverse_scale_factor());

        for (hsv_rect_handle, mut node, mut bg) in &mut hsv_rect_handles {
            if hsv_rect_handle.0 != picker_entity {
                continue;
            }
            if let Some(size) = hsv_size
                && size.x > 0.0
                && size.y > 0.0
            {
                node.left = px(state.saturation * size.x - HANDLE_SIZE / 2.0);
                node.top = px((1.0 - state.brightness.min(1.0)) * size.y - HANDLE_SIZE / 2.0);
            }
            bg.0 = current_color.with_alpha(1.0).into();
        }

        for (hue_handle, mut node, mut bg) in &mut hue_handles {
            if hue_handle.0 != picker_entity {
                continue;
            }
            if let Some(size) = hue_size
                && size.x > 0.0
            {
                node.left = px((state.hue / 360.0) * size.x - HANDLE_SIZE / 2.0);
            }
            let hue_color = hsv_to_rgb(state.hue, 1.0, 1.0);
            bg.0 = Srgba::new(hue_color.0, hue_color.1, hue_color.2, 1.0).into();
        }

        for (alpha_handle, mut node) in &mut alpha_handles {
            if alpha_handle.0 != picker_entity {
                continue;
            }
            if let Some(size) = alpha_size
                && size.x > 0.0
            {
                node.left = px(state.alpha * size.x - HANDLE_SIZE / 2.0);
            }
        }

        for (alpha_handle_mat, material_node) in &alpha_handle_material_nodes {
            if alpha_handle_mat.0 != picker_entity {
                continue;
            }
            if let Some(material) = checkerboard_materials.get_mut(&material_node.0) {
                material.color = Vec4::new(
                    current_color.red,
                    current_color.green,
                    current_color.blue,
                    current_color.alpha,
                );
            }
        }

        for (preview_mat, material_node) in &preview_swatch_nodes {
            if preview_mat.0 != picker_entity {
                continue;
            }
            if let Some(material) = checkerboard_materials.get_mut(&material_node.0) {
                material.color = Vec4::new(
                    current_color.red,
                    current_color.green,
                    current_color.blue,
                    current_color.alpha,
                );
            }
        }

        for (hsv_rect_mat_node, material_node) in &hsv_rect_material_nodes {
            if hsv_rect_mat_node.0 != picker_entity {
                continue;
            }
            if let Some(material) = hsv_rect_materials.get_mut(&material_node.0) {
                material.hue = state.hue;
            }
        }

        for (alpha_mat_node, material_node) in &alpha_material_nodes {
            if alpha_mat_node.0 != picker_entity {
                continue;
            }
            if let Some(material) = alpha_materials.get_mut(&material_node.0) {
                let (r, g, b) = hsv_to_rgb(state.hue, state.saturation, state.brightness.min(1.0));
                material.color = Vec4::new(r, g, b, 1.0);
            }
        }
    }
}

pub(super) fn update_trigger_display(
    pickers: Query<(Entity, &ColorPickerState), Changed<ColorPickerState>>,
    trigger_swatch_materials: Query<(&TriggerSwatchMaterial, &MaterialNode<CheckerboardMaterial>)>,
    mut trigger_labels: Query<(&TriggerLabel, &mut Text)>,
    mut checkerboard_materials: ResMut<Assets<CheckerboardMaterial>>,
) {
    for (picker_entity, state) in &pickers {
        let srgba = state.to_srgba();
        let hex = state.to_hex();

        for (swatch_mat, material_node) in &trigger_swatch_materials {
            if swatch_mat.0 != picker_entity {
                continue;
            }
            if let Some(material) = checkerboard_materials.get_mut(&material_node.0) {
                material.color = Vec4::new(srgba.red, srgba.green, srgba.blue, srgba.alpha);
            }
        }

        for (label, mut text) in &mut trigger_labels {
            if label.0 != picker_entity {
                continue;
            }
            **text = hex.clone();
        }
    }
}
