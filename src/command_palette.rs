use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::{OperatorAction, OperatorEntity};
use jackdaw_feathers::icons::EditorFont;
use jackdaw_feathers::picker::{
    Category, Matchable, PickerItems, PickerProps, SelectInput, SpawnItemInput, match_text,
    picker_item,
};
use jackdaw_feathers::tokens;
use jackdaw_feathers::tooltip::Tooltip;

use crate::core_extension::CoreExtensionInputContext;
use crate::creation_taxonomy::{CreationTaxonomy, WIDGET_ACTION_PREFIX};
use crate::operator_tooltip::display_keybind;

#[derive(Component)]
struct CommandPalette;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ToggleCommandPaletteOp>();
    ctx.register_menu_entry::<ToggleCommandPaletteOp>(TopLevelMenu::Tools);

    // Deferred: condition is not bare Press::default() (modifier chord + key).
    ctx.entity_mut()
        .with_related::<ActionOf<CoreExtensionInputContext>>((
            Action::<ToggleCommandPaletteOp>::new(),
            bindings![
                (
                    KeyCode::Space.with_mod_keys(ModKeys::CONTROL),
                    bevy_enhanced_input::prelude::Press::default()
                ),
                (KeyCode::F3, bevy_enhanced_input::prelude::Press::default())
            ],
        ));
}

#[operator(
    id = "command_palette.toggle",
    label = "Toggle command palette",
    is_available = no_modal_active,
    allows_undo = false
)]
pub(crate) fn toggle_command_palette(
    _: In<OperatorParameters>,
    // need world access to run the availability checks :(
    world: &mut World,
    existing_query: &mut QueryState<Entity, With<CommandPalette>>,
) -> OperatorResult {
    let mut any_existing = false;

    for existing in existing_query.query(world).iter().collect::<Vec<_>>() {
        world.entity_mut(existing).despawn();
        any_existing = true;
    }

    if any_existing {
        return OperatorResult::Finished;
    }

    let mut entries = match world.run_system_cached(get_operators) {
        Ok(ops) => ops,
        Err(e) => {
            error!("Couldn't get the available operators: {e}");
            return OperatorResult::Finished;
        }
    };
    // One read of the vocabulary serves both passes below: which operators are
    // creation commands, and which creation entries no operator backs.
    let taxonomy = CreationTaxonomy::collect(world);
    let categories = creation_categories(&taxonomy);
    for entry in &mut entries {
        let PaletteAction::Operator(id) = entry.action else {
            continue;
        };
        if let Some(category) = categories.get(&format!("op:{id}")) {
            entry.category = category.clone();
        }
    }
    entries.extend(creation_entries(&taxonomy));

    let props = PickerProps::new(spawn_item, on_select)
        .items(entries)
        .title("Command Palette");

    world.spawn((
        props,
        CommandPalette,
        crate::BlocksCameraInput,
        crate::EditorEntity,
    ));

    OperatorResult::Finished
}

fn no_modal_active(active: ActiveModalQuery) -> bool {
    !active.is_modal_running()
}

fn get_operators(
    world: &mut World,
    operator_entities: &mut QueryState<&OperatorEntity>,
    actions: &mut QueryState<(&OperatorAction, &Bindings)>,
    binding_components: &mut QueryState<&Binding>,
) -> Vec<PaletteEntry> {
    operator_entities
        .iter(world)
        .map(|op| PaletteEntry {
            label: String::from(op.label()),
            description: String::from(op.description()),
            keybind: display_keybind(
                op.id(),
                &actions.query(world),
                &binding_components.query(world),
            ),
            footer: String::from(op.id()),
            // Creation operators take their group from the taxonomy at the
            // call site, which reads it once for both passes.
            category: Category::default(),
            action: PaletteAction::Operator(op.id()),
        })
        .collect::<Vec<_>>() // if i don't collect, it's a double borrow of `world`
        .into_iter()
        .filter(|entry| match entry.action {
            PaletteAction::Operator(id) => world.operator(id).is_available().unwrap_or(false),
            PaletteAction::Menu(_) => true,
        })
        .collect()
}

/// The categories the creation taxonomy puts its entries in, by action.
fn creation_categories(taxonomy: &CreationTaxonomy) -> std::collections::HashMap<String, Category> {
    let mut categories = std::collections::HashMap::new();
    for group in taxonomy.groups() {
        let name = taxonomy
            .qualified_label(&group.id)
            .unwrap_or_else(|| group.label.clone());
        for entry in taxonomy.entries_of(&group.id) {
            categories.insert(
                entry.action.clone(),
                Category {
                    name: Some(name.clone()),
                    order: group.order,
                },
            );
        }
    }
    categories
}

/// The creatable things that are not operators, the registered UI widgets, so
/// the palette offers the whole creation vocabulary rather than only its
/// operator-backed part.
fn creation_entries(taxonomy: &CreationTaxonomy) -> Vec<PaletteEntry> {
    let mut entries = Vec::new();
    for group in taxonomy.groups() {
        let name = taxonomy
            .qualified_label(&group.id)
            .unwrap_or_else(|| group.label.clone());
        for entry in taxonomy.entries_of(&group.id) {
            if !entry.action.starts_with(WIDGET_ACTION_PREFIX) {
                continue;
            }
            entries.push(PaletteEntry {
                label: entry.label.clone(),
                description: format!("Add a {} to the open UI scene.", entry.label),
                keybind: String::new(),
                footer: entry.action.clone(),
                category: Category {
                    name: Some(name.clone()),
                    order: group.order,
                },
                action: PaletteAction::Menu(entry.action.clone()),
            });
        }
    }
    entries
}

fn spawn_item(
    In(SpawnItemInput { matched, entities }): In<SpawnItemInput>,
    items: Query<&PickerItems<PaletteEntry>>,
    font: Res<EditorFont>,
    mut commands: Commands,
) -> Result {
    let item = items.get(entities.picker)?.at(matched.index)?;

    commands.entity(entities.list).with_child((
        picker_item(matched.index),
        children![(
            Node {
                width: percent(100),
                align_items: AlignItems::Center,
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                ..default()
            },
            children![
                match_text(matched.segments),
                (
                    Text::new(item.keybind.clone()),
                    TextFont::from(font.0.clone()).with_font_size(tokens::TEXT_SIZE_SM),
                    TextColor(tokens::TEXT_MUTED_COLOR.into())
                )
            ],
        )],
        Tooltip::title(item.label.clone())
            .with_description(item.description.clone())
            .with_keybind(item.keybind.clone())
            .with_footer(item.footer.clone()),
    ));
    Ok(())
}

fn on_select(
    input: In<SelectInput>,
    items: Query<&PickerItems<PaletteEntry>>,
    mut commands: Commands,
) -> Result {
    let item = items.get(input.entities.picker)?.at(input.index)?;

    match &item.action {
        PaletteAction::Operator(id) => {
            commands.operator(*id).call();
        }
        // Widget creation is not an operator; it dispatches through
        // `handle_menu_action`, as the Add menu's rows do.
        PaletteAction::Menu(action) => {
            commands.trigger(jackdaw_widgets::menu_bar::MenuAction {
                action: action.clone(),
            });
        }
    }

    commands.entity(input.entities.picker).try_despawn();

    Ok(())
}

/// One row of the palette: a registered operator, or a taxonomy creation entry
/// that no operator backs.
#[derive(Debug, PartialEq, Clone)]
struct PaletteEntry {
    label: String,
    description: String,
    keybind: String,
    /// What the tooltip prints under the row: an operator id, or the action a
    /// creation entry fires.
    footer: String,
    category: Category,
    action: PaletteAction,
}

#[derive(Debug, PartialEq, Clone)]
enum PaletteAction {
    Operator(&'static str),
    Menu(String),
}

impl Matchable for PaletteEntry {
    fn haystack(&self) -> String {
        self.label.clone()
    }

    fn category(&self) -> Category {
        self.category.clone()
    }
}
