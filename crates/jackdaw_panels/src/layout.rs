use std::collections::HashMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::area::{ActiveDockWindow, DockArea, DockTabContent};

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct LayoutState {
    pub areas: HashMap<String, AreaState>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AreaState {
    pub windows: Vec<String>,
    /// Window id of the active tab in this area, or `None` for an
    /// empty area. The live model in [`crate::tree::DockTree`] keys
    /// the active tab by [`crate::tree::TabId`] so duplicates can
    /// coexist; this snapshot can't represent that distinction and
    /// is used only by legacy save / restore paths.
    pub active: Option<String>,
    pub size_ratio: f32,
}

impl Default for AreaState {
    fn default() -> Self {
        Self {
            windows: Vec::new(),
            active: None,
            size_ratio: 1.0,
        }
    }
}

/// Capture the current live layout as a `LayoutState` for serialization.
pub fn capture_layout_state(world: &mut World) -> LayoutState {
    let mut state = LayoutState::default();

    let mut area_query = world.query::<(
        Entity,
        &DockArea,
        Option<&ActiveDockWindow>,
        Option<&crate::Panel>,
    )>();
    // Walk every area, capturing its active `TabId` plus panel ratio.
    // The window-id resolution is deferred until we've collected the
    // content children for each area below.
    let areas: Vec<(Entity, String, Option<crate::tree::TabId>, f32)> = area_query
        .iter(world)
        .map(|(e, a, active, panel)| {
            (
                e,
                a.id.clone(),
                active.and_then(|a| a.0),
                panel.map(|p| p.ratio).unwrap_or(1.0),
            )
        })
        .collect();

    let mut content_query = world.query::<(&DockTabContent, &ChildOf)>();
    let all_content: Vec<(String, crate::tree::TabId, Entity)> = content_query
        .iter(world)
        .map(|(c, co)| (c.window_id.clone(), c.tab_id, co.parent()))
        .collect();

    for (area_entity, area_id, active_tab, ratio) in areas {
        let windows: Vec<String> = all_content
            .iter()
            .filter(|(_, _, p)| *p == area_entity)
            .map(|(w, _, _)| w.clone())
            .collect();
        // Resolve the active tab id back to a window id by matching
        // against the area's content children.
        let active = active_tab.and_then(|tid| {
            all_content
                .iter()
                .find(|(_, t, p)| *t == tid && *p == area_entity)
                .map(|(w, _, _)| w.clone())
        });
        state.areas.insert(
            area_id,
            AreaState {
                windows,
                active,
                size_ratio: ratio,
            },
        );
    }

    state
}
