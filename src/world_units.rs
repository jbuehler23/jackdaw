use std::path::PathBuf;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use serde::{Deserialize, Serialize};

use jackdaw_scene_types::Scene2dRoot;

use crate::inspector::{Inspector, InspectorDirty, InspectorTarget};
use crate::project::ProjectRoot;
use crate::project_settings::{Section, load_section, store_section};

/// The settings-file key the 2D world's units live under.
const UNITS_SECTION: &str = "units";

/// The pixels per unit the 2D view's Snap menu offers.
pub const PPU_PRESETS: [f32; 6] = [1.0, 8.0, 16.0, 32.0, 64.0, 100.0];

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<WorldUnits>().add_systems(
        Update,
        (
            load_project_units,
            rebuild_inspectors.run_if(resource_changed::<WorldUnits>),
        )
            .chain(),
    );
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ProjectSetPixelsPerUnitOp>()
        .register_operator::<ProjectSetLengthUnitOp>();
}

/// The unit the editor shows a 2D world scene's lengths in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LengthUnit {
    #[default]
    Pixels,
    Units,
}

impl LengthUnit {
    pub const ALL: [Self; 2] = [Self::Pixels, Self::Units];

    /// The name `project.set_length_unit` takes.
    pub fn id(self) -> &'static str {
        match self {
            Self::Pixels => "pixels",
            Self::Units => "units",
        }
    }

    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|unit| unit.id() == id)
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Pixels => "Pixels",
            Self::Units => "World Units",
        }
    }

    pub fn suffix(self) -> &'static str {
        match self {
            Self::Pixels => "px",
            Self::Units => "u",
        }
    }
}

/// How the editor reads a 2D world scene's lengths: how many sprite pixels
/// make one world unit, and which of the two it shows. An editor-only project
/// setting; the scene and the game only ever hold world units.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorldUnits {
    pub pixels_per_unit: f32,
    pub display: LengthUnit,
}

impl Default for WorldUnits {
    fn default() -> Self {
        Self {
            pixels_per_unit: 1.0,
            display: LengthUnit::Pixels,
        }
    }
}

impl WorldUnits {
    /// The pixels per unit, `1.0` for one that is not a finite number above 0.
    pub fn ppu(&self) -> f32 {
        if self.pixels_per_unit.is_finite() && self.pixels_per_unit > 0.0 {
            self.pixels_per_unit
        } else {
            1.0
        }
    }

    /// One pixel, in world units: the lattice a pixel snap lands on.
    pub fn pixel_size(&self) -> f32 {
        1.0 / self.ppu()
    }

    /// Shown units per world unit.
    pub fn shown_per_unit(&self) -> f32 {
        match self.display {
            LengthUnit::Pixels => self.ppu(),
            LengthUnit::Units => 1.0,
        }
    }

    /// A world length as the editor writes it, with its unit.
    pub fn label(&self, world_length: f32) -> String {
        let shown = (world_length * self.shown_per_unit() * 1000.0).round() / 1000.0;
        format!("{shown} {}", self.display.suffix())
    }
}

/// The lengths the inspector shows in the project's length unit, as (component
/// type path, field path). Each is a world length on an entity in a 2D world
/// scene. A sprite's `rect` is not here: it counts texture pixels already.
pub const LENGTH_FIELDS: [(&str, &str); 4] = [
    (TRANSFORM, "translation.x"),
    (TRANSFORM, "translation.y"),
    (SPRITE, "custom_size.0.x"),
    (SPRITE, "custom_size.0.y"),
];

const TRANSFORM: &str = "bevy_transform::components::transform::Transform";
const SPRITE: &str = "bevy_sprite::sprite::Sprite";

/// Whether the inspector shows `field_path` of `type_path` in the project's
/// length unit.
pub fn is_length_field(type_path: &str, field_path: &str) -> bool {
    LENGTH_FIELDS.contains(&(type_path, field_path))
}

/// Shown units per world unit of a length on `entity`: the project's while it
/// sits under an open 2D world root, else 1.
pub fn length_scale(world: &World, entity: Entity) -> f32 {
    let mut top = entity;
    while let Some(parent) = world.get::<ChildOf>(top) {
        top = parent.parent();
    }
    if top != entity && world.get::<Scene2dRoot>(top).is_some() {
        world.resource::<WorldUnits>().shown_per_unit()
    } else {
        1.0
    }
}

/// Read the open project's units whenever the project opens, changes or
/// closes; with none open they are the defaults.
fn load_project_units(
    project: Option<Res<ProjectRoot>>,
    mut units: ResMut<WorldUnits>,
    mut loaded_root: Local<Option<PathBuf>>,
) {
    let root = project.map(|project| project.root.clone());
    if *loaded_root == root {
        return;
    }
    *units = match &root {
        Some(root) => load_section(root, Section::Key(UNITS_SECTION)),
        None => WorldUnits::default(),
    };
    *loaded_root = root;
}

/// Rebuild every inspector, so its length fields read in the units now set.
fn rebuild_inspectors(
    inspectors: Query<&InspectorTarget, With<Inspector>>,
    mut commands: Commands,
) {
    for target in &inspectors {
        commands.entity(target.0).try_insert(InspectorDirty);
    }
}

fn store(units: &WorldUnits, project: Option<&ProjectRoot>) {
    if let Some(project) = project {
        store_section(&project.root, Section::Key(UNITS_SECTION), units);
    }
}

/// Set how many sprite pixels make one world unit in the 2D view. The editor
/// reads lengths, snaps pixels and sizes sprites by it; the scene keeps world
/// units. A project setting, so no history entry.
#[operator(
    id = "project.set_pixels_per_unit",
    label = "Set Pixels Per Unit",
    description = "Set how many sprite pixels make one world unit in the 2D view's rulers, \
                   inspector and pixel snap. Editor-only: the game never sees it.",
    allows_undo = false,
    params(value(f64, doc = "Pixels per world unit, above 0. 1 by default."))
)]
pub(crate) fn project_set_pixels_per_unit(
    params: In<OperatorParameters>,
    mut units: ResMut<WorldUnits>,
    project: Option<Res<ProjectRoot>>,
) -> OperatorResult {
    let Some(value) = params
        .as_float("value")
        .map(|value| value as f32)
        .filter(|value| value.is_finite() && *value > 0.0)
    else {
        warn!("project.set_pixels_per_unit: 'value' must be a number above 0");
        return OperatorResult::Cancelled;
    };
    units.pixels_per_unit = value;
    store(&units, project.as_deref());
    OperatorResult::Finished
}

/// Show the 2D view's lengths in pixels or in world units. A project setting,
/// so no history entry.
#[operator(
    id = "project.set_length_unit",
    label = "Set Length Unit",
    description = "Show 2D world lengths in pixels or in world units.",
    allows_undo = false,
    params(unit(String, doc = "`pixels` (the default) or `units`."))
)]
pub(crate) fn project_set_length_unit(
    params: In<OperatorParameters>,
    mut units: ResMut<WorldUnits>,
    project: Option<Res<ProjectRoot>>,
) -> OperatorResult {
    let Some(unit) = params.as_str("unit").and_then(LengthUnit::parse) else {
        warn!("project.set_length_unit: 'unit' must be `pixels` or `units`");
        return OperatorResult::Cancelled;
    };
    units.display = unit;
    store(&units, project.as_deref());
    OperatorResult::Finished
}
