//! Region-centric authoring: the region grid over the selected terrain,
//! which region is active, and how the rest of them draw.
//!
//! Everything here is view state. The active region and the visibility
//! mode decide what reaches the screen and nothing else: a brush lands
//! wherever the cursor is whether or not that region is active, and stored
//! heights, control words and the sidecar a save writes are untouched.
//! Hiding a region hides its triangles; `Base only` substitutes the default
//! control word on the way to the uploaded texture. Neither writes back, so
//! saving and exporting produce the same bytes under every mode.
//!
//! None of it is persisted or undoable, like the paint tool's channel view
//! (`channel_ops.rs`).

use std::borrow::Cow;

use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::keymap::PresetInput;
use jackdaw_terrain::{Control, GridRect, RegionCoord};

use super::sculpt::terrain_brush_hit;
use super::{TerrainDataStore, TerrainEditMode};
use crate::core_extension::CoreExtensionInputContext;
use crate::default_style;
use crate::selection::Selection;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<TerrainRegionView>().add_systems(
        Update,
        (region_pick_trigger, draw_region_grid)
            .chain()
            .run_if(in_state(crate::AppState::Editor)),
    );
}

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<TerrainToolRegionsOp>()
        .register_operator::<TerrainRegionSelectOp>()
        .register_operator::<TerrainRegionToggleGridOp>()
        .register_operator::<TerrainRegionVisibilityOp>();

    // Alt+9, the next free digit down the palette (see `ops.rs`).
    ctx.bind_operator::<CoreExtensionInputContext, TerrainToolRegionsOp>([PresetInput::key(
        "Digit9",
    )
    .alt()]);
}

// --- State ---

/// How the regions that are not active draw.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum RegionVisibility {
    /// Every region draws its own geometry and its own paint.
    #[default]
    Full,
    /// Only the active region is meshed. The others are filtered out at
    /// mesh-build time as an unallocated region is, so a hidden region
    /// costs its triangles and nothing else.
    Hidden,
    /// Every region draws, but only the active one draws its paint: the
    /// rest upload the default control word, so they show texture id 0.
    BaseOnly,
}

impl RegionVisibility {
    /// The bar's picker options, in the order they are offered.
    pub(crate) const ALL: [RegionVisibility; 3] = [
        RegionVisibility::Full,
        RegionVisibility::Hidden,
        RegionVisibility::BaseOnly,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            RegionVisibility::Full => "Full",
            RegionVisibility::Hidden => "Hidden",
            RegionVisibility::BaseOnly => "Base Only",
        }
    }

    /// The operator parameter value naming this mode. Part of the scripted
    /// contract, so it is spelled out rather than derived from the label.
    pub(crate) fn param(self) -> &'static str {
        match self {
            RegionVisibility::Full => "full",
            RegionVisibility::Hidden => "hidden",
            RegionVisibility::BaseOnly => "base",
        }
    }

    fn from_param(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|mode| mode.param() == value)
    }
}

/// Which region the author is working in and what that does to the others.
/// Editor state: never saved, never in the undo history.
#[derive(Resource, Clone, Debug)]
pub struct TerrainRegionView {
    /// Whether the region boundaries are drawn over the selected terrain.
    /// On by default.
    pub show_grid: bool,
    /// The terrain the active region belongs to, and which region it is.
    /// Carrying the entity keeps one terrain's active region from hiding
    /// another terrain's ground.
    pub active: Option<(Entity, RegionCoord)>,
    pub visibility: RegionVisibility,
}

impl Default for TerrainRegionView {
    fn default() -> Self {
        Self {
            show_grid: true,
            active: None,
            visibility: RegionVisibility::Full,
        }
    }
}

impl TerrainRegionView {
    /// The active region of `terrain`, if the active one is its.
    pub(crate) fn active_of(&self, terrain: Entity) -> Option<RegionCoord> {
        self.active
            .and_then(|(holder, coord)| (holder == terrain).then_some(coord))
    }

    /// Whether a region of `terrain` is meshed.
    ///
    /// Only `Hidden` answers no, and only once a region is active: with
    /// nothing active there is no non-active region to hide, so the terrain
    /// draws whole rather than blank.
    pub(crate) fn draws_geometry(&self, terrain: Entity, coord: RegionCoord) -> bool {
        match self.active_of(terrain) {
            Some(active) => self.visibility != RegionVisibility::Hidden || active == coord,
            None => true,
        }
    }

    /// What a terrain's uploaded control map is masked for.
    pub(crate) fn control_mask(&self, terrain: Entity) -> ControlMask {
        ControlMask {
            visibility: self.visibility,
            active: self.active_of(terrain),
        }
    }

    /// Whether the meshed geometry depends on which region is active, which
    /// decides whether a click moving the active region remeshes anything.
    pub(crate) fn geometry_key(&self) -> Option<Option<(Entity, RegionCoord)>> {
        (self.visibility == RegionVisibility::Hidden).then_some(self.active)
    }
}

// --- Control map masking ---

/// What one terrain's uploaded control map was built for, kept beside the
/// map so a mode change re-uploads it and nothing else does.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) struct ControlMask {
    visibility: RegionVisibility,
    active: Option<RegionCoord>,
}

impl ControlMask {
    fn substitutes(self) -> Option<RegionCoord> {
        match self.visibility {
            RegionVisibility::BaseOnly => self.active,
            _ => None,
        }
    }
}

/// The region a grid vertex belongs to.
///
/// The same floor division the store's addressing uses, so a vertex lands
/// in the region that holds it under every mode.
pub(crate) fn region_of(x: i32, z: i32, region_side: u32) -> RegionCoord {
    let side = region_side.max(1) as i32;
    RegionCoord::new(x.div_euclid(side), z.div_euclid(side))
}

/// The control words a terrain's map is uploaded from.
///
/// `Base only` substitutes the default word (texture id 0, no overlay, no
/// blend) for every cell outside the active region. The stored words are
/// borrowed, never written: a save, an export or the brush reads the
/// document, which this does not touch. Every other mode hands the stored
/// words straight back.
pub(crate) fn masked_control<'a>(
    mask: ControlMask,
    region_side: u32,
    resolution: u32,
    words: Cow<'a, [Control]>,
) -> Cow<'a, [Control]> {
    let Some(active) = mask.substitutes() else {
        return words;
    };
    if resolution == 0 {
        return words;
    }
    let mut masked = words.into_owned();
    let res = resolution as usize;
    for (index, word) in masked.iter_mut().enumerate() {
        let x = (index % res) as i32;
        let z = (index / res) as i32;
        if region_of(x, z, region_side) != active {
            *word = Control::default();
        }
    }
    Cow::Owned(masked)
}

/// [`masked_control`] over one rect of the grid, whose words are
/// row-major over the rect rather than over the terrain.
///
/// The stroke path does not have the whole grid in hand, since gathering it
/// is the cost the rect avoids, so the mask is applied to the block in
/// place from the block's own grid coordinates.
pub(crate) fn mask_control_block(
    mask: ControlMask,
    region_side: u32,
    rect: GridRect,
    words: &mut [Control],
) {
    let Some(active) = mask.substitutes() else {
        return;
    };
    let width = rect.width.max(1) as usize;
    for (index, word) in words.iter_mut().enumerate() {
        let x = rect.x as i32 + (index % width) as i32;
        let z = rect.z as i32 + (index / width) as i32;
        if region_of(x, z, region_side) != active {
            *word = Control::default();
        }
    }
}

// --- Viewport ---

/// LMB in Regions mode makes the region under the cursor active.
///
/// Goes through the operator rather than writing the resource here, so the
/// click and a scripted `terrain.region.select` take the same path.
fn region_pick_trigger(
    mouse: Res<ButtonInput<MouseButton>>,
    edit_mode: Res<TerrainEditMode>,
    vp: crate::viewport::ViewportCursor,
    terrains: Query<(Entity, &jackdaw_scene_types::Terrain, &GlobalTransform)>,
    selection: Res<Selection>,
    store: Res<TerrainDataStore>,
    mut commands: Commands,
) {
    if *edit_mode != TerrainEditMode::Regions || !mouse.just_pressed(MouseButton::Left) {
        return;
    }
    let Some(coord) = region_under_pointer(&vp, &terrains, &selection, &store) else {
        return;
    };
    commands
        .operator(TerrainRegionSelectOp::ID)
        .param("x", i64::from(coord.x))
        .param("z", i64::from(coord.z))
        .settings(CallOperatorSettings {
            creates_history_entry: false,
            execution_context: ExecutionContext::Invoke,
        })
        .call();
}

/// The region a viewport press lands in, or `None` when the press misses
/// the terrain: off the ground, or on UI floating over the viewport such as
/// the tool palette.
///
/// Shares [`terrain_brush_hit`] with the brushes, so a press the palette
/// swallows picks no region for the same reason it starts no stroke.
fn region_under_pointer(
    vp: &crate::viewport::ViewportCursor,
    terrains: &Query<(Entity, &jackdaw_scene_types::Terrain, &GlobalTransform)>,
    selection: &Selection,
    store: &TerrainDataStore,
) -> Option<RegionCoord> {
    let (entity, grid) = terrain_brush_hit(vp, terrains, selection, store)?;
    let (_, terrain, _) = terrains.get(entity).ok()?;
    Some(region_of(
        grid.x as i32,
        grid.y as i32,
        region_side(store, terrain).unwrap_or(1),
    ))
}

/// Cells per region edge for a terrain's document, `None` for one the
/// store has never minted.
pub(crate) fn region_side(
    store: &TerrainDataStore,
    terrain: &jackdaw_scene_types::Terrain,
) -> Option<u32> {
    store
        .get(&terrain.data_path)
        .map(|data| data.regions.region_size().get())
}

/// Draw region boundaries over the selected terrain, following the
/// sculpted surface the way the brush ring does: sampled height, lifted
/// clear of it, so the grid reads as painted on the ground.
fn draw_region_grid(
    view: Res<TerrainRegionView>,
    edit_mode: Res<TerrainEditMode>,
    selection: Res<Selection>,
    terrains: Query<(&jackdaw_scene_types::Terrain, &GlobalTransform)>,
    store: Res<TerrainDataStore>,
    mut gizmos: Gizmos,
) {
    if !view.show_grid || *edit_mode == TerrainEditMode::None {
        return;
    }
    let Some(entity) = selection.primary() else {
        return;
    };
    let Ok((terrain, transform)) = terrains.get(entity) else {
        return;
    };
    let Some(side) = region_side(&store, terrain) else {
        return;
    };
    // The overlay draws the regions the terrain holds, so it follows ground
    // as it is allocated rather than a declared rectangle.
    let shape = store.grid_shape(terrain);
    if shape.resolution < 2 || side == 0 {
        return;
    }

    let heightmap = store.heightmap(terrain);
    let cell = heightmap.map.cell_size();
    let origin = transform.translation();
    let last = (shape.resolution - 1) as f32;
    // One sample every few cells rather than every one: a boundary line
    // runs hundreds of cells and only has to hug the ground.
    let step = (shape.resolution / 128).max(1) as f32;

    // The lift scales with the footprint rather than taking the brush
    // ring's fixed 0.1: a kilometre-long line is seen at a grazing angle
    // for most of its length, where a constant lift loses the depth test.
    let lift = (shape.size.max_element() * LIFT_FRACTION).max(MIN_LIFT);
    let point = |gx: f32, gz: f32| {
        let height = heightmap.map.sample_bilinear(gx, gz);
        origin
            + Vec3::new(
                gx * cell.x + shape.origin.x,
                height + lift,
                gz * cell.y + shape.origin.y,
            )
    };
    let mut span = |from: Vec2, to: Vec2, color: Color| {
        let length = (to - from).length();
        let segments = (length / step).ceil().max(1.0);
        let mut previous = point(from.x, from.y);
        for i in 1..=(segments as u32) {
            let at = from.lerp(to, i as f32 / segments);
            let next = point(at.x, at.y);
            gizmos.line(previous, next, color);
            previous = next;
        }
    };

    let boundaries: Vec<f32> = std::iter::successors(Some(0u32), |at| Some(at + side))
        .take_while(|at| *at < shape.resolution)
        .map(|at| at as f32)
        .chain(std::iter::once(last))
        .collect();
    for at in &boundaries {
        span(Vec2::new(*at, 0.0), Vec2::new(*at, last), GRID);
        span(Vec2::new(0.0, *at), Vec2::new(last, *at), GRID);
    }

    let Some(active) = view.active_of(entity) else {
        return;
    };
    let min = Vec2::new(active.x as f32, active.z as f32) * side as f32;
    let max = (min + Vec2::splat(side as f32)).min(Vec2::splat(last));
    let min = min.max(Vec2::ZERO);
    if max.x <= min.x || max.y <= min.y {
        return;
    }
    // Drawn twice, a cell apart, because gizmo line width is global rather
    // than per-line: the doubled border reads as thicker without restyling
    // every other gizmo in the viewport.
    for inset in [0.0, ACTIVE_INSET] {
        let min = min + Vec2::splat(inset);
        let max = max - Vec2::splat(inset);
        span(min, Vec2::new(max.x, min.y), ACTIVE);
        span(Vec2::new(max.x, min.y), max, ACTIVE);
        span(max, Vec2::new(min.x, max.y), ACTIVE);
        span(Vec2::new(min.x, max.y), min, ACTIVE);
    }
}

/// How far above the sampled surface the grid sits, as a fraction of the
/// terrain's extent.
const LIFT_FRACTION: f32 = 0.002;
/// Floor for that lift, matching the brush ring's.
const MIN_LIFT: f32 = 0.1;
/// Cells between the active border's two passes.
const ACTIVE_INSET: f32 = 1.5;

const GRID: Color = default_style::TERRAIN_REGION_GRID;
const ACTIVE: Color = default_style::TERRAIN_REGION_ACTIVE;

// --- Operators ---

/// Pick the Regions tool, which brings up the region options bar (grid
/// toggle, visibility of the non-active regions) and makes a viewport click
/// choose the active region. Pressing again puts it away.
#[operator(
    id = "terrain.tool.regions",
    label = "Regions",
    description = "Work region by region: pick an active region and choose how the rest draw.",
    is_available = super::ops::has_selected_terrain
)]
pub(crate) fn terrain_tool_regions(
    _: In<OperatorParameters>,
    mut mode: ResMut<TerrainEditMode>,
) -> OperatorResult {
    *mode = if *mode == TerrainEditMode::Regions {
        TerrainEditMode::None
    } else {
        TerrainEditMode::Regions
    };
    OperatorResult::Finished
}

/// Make a region of the selected terrain active. A view choice, so it
/// leaves no history entry.
#[operator(
    id = "terrain.region.select",
    label = "Select Region",
    description = "Make a region of the selected terrain the one being worked in.",
    is_available = super::ops::has_selected_terrain,
    allows_undo = false,
    params(
        x(i64, doc = "Region column."),
        z(i64, doc = "Region row."),
    )
)]
pub(crate) fn terrain_region_select(
    params: In<OperatorParameters>,
    selection: Res<Selection>,
    terrains: Query<(), With<jackdaw_scene_types::Terrain>>,
    mut view: ResMut<TerrainRegionView>,
) -> OperatorResult {
    let entity = selection.primary().filter(|e| terrains.contains(*e))?;
    let x = i32::try_from(params.as_int("x")?).ok()?;
    let z = i32::try_from(params.as_int("z")?).ok()?;
    view.active = Some((entity, RegionCoord::new(x, z)));
    OperatorResult::Finished
}

/// Show or hide the region boundaries. View state, no history entry.
#[operator(
    id = "terrain.region.toggle_grid",
    label = "Region Grid",
    description = "Draw the region boundaries over the selected terrain.",
    allows_undo = false
)]
pub(crate) fn terrain_region_toggle_grid(
    _: In<OperatorParameters>,
    mut view: ResMut<TerrainRegionView>,
) -> OperatorResult {
    view.show_grid = !view.show_grid;
    OperatorResult::Finished
}

/// Choose how the regions that are not active draw. View state: no history
/// entry, and no change to what is stored.
#[operator(
    id = "terrain.region.visibility",
    label = "Region Visibility",
    description = "Choose how the regions that are not active draw.",
    allows_undo = false,
    params(mode(String, doc = "One of full, hidden, base."))
)]
pub(crate) fn terrain_region_visibility(
    params: In<OperatorParameters>,
    mut view: ResMut<TerrainRegionView>,
) -> OperatorResult {
    view.visibility = RegionVisibility::from_param(params.as_str("mode")?)?;
    OperatorResult::Finished
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: Entity = Entity::from_raw_u32(1).expect("a valid test entity");
    const B: Entity = Entity::from_raw_u32(2).expect("a valid test entity");

    fn view(
        visibility: RegionVisibility,
        active: Option<(Entity, RegionCoord)>,
    ) -> TerrainRegionView {
        TerrainRegionView {
            show_grid: true,
            active,
            visibility,
        }
    }

    #[test]
    fn a_vertex_lands_in_the_region_that_holds_it() {
        assert_eq!(region_of(0, 0, 256), RegionCoord::ORIGIN);
        assert_eq!(region_of(255, 255, 256), RegionCoord::ORIGIN);
        assert_eq!(region_of(256, 0, 256), RegionCoord::new(1, 0));
        assert_eq!(region_of(1023, 1023, 256), RegionCoord::new(3, 3));
    }

    /// Under Full every region draws, active or not.
    #[test]
    fn full_draws_every_region() {
        let view = view(RegionVisibility::Full, Some((A, RegionCoord::ORIGIN)));
        assert!(view.draws_geometry(A, RegionCoord::ORIGIN));
        assert!(view.draws_geometry(A, RegionCoord::new(2, 1)));
    }

    #[test]
    fn hidden_keeps_only_the_active_region() {
        let view = view(RegionVisibility::Hidden, Some((A, RegionCoord::new(1, 1))));
        assert!(view.draws_geometry(A, RegionCoord::new(1, 1)));
        assert!(!view.draws_geometry(A, RegionCoord::ORIGIN));
    }

    /// With nothing active there is no non-active region to hide, so the
    /// terrain draws whole rather than vanishing.
    #[test]
    fn hidden_with_no_active_region_draws_everything() {
        let view = view(RegionVisibility::Hidden, None);
        assert!(view.draws_geometry(A, RegionCoord::ORIGIN));
        assert!(view.draws_geometry(A, RegionCoord::new(9, 9)));
    }

    /// One terrain's active region says nothing about another's ground.
    #[test]
    fn hiding_one_terrains_regions_leaves_another_terrain_whole() {
        let view = view(RegionVisibility::Hidden, Some((A, RegionCoord::ORIGIN)));
        assert!(!view.draws_geometry(A, RegionCoord::new(1, 0)));
        assert!(view.draws_geometry(B, RegionCoord::new(1, 0)));
    }

    /// Every mode but `Base only` hands the stored words straight back, so
    /// nothing is copied or altered on the way.
    #[test]
    fn every_mode_but_base_only_passes_the_stored_words_through() {
        let words = vec![Control::default().with_base_id(3); 16];
        for visibility in [RegionVisibility::Full, RegionVisibility::Hidden] {
            let mask = view(visibility, Some((A, RegionCoord::ORIGIN))).control_mask(A);
            let out = masked_control(mask, 2, 4, Cow::Borrowed(&words));
            assert!(matches!(out, Cow::Borrowed(_)), "{visibility:?} copied");
            assert_eq!(out.as_ref(), words.as_slice());
        }
    }

    /// `Base only` substitutes the default word outside the active
    /// region and leaves the active region's own paint alone.
    #[test]
    fn base_only_substitutes_the_default_word_outside_the_active_region() {
        let words = vec![Control::default().with_base_id(3); 16];
        let mask = view(RegionVisibility::BaseOnly, Some((A, RegionCoord::ORIGIN))).control_mask(A);
        let out = masked_control(mask, 2, 4, Cow::Borrowed(&words));

        // Region (0, 0) is the top-left 2x2 of a 4x4 grid.
        for (index, word) in out.iter().enumerate() {
            let (x, z) = (index % 4, index / 4);
            let inside = x < 2 && z < 2;
            assert_eq!(
                *word,
                if inside {
                    Control::default().with_base_id(3)
                } else {
                    Control::default()
                },
                "cell ({x}, {z})",
            );
        }
        assert_eq!(
            words,
            vec![Control::default().with_base_id(3); 16],
            "the stored words must be left as they were",
        );
    }

    /// With nothing active, `Base only` has no region to keep the paint
    /// of, so it changes nothing.
    #[test]
    fn base_only_with_no_active_region_changes_nothing() {
        let words = vec![Control::default().with_base_id(3); 16];
        let mask = view(RegionVisibility::BaseOnly, None).control_mask(A);
        let out = masked_control(mask, 2, 4, Cow::Borrowed(&words));
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    /// Moving the active region remeshes only under `Hidden`; the other
    /// modes' geometry does not depend on it.
    #[test]
    fn only_hidden_makes_the_geometry_depend_on_the_active_region() {
        assert_eq!(
            view(RegionVisibility::Full, Some((A, RegionCoord::ORIGIN))).geometry_key(),
            None,
        );
        assert_eq!(
            view(RegionVisibility::BaseOnly, Some((A, RegionCoord::ORIGIN))).geometry_key(),
            None,
        );
        assert_eq!(
            view(RegionVisibility::Hidden, Some((A, RegionCoord::ORIGIN))).geometry_key(),
            Some(Some((A, RegionCoord::ORIGIN))),
        );
    }

    #[test]
    fn every_visibility_mode_round_trips_through_its_parameter() {
        for mode in RegionVisibility::ALL {
            assert_eq!(RegionVisibility::from_param(mode.param()), Some(mode));
        }
        assert_eq!(RegionVisibility::from_param("nonsense"), None);
    }
}

/// Which region a viewport press makes active, and which press makes
/// none.
#[cfg(test)]
mod pointer_tests {
    use super::*;
    use crate::terrain::pointer_harness;

    /// Where the last run of [`probe_region`] landed.
    #[derive(Resource, Default)]
    struct RegionProbe(Option<RegionCoord>);

    fn probe_region(
        vp: crate::viewport::ViewportCursor,
        terrains: Query<(Entity, &jackdaw_scene_types::Terrain, &GlobalTransform)>,
        selection: Res<Selection>,
        store: Res<TerrainDataStore>,
        mut probe: ResMut<RegionProbe>,
    ) {
        probe.0 = region_under_pointer(&vp, &terrains, &selection, &store);
    }

    fn regions_app() -> App {
        let mut app = pointer_harness::app(16);
        app.init_resource::<RegionProbe>()
            .insert_resource(TerrainEditMode::Regions)
            .add_systems(Update, probe_region);
        app
    }

    /// A press on open ground names the region under the cursor, which the
    /// click makes active.
    #[test]
    fn a_press_on_open_ground_names_a_region() {
        let mut app = regions_app();
        app.update();
        assert!(
            app.world().resource::<RegionProbe>().0.is_some(),
            "a press over the viewport's own node must name a region",
        );
    }

    /// The palette floats over the viewport, so a press on one of its
    /// buttons names no region.
    #[test]
    fn a_press_on_the_tool_palette_names_no_region() {
        let mut app = regions_app();
        pointer_harness::hover_overlay(&mut app);
        app.update();
        assert_eq!(
            app.world().resource::<RegionProbe>().0,
            None,
            "clicking a palette button must leave the active region alone",
        );
    }
}
