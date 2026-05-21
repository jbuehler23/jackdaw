use bevy_ecs::prelude::*;
use bevy_math::prelude::*;
use bevy_picking::events::{DragEnd, DragStart, Press, Release};
use bevy_picking::prelude::*;
use bevy_ui::prelude::*;

use bevy_ui::UiGlobalTransform;

use super::{
    AlphaSlider, ColorPickerChangeEvent, ColorPickerCommitEvent, ColorPickerState, HsvRectangle,
    HueSlider,
};

#[derive(Component, Default)]
pub(super) struct Dragging;

pub(super) trait PickerControl: Component {
    fn picker_entity(&self) -> Entity;
    fn update_state(&self, state: &mut ColorPickerState, normalized: Vec2);
}

impl PickerControl for HsvRectangle {
    fn picker_entity(&self) -> Entity {
        self.0
    }

    fn update_state(&self, state: &mut ColorPickerState, normalized: Vec2) {
        state.saturation = (normalized.x + 0.5).clamp(0.0, 1.0);
        state.brightness = (0.5 - normalized.y).clamp(0.0, 1.0);
    }
}

impl PickerControl for HueSlider {
    fn picker_entity(&self) -> Entity {
        self.0
    }

    fn update_state(&self, state: &mut ColorPickerState, normalized: Vec2) {
        state.hue = (normalized.x + 0.5).clamp(0.0, 1.0) * 360.0;
    }
}

impl PickerControl for AlphaSlider {
    fn picker_entity(&self) -> Entity {
        self.0
    }

    fn update_state(&self, state: &mut ColorPickerState, normalized: Vec2) {
        state.alpha = (normalized.x + 0.5).clamp(0.0, 1.0);
    }
}

pub(super) fn on_control_press<C: PickerControl>(
    event: On<Pointer<Press>>,
    mut commands: Commands,
    controls: Query<(&C, &ComputedNode, &UiGlobalTransform)>,
    mut pickers: Query<&mut ColorPickerState>,
) {
    let Ok((control, computed, ui_transform)) = controls.get(event.event_target()) else {
        return;
    };
    let picker_entity = control.picker_entity();

    let cursor_pos = event.pointer_location.position / computed.inverse_scale_factor;
    let Some(normalized) = computed.normalize_point(*ui_transform, cursor_pos) else {
        return;
    };

    let Ok(mut state) = pickers.get_mut(picker_entity) else {
        return;
    };

    control.update_state(&mut state, normalized);

    commands.trigger(ColorPickerChangeEvent {
        entity: picker_entity,
        color: state.to_rgba(),
    });
}

pub(super) fn on_control_release<C: PickerControl>(
    event: On<Pointer<Release>>,
    mut commands: Commands,
    controls: Query<&C, Without<Dragging>>,
    pickers: Query<&ColorPickerState>,
) {
    let Ok(control) = controls.get(event.event_target()) else {
        return;
    };
    let picker_entity = control.picker_entity();

    if let Ok(state) = pickers.get(picker_entity) {
        commands.trigger(ColorPickerCommitEvent {
            entity: picker_entity,
            color: state.to_rgba(),
        });
    }
}

pub(super) fn on_control_drag_start<C: PickerControl>(
    event: On<Pointer<DragStart>>,
    mut commands: Commands,
    controls: Query<(&C, &ComputedNode, &UiGlobalTransform)>,
    mut pickers: Query<&mut ColorPickerState>,
) {
    let Ok((control, computed, ui_transform)) = controls.get(event.event_target()) else {
        return;
    };
    let picker_entity = control.picker_entity();

    commands.entity(event.event_target()).insert(Dragging);

    let cursor_pos = event.pointer_location.position / computed.inverse_scale_factor;
    let Some(normalized) = computed.normalize_point(*ui_transform, cursor_pos) else {
        return;
    };

    let Ok(mut state) = pickers.get_mut(picker_entity) else {
        return;
    };

    control.update_state(&mut state, normalized);

    commands.trigger(ColorPickerChangeEvent {
        entity: picker_entity,
        color: state.to_rgba(),
    });
}

pub(super) fn on_control_drag<C: PickerControl>(
    event: On<Pointer<Drag>>,
    mut commands: Commands,
    controls: Query<(&C, &ComputedNode, &UiGlobalTransform), With<Dragging>>,
    mut pickers: Query<&mut ColorPickerState>,
) {
    let Ok((control, computed, ui_transform)) = controls.get(event.event_target()) else {
        return;
    };
    let picker_entity = control.picker_entity();

    let cursor_pos = event.pointer_location.position / computed.inverse_scale_factor;
    let Some(normalized) = computed.normalize_point(*ui_transform, cursor_pos) else {
        return;
    };

    let Ok(mut state) = pickers.get_mut(picker_entity) else {
        return;
    };

    control.update_state(&mut state, normalized);

    commands.trigger(ColorPickerChangeEvent {
        entity: picker_entity,
        color: state.to_rgba(),
    });
}

pub(super) fn on_control_drag_end<C: PickerControl>(
    event: On<Pointer<DragEnd>>,
    mut commands: Commands,
    controls: Query<&C>,
    pickers: Query<&ColorPickerState>,
) {
    let Ok(control) = controls.get(event.event_target()) else {
        return;
    };
    let picker_entity = control.picker_entity();

    commands.entity(event.event_target()).remove::<Dragging>();

    if let Ok(state) = pickers.get(picker_entity) {
        commands.trigger(ColorPickerCommitEvent {
            entity: picker_entity,
            color: state.to_rgba(),
        });
    }
}
