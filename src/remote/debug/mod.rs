//! Native remote-game debugger: read-only introspection panels that inspect a
//! running Bevy game over BRP, built on the existing `super` connection client.
//!
//! `RemoteDebugPlugin` wires the shared pieces (the sparkline material, the
//! graph edge material, and the poll helper) plus the Diagnostics view. Later
//! views hang off the same plugin.

pub mod archetypes;
pub mod depgraph;
pub mod diagnostics;
pub mod graph;
pub mod poll;
pub mod queries;
pub mod relationships;
pub mod schedules;
pub mod sparkline;
pub mod style;

use bevy::asset::embedded_asset;
use bevy::prelude::*;

/// Registers the debugger's shared rendering and the Diagnostics view.
pub struct RemoteDebugPlugin;

impl Plugin for RemoteDebugPlugin {
    fn build(&self, app: &mut App) {
        embedded_asset!(app, "shaders/sparkline.wgsl");
        embedded_asset!(app, "shaders/graph_edge.wgsl");

        app.add_plugins(UiMaterialPlugin::<sparkline::SparklineMaterial>::default());
        app.add_plugins(UiMaterialPlugin::<graph::GraphEdgeMaterial>::default());
        app.add_plugins(poll::BrpPollPlugin::<diagnostics::DiagnosticsSample>::new(
            "jackdaw/diagnostics",
            0.25,
        ));

        app.init_resource::<diagnostics::DiagBuffers>();
        app.add_systems(
            Update,
            diagnostics::update_diagnostics_panel.run_if(diagnostics::connected),
        );

        app.init_resource::<queries::QuerySpec>();
        app.init_resource::<queries::QueryResults>();
        app.init_resource::<queries::QueryPollTimer>();
        app.add_systems(
            Update,
            (
                queries::query_poll,
                queries::poll_query_task,
                queries::rebuild_query_chips,
                queries::rebuild_query_results,
            ),
        );
        app.add_observer(queries::on_add_filter_clicked);
        app.add_observer(queries::on_filter_chip_clicked);

        app.add_plugins(poll::BrpPollPlugin::<archetypes::ArchetypesReply>::new(
            "jackdaw/archetypes",
            0.5,
        ));
        app.init_resource::<archetypes::ArchetypeSort>();
        app.add_systems(Update, archetypes::rebuild_archetypes);
        app.add_observer(archetypes::on_sort_header_clicked);

        app.add_plugins(poll::BrpPollPlugin::<schedules::SchedulesReply>::new(
            "jackdaw/schedules",
            1.0,
        ));
        app.add_systems(Update, schedules::rebuild_schedules);

        app.init_resource::<depgraph::SelectedSchedule>();
        app.add_systems(Update, depgraph::rebuild_depgraph);
        app.add_systems(
            PostUpdate,
            graph::sync_graph_edges.after(bevy::ui::UiSystems::Layout),
        );
        app.add_observer(depgraph::on_schedule_button_clicked);

        app.add_plugins(
            poll::BrpPollPlugin::<relationships::RelationshipsReply>::new(
                "jackdaw/scene_snapshot",
                1.5,
            ),
        );
        app.add_systems(Update, relationships::rebuild_relationships);
    }
}
