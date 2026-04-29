use bevy::prelude::*;
use jackdaw_api::prelude::*;
use jackdaw_api_internal::lifecycle::{OperatorAction, OperatorEntity};
use jackdaw_feathers::icons::EditorFont;
use jackdaw_feathers::picker::{
    Matchable, PickerItems, PickerProps, SelectInput, SpawnItemInput, match_text, picker_item,
};
use jackdaw_feathers::tokens;
use jackdaw_feathers::tooltip::Tooltip;

use crate::core_extension::CoreExtensionInputContext;
use crate::operator_tooltip::display_keybind;

#[derive(Component)]
struct CommandPalette;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ToggleCommandPaletteOp>();
    ctx.register_menu_entry::<ToggleCommandPaletteOp>(TopLevelMenu::Tools);

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

    let operators = match world.run_system_cached(get_operators) {
        Ok(ops) => ops,
        Err(e) => {
            error!("Couldn't get the avilable operators: {e}");
            return OperatorResult::Finished;
        }
    };

    let props = PickerProps::new(spawn_item, on_select)
        .items(operators)
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
) -> Vec<RegisteredOperator> {
    operator_entities
        .iter(world)
        .map(|op| RegisteredOperator {
            label: op.label(),
            description: op.description(),
            keybind: display_keybind(
                op.id(),
                &actions.query(world),
                &binding_components.query(world),
            ),
            id: op.id(),
        })
        .collect::<Vec<_>>() // if i don't collect, it's a double borrow of `world`
        .into_iter()
        .filter(|op| world.operator(op.id).is_available().unwrap_or(false))
        .collect()
}

fn spawn_item(
    In(SpawnItemInput { matched, entities }): In<SpawnItemInput>,
    items: Query<&PickerItems<RegisteredOperator>>,
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
        Tooltip::title(item.label)
            .with_description(item.description)
            .with_keybind(item.keybind.clone())
            .with_footer(item.id),
    ));
    Ok(())
}

fn on_select(
    input: In<SelectInput>,
    items: Query<&PickerItems<RegisteredOperator>>,
    mut commands: Commands,
) -> Result {
    let item = items.get(input.entities.picker)?.at(input.index)?;

    commands.operator(item.id).call();

    commands.entity(input.entities.picker).try_despawn();

    Ok(())
}

#[derive(Debug, PartialEq, Clone)]
struct RegisteredOperator {
    label: &'static str,
    description: &'static str,
    id: &'static str,
    keybind: String,
}

impl Matchable for RegisteredOperator {
    fn haystack(&self) -> String {
        String::from(self.label)
    }
}
