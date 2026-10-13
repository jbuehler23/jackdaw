//! Entity-based lifecycle primitives for extensions.
//!
//! An extension is represented as an [`Entity`] carrying an [`Extension`]
//! component. Everything it registers (operators, input context entities,
//! dock windows, workspaces) is spawned as a child of that entity.
//! Unloading is `world.entity_mut(ext).despawn()`; Bevy cascades through
//! the children. A small set of observers in `ExtensionLoaderPlugin`
//! handles cleanup that can't be expressed purely as entity despawn:
//! unregistering stored `SystemId`s, removing entries from the dock
//! `WindowRegistry`, and so on.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use crate::extensions_config::init_extension;
use crate::operator::cancel_active_modal;
use crate::{TopLevelMenu, prelude::*};
use bevy::ecs::component::ComponentId;
use bevy::ecs::system::{SystemId, SystemParam};
use bevy::prelude::*;

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<ExtensionCatalog>()
        .init_resource::<ExtensionAppPlugins>()
        .init_resource::<OperatorIndex>()
        .add_observer(index_operator_on_add)
        .add_observer(deindex_and_cleanup_operator_on_remove)
        .add_observer(cleanup_window_on_remove)
        .add_observer(cleanup_workspace_on_remove)
        .add_observer(cleanup_window_extension_on_remove)
        .add_observer(cleanup_widget_on_remove)
        .add_observer(cleanup_resource_on_remove);
    app.world_mut().register_component::<ActiveModalOperator>();
}

/// Root component for an extension.
///
/// Despawning this entity tears down all of the extension's child entities:
/// operators, input context/action entities, registered windows/workspaces, and
/// observer entities. Non-ECS cleanup (unregistering `SystemId`s, removing
/// entries from `WindowRegistry`) is handled by observers reacting to the
/// child-entity despawns.
#[derive(Component, Debug)]
pub struct Extension {
    pub id: String,
}

/// [`Resource`]s attached to a specific [`Extension`].
/// When the extension is unloaded, these resources are removed via [`World::remove_resource_by_id`].
#[derive(Component, Default, Debug, PartialEq, Eq, Deref)]
#[relationship_target(relationship = ExtensionResourceOf, linked_spawn)]
pub(crate) struct ExtensionResources(Vec<Entity>);

/// Link from an entity representing a [`Resource`] to its owning [`Extension`], ensuring
/// that the resource is removed when the extension is unloaded.
#[derive(Component, Debug, PartialEq, Eq)]
#[relationship(relationship_target = ExtensionResources)]
pub(crate) struct ExtensionResourceOf {
    #[relationship]
    pub(crate) entity: Entity,
    pub(crate) resource_id: ResourceId,
}

/// The [`ComponentId`] of a [`Resource`]
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ResourceId(pub(crate) ComponentId);

impl Default for ResourceId {
    fn default() -> Self {
        Self(ComponentId::new(0))
    }
}

/// Child of an [`Extension`]; represents a single operator.
///
/// Holds the `SystemId`s that the dispatcher runs. An observer on
/// `On<Remove<OperatorEntity>>` unregisters those systems when this entity
/// despawns, and keeps the `OperatorIndex` in sync.
#[derive(Component, Debug, Clone)]
pub struct OperatorEntity {
    pub(crate) id: &'static str,
    pub(crate) label: &'static str,
    pub(crate) description: &'static str,
    pub(crate) parameters: &'static [crate::operator::ParamSpec],
    pub(crate) execute: OperatorSystemId,
    pub(crate) invoke: OperatorSystemId,
    /// Optional system that returns whether the operator can run in
    /// the current editor state.
    pub(crate) availability_check: Option<SystemId<(), bool>>,
    /// Mirrors [`crate::Operator::MODAL`]. Set at registration so the
    /// dispatcher can enter modal mode without re-resolving the generic
    /// operator type.
    pub(crate) cancel: Option<SystemId<()>>,
    pub(crate) modal: bool,
    pub(crate) allows_undo: bool,
    pub(crate) remote_hidden: Option<&'static str>,
}

impl OperatorEntity {
    /// Stable string id (e.g. `"physics.enable"`).
    pub fn id(&self) -> &'static str {
        self.id
    }

    /// User-facing label for menus, buttons, and the keybind UI.
    pub fn label(&self) -> &'static str {
        self.label
    }

    /// One-sentence description shown in tooltips and the (planned)
    /// command palette.
    pub fn description(&self) -> &'static str {
        self.description
    }

    /// Static parameter schema declared in the operator's
    /// `#[operator(params(...))]` block. Empty for ops that take no
    /// parameters.
    pub fn parameters(&self) -> &'static [crate::operator::ParamSpec] {
        self.parameters
    }

    /// True if this operator is declared `modal = true`. Modal ops
    /// can return `OperatorResult::Running` to enter a multi-frame
    /// session that the dispatcher manages via `ActiveModalOperator`.
    pub fn is_modal(&self) -> bool {
        self.modal
    }

    /// True if a successful call should push an undo entry. Mirrors
    /// the operator's `allows_undo = ...` flag. Operators with this
    /// set to `false` can still implement custom undo by pushing
    /// `EditorCommand`s directly, but the snapshot dispatcher won't
    /// auto-capture for them.
    pub fn allows_undo(&self) -> bool {
        self.allows_undo
    }

    /// Why a scripted caller is not offered this operator, or `None`
    /// when it is. Mirrors [`crate::Operator::REMOTE_HIDDEN`].
    pub fn remote_hidden(&self) -> Option<&'static str> {
        self.remote_hidden
    }
}

/// Tag on an input action entity naming the operator it dispatches when
/// it fires. [`crate::ExtensionContext::bind_operator`] and
/// [`crate::ExtensionContext::action_for`] spawn actions carrying it.
///
/// Lets systems (notably the tooltip pipeline and the keymap applier) walk
/// from operator id to the action entity's `Bindings`.
#[derive(Component, Clone, Copy, Debug)]
pub struct OperatorAction(pub &'static str);

/// The chords on this action entity belong to `operator`, for anything
/// that shows a chord next to a command's name.
///
/// For an action that is not the operator's own: a marker action whose
/// press dispatches an operator with particular parameters. The dialog
/// and the tooltips would otherwise say the operator has only the chords
/// on its own action, which is not what the user can press.
///
/// Display only. Unlike [`OperatorAction`] it is not what the keymap
/// applier resolves a preset row against, so tagging a site does not put
/// it under the keymap's control.
#[derive(Component, Clone, Copy, Debug)]
pub struct OperatorChordSite(pub &'static str);

/// Tracks the currently-active modal operator. Exactly zero or one is
/// active at any time; starting a second modal while one is running is
/// refused.
///
/// The `before_snapshot` is captured when the modal begins; on commit
/// the dispatcher diffs it against a fresh snapshot and pushes a single
/// undo entry, so the entire modal session rolls up into one Ctrl+Z.
#[derive(Component)]
pub struct ActiveModalOperator {
    pub(crate) before_snapshot: Option<Box<dyn SceneSnapshot>>,
}

/// Convenience [`SystemParam`] for querying the active modal operator.
#[derive(SystemParam)]
pub struct ActiveModalQuery<'w, 's> {
    maybe_modal: Option<Single<'w, 's, (&'static OperatorEntity, &'static ActiveModalOperator)>>,
    commands: Commands<'w, 's>,
}

impl<'w, 's> ActiveModalQuery<'w, 's> {
    pub fn is_modal_running(&self) -> bool {
        self.maybe_modal.is_some()
    }

    pub fn is_operator(&self, operator_id: impl AsRef<str>) -> bool {
        self.get_operator()
            .is_some_and(|op| op.id == operator_id.as_ref())
    }
    pub fn get_operator(&self) -> Option<&OperatorEntity> {
        self.get_operator_and_modal().map(|m| m.0)
    }
    pub fn get_modal(&self) -> Option<&ActiveModalOperator> {
        self.get_operator_and_modal().map(|m| m.1)
    }
    pub fn get_operator_and_modal(&self) -> Option<(&OperatorEntity, &ActiveModalOperator)> {
        self.maybe_modal.as_ref().map(|m| (m.0, m.1))
    }

    pub fn cancel(&mut self) {
        self.commands.queue(|world: &mut World| {
            let res: Result = world
                .run_system_cached(cancel_active_modal)
                .map_err(BevyError::from);
            if let Err(err) = res {
                error!("Failed to cancel active modal: {err}");
            }
        });
    }
}

/// Marks an entity as tracking a dock window registration.
///
/// Spawned as a child of the [`Extension`] entity when `register_window` is
/// called. An observer on `On<Remove<RegisteredWindow>>` calls
/// `WindowRegistry::unregister(id)` so the window disappears from the
/// add-window popup when the extension unloads.
#[derive(Component, Clone, Debug)]
pub struct RegisteredWindow {
    pub(crate) id: String,
}

/// Marks an entity as tracking a workspace registration.
#[derive(Component, Clone, Debug)]
pub(crate) struct RegisteredWorkspace {
    pub(crate) id: String,
}

/// Marks an entity as tracking a window-extension registration (a section
/// injected into an existing window via `ExtensionContext::extend_window`).
#[derive(Component, Clone, Debug)]
pub(crate) struct RegisteredWindowExtension {
    pub(crate) window_id: Cow<'static, str>,
    pub(crate) section_index: usize,
}

/// Tracks a widget-definition registration owned by an extension.
#[derive(Component, Clone, Debug)]
pub(crate) struct RegisteredWidgetDefinition {
    pub(crate) id: String,
    pub(crate) registration: crate::widgets::WidgetRegistrationId,
}

/// An extension-contributed entry in the editor menu bar.
///
/// Spawned as a child of the [`Extension`] entity via
/// [`crate::ExtensionContext::register_menu_entry`]. The editor's
/// `populate_menu` system queries these and inserts them into the right
/// menu. Clicking one dispatches the referenced operator.
///
/// `menu` is the top-level menu name (`"Add"`, `"Tools"`, etc.). The
/// menu system is flat today; using a path-like string here leaves room
/// for nested menus later without breaking callers.
#[derive(Component, Clone, Debug)]
pub struct RegisteredMenuEntry {
    pub menu: TopLevelMenu,
    pub label: String,
    pub operator_id: &'static str,
}

/// Reactive index from operator id -> operator entity. Maintained by the
/// `index_operator_on_add` / `deindex_operator_on_remove` observers.
/// Lets the dispatcher resolve an id to a `SystemId` in O(1).
#[derive(Resource, Default, Deref, DerefMut)]
pub(crate) struct OperatorIndex {
    pub(crate) by_id: HashMap<&'static str, Entity>,
}

/// Constructor function for an extension. Stored in [`ExtensionCatalog`].
pub(crate) type ExtensionCtor = Arc<dyn Fn() -> Box<dyn crate::JackdawExtension> + Send + Sync>;

/// Registry of all extensions compiled into this build of Jackdaw.
///
/// Populated once during startup by calling `ExtensionCatalog::register` for
/// each built-in extension. External extensions (if/when dylib loading lands)
/// would register themselves here too. Toggle UIs read the catalog to list
/// available extensions.
#[derive(Resource, Default)]
pub struct ExtensionCatalog {
    entries: HashMap<String, CatalogEntry>,
}

struct CatalogEntry {
    ctor: ExtensionCtor,
}

/// Classifies an entry in the catalog. Surfaced in toggle UIs so
/// Jackdaw-shipped feature areas and third-party extensions can be
/// presented separately. Extensions declare their own kind via
/// [`crate::JackdawExtension::kind`]; registration captures it.
///
/// Defaults to [`ExtensionKind::Regular`]. [`ExtensionKind::Builtin`] is reserved
/// for extensions shipped inside the editor binary.
// Don't change the repr(u8) value of variants; they're used in FFI
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ExtensionKind {
    /// Ships with Jackdaw as a core feature area (scene tree, inspector,
    /// Project window, etc.). Present in every build.
    Builtin = 0,
    /// Everything else: example extensions bundled for demonstration,
    /// third-party extensions loaded from disk, user-authored addons.
    Regular = 1,
}

impl ExtensionCatalog {
    /// Register a constructor with its declared kind. Most callers
    /// should use [`App::register_extension`] instead.
    fn register_extension_internal(
        &mut self,
        ctor: impl Fn() -> Box<dyn crate::JackdawExtension> + Send + Sync + 'static,
    ) {
        let id = ctor().id();
        self.entries.insert(
            id,
            CatalogEntry {
                ctor: Arc::new(ctor),
            },
        );
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// Iterate IDs with their declared [`ExtensionKind`]. Useful for
    /// grouping the Extensions dialog into Built-in and Custom sections.
    pub fn iter_with_content(
        &self,
    ) -> impl Iterator<Item = (String, String, String, ExtensionKind)> {
        self.entries.iter().map(|(id, entry)| {
            let ext = (entry.ctor)();
            (
                id.to_string(),
                ext.label().to_string(),
                ext.description().to_string(),
                ext.kind(),
            )
        })
    }

    /// Look up the declared [`ExtensionKind`] for a registered name.
    pub fn kind(&self, name: &str) -> Option<ExtensionKind> {
        self.entries.get(name).map(|e| (e.ctor)().kind())
    }

    /// Whether the named extension is a Jackdaw-shipped built-in.
    /// Returns `false` for unknown names.
    pub fn is_builtin(&self, name: &str) -> bool {
        self.kind(name) == Some(ExtensionKind::Builtin)
    }

    /// Construct a fresh instance of the named extension, if registered.
    pub fn construct(&self, name: &str) -> Option<Box<dyn crate::JackdawExtension>> {
        self.entries.get(name).map(|e| (e.ctor)())
    }

    pub fn unregister(&mut self, name: &str) -> bool {
        self.entries.remove(name).is_some()
    }
}

pub trait ExtensionAppExt {
    /// Register a built-in extension constructor in the catalog.
    ///
    /// Call this once per built-in extension during app setup. Registering the
    /// constructor lets the Extensions dialog list it. Runtime-safe input
    /// bindings belong in the host-owned context through
    /// [`crate::ExtensionContext::bind_operator_host`].
    ///
    /// See also [`Self::register_extension_with`].
    fn register_extension<T: crate::JackdawExtension + Default>(&mut self) -> &mut Self {
        self.register_extension_with(|| Box::new(T::default()))
    }

    /// Like [`Self::register_extension`], but with a custom constructor.
    fn register_extension_with(
        &mut self,
        ctor: impl Fn() -> Box<dyn crate::JackdawExtension> + Send + Sync + 'static,
    ) -> &mut Self;
}

impl ExtensionAppExt for App {
    fn register_extension_with(
        &mut self,
        ctor: impl Fn() -> Box<dyn crate::JackdawExtension> + Send + Sync + 'static,
    ) -> &mut Self {
        let extension = ctor();
        let id = extension.id();
        add_app_plugin(self, extension.as_ref());
        self.world_mut()
            .resource_mut::<ExtensionCatalog>()
            .register_extension_internal(ctor);

        init_extension(id);
        self
    }
}

/// The App plugins extensions added while the editor `App` was built, and
/// the extensions waiting on a restart because of them.
///
/// See [`crate::JackdawExtension::app_plugin`].
#[derive(Resource, Default)]
pub struct ExtensionAppPlugins {
    /// Plugin name by extension id, for each plugin added at launch.
    added: HashMap<String, String>,
    /// Extensions whose requested state takes effect after a restart, with
    /// whether that state is enabled.
    pub(crate) pending: HashMap<String, bool>,
}

impl ExtensionAppPlugins {
    /// The name of the plugin `id` added at launch, if it added one.
    pub fn plugin_of(&self, id: &str) -> Option<&str> {
        self.added.get(id).map(String::as_str)
    }

    /// Whether `extension` can activate in this session: it declares the
    /// same App plugin it added at launch, or none and added none.
    pub fn can_activate(&self, extension: &dyn crate::JackdawExtension) -> bool {
        let declared = extension
            .app_plugin()
            .map(|plugin| plugin.name().to_string());
        declared.as_deref() == self.plugin_of(&extension.id())
    }

    /// Whether `id` waits on a restart.
    pub fn restart_required(&self, id: &str) -> bool {
        self.pending.contains_key(id)
    }

    /// The state `id` takes after a restart, if it waits on one.
    pub fn pending_state(&self, id: &str) -> Option<bool> {
        self.pending.get(id).copied()
    }

    /// Record that `id` takes the given enabled state after a restart.
    pub fn require_restart(&mut self, id: impl Into<String>, enabled: bool) {
        self.pending.insert(id.into(), enabled);
    }

    /// Forget a pending restart for `id`, once the session already matches it.
    pub fn clear_restart(&mut self, id: &str) {
        self.pending.remove(id);
    }
}

/// Wraps an extension's boxed plugin so `App::add_plugins` accepts it.
struct ExtensionAppPlugin(Box<dyn Plugin>);

impl Plugin for ExtensionAppPlugin {
    fn build(&self, app: &mut App) {
        self.0.build(app);
    }

    fn ready(&self, app: &App) -> bool {
        self.0.ready(app)
    }

    fn finish(&self, app: &mut App) {
        self.0.finish(app);
    }

    fn cleanup(&self, app: &mut App) {
        self.0.cleanup(app);
    }

    fn name(&self) -> &str {
        self.0.name()
    }

    fn is_unique(&self) -> bool {
        self.0.is_unique()
    }
}

/// Add `extension`'s App plugin to `app` when the user has not turned the
/// extension off. Call while the `App` is being built; built-in extensions
/// get this through [`ExtensionAppExt::register_extension`] and installed
/// ones through the dylib loader.
pub fn add_app_plugin(app: &mut App, extension: &dyn crate::JackdawExtension) {
    let Some(plugin) = extension.app_plugin() else {
        return;
    };
    let id = extension.id();
    if !crate::extensions_config::is_extension_enabled(&id) {
        return;
    }
    let name = plugin.name().to_string();
    info!("Adding the App plugin `{name}` of extension `{id}`");
    app.add_plugins(ExtensionAppPlugin(plugin));
    app.world_mut()
        .get_resource_or_init::<ExtensionAppPlugins>()
        .added
        .insert(id, name);
}

/// Unload an extension. Despawns the root entity; the cascade and
/// cleanup observers handle the rest.
pub fn unload_extension(world: &mut World, ext_entity: Entity) {
    let ext_name = world
        .get::<Extension>(ext_entity)
        .map(|e| e.id.clone())
        .unwrap_or_default();
    info!("Unloading extension: {}", ext_name);

    if let Some(stored) = world.entity_mut(ext_entity).take::<StoredExtension>() {
        stored.0.unregister(world, ext_entity);
    }
    if let Ok(ec) = world.get_entity_mut(ext_entity) {
        ec.despawn();
    }
}

/// Enable a named extension via the catalog. Returns the new extension
/// entity, or `None` if the name is unknown or already loaded.
pub fn enable_extension(world: &mut World, id: &str) -> Option<Entity> {
    {
        let mut query = world.query::<&Extension>();
        if query.iter(world).any(|e| e.id == id) {
            return None;
        }
    }

    let extension = world.resource::<ExtensionCatalog>().construct(id)?;
    let mut app_plugins = world.get_resource_or_init::<ExtensionAppPlugins>();
    if !app_plugins.can_activate(extension.as_ref()) {
        info!("Extension `{id}` adds an App plugin; it is enabled after a restart");
        app_plugins.require_restart(id, true);
        return None;
    }
    app_plugins.clear_restart(id);
    Some(load_static_extension(world, extension))
}

/// Load an extension statically. Spawns an `Extension` entity, runs
/// `extension.register()` against it, returns the entity.
///
/// Takes `&mut World` (not `&mut App`) so this can be called from
/// world-scoped contexts like observer callbacks.
pub fn load_static_extension(
    world: &mut World,
    extension: Box<dyn crate::JackdawExtension>,
) -> Entity {
    let id = extension.id();
    info!("Loading extension: {id}");

    let extension_entity = world
        .spawn((Extension { id }, crate::ExtensionInputContext))
        .id();

    let mut ctx = crate::ExtensionContext::new(world, extension_entity);
    extension.register(&mut ctx);

    // Store the extension trait object on the entity so `unload_extension`
    // can call `unregister` before despawn.
    world
        .entity_mut(extension_entity)
        .insert(StoredExtension(extension));

    extension_entity
}

/// Disable a named extension by despawning its root entity.
pub fn disable_extension(world: &mut World, id: &str) -> bool {
    let mut query = world.query::<(Entity, &Extension)>();
    let Some(ext_entity) = query.iter(world).find(|(_, e)| e.id == id).map(|(e, _)| e) else {
        return false;
    };
    unload_extension(world, ext_entity);
    true
}

/// The dock layout before an extension is reloaded, so the reload can put
/// back the tabs its unload closed.
pub struct DockLayoutSnapshot {
    live: Option<jackdaw_panels::tree::DockTree>,
    workspaces: Vec<(String, jackdaw_panels::tree::DockTree)>,
}

impl DockLayoutSnapshot {
    /// Record the live dock tree and every workspace's tree.
    pub fn capture(world: &World) -> Self {
        Self {
            live: world
                .get_resource::<jackdaw_panels::tree::DockTree>()
                .cloned(),
            workspaces: world
                .get_resource::<jackdaw_panels::WorkspaceRegistry>()
                .map(|registry| {
                    registry
                        .workspaces
                        .iter()
                        .map(|workspace| (workspace.id.clone(), workspace.tree.clone()))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }

    /// Put back each recorded tree that has lost tabs since the capture,
    /// leaving out the tabs of windows that are no longer registered.
    pub fn restore(self, world: &mut World) {
        let Some(registry) = world.get_resource::<jackdaw_panels::WindowRegistry>() else {
            return;
        };
        let live = self.live.and_then(|snapshot| {
            let current = world.get_resource::<jackdaw_panels::tree::DockTree>()?;
            restored_tree(snapshot, current, registry)
        });
        let workspaces: Vec<(String, jackdaw_panels::tree::DockTree)> = world
            .get_resource::<jackdaw_panels::WorkspaceRegistry>()
            .map(|workspaces| {
                self.workspaces
                    .into_iter()
                    .filter_map(|(id, snapshot)| {
                        let current = &workspaces.workspaces.iter().find(|w| w.id == id)?.tree;
                        Some((id, restored_tree(snapshot, current, registry)?))
                    })
                    .collect()
            })
            .unwrap_or_default();
        if let Some(tree) = live {
            *world.resource_mut::<jackdaw_panels::tree::DockTree>() = tree;
        }
        if let Some(mut registry) = world.get_resource_mut::<jackdaw_panels::WorkspaceRegistry>() {
            for (id, tree) in workspaces {
                if let Some(workspace) = registry.workspaces.iter_mut().find(|w| w.id == id) {
                    workspace.tree = tree;
                }
            }
        }
    }
}

/// `snapshot` without the tabs of unregistered windows, when `current` has
/// lost a tab `snapshot` holds. `None` when nothing was lost.
fn restored_tree(
    mut snapshot: jackdaw_panels::tree::DockTree,
    current: &jackdaw_panels::tree::DockTree,
    registry: &jackdaw_panels::WindowRegistry,
) -> Option<jackdaw_panels::tree::DockTree> {
    let lost = snapshot
        .tabs()
        .any(|(_, tab)| !current.has_window_kind(&tab.window_id));
    if !lost {
        return None;
    }
    let unregistered: Vec<String> = snapshot
        .tabs()
        .filter(|(_, tab)| registry.get(&tab.window_id).is_none())
        .map(|(_, tab)| tab.window_id.clone())
        .collect();
    for window in unregistered {
        snapshot.remove_window_kind(&window);
    }
    Some(snapshot)
}

/// Internal component holding the extension trait object for the duration
/// of its lifetime. Used by [`unload_extension`] to invoke the optional
/// `unregister` hook before despawning. Not part of the public API.
#[derive(Component)]
pub(crate) struct StoredExtension(pub(crate) Box<dyn crate::JackdawExtension>);

/// Register a dylib-loaded extension into a running editor's catalog.
/// The dylib loader uses this after reading the dylib's entry metadata.
/// Operates on `&mut World` (not `&mut App`) because installs happen post-startup.
pub fn register_dylib_extension<F>(world: &mut World, ctor: F)
where
    F: Fn() -> Box<dyn crate::JackdawExtension> + Send + Sync + 'static,
{
    world
        .resource_mut::<ExtensionCatalog>()
        .register_extension_internal(ctor);
}

/// Remove a runtime extension constructor after its live registrations have
/// been disabled. The native library mapping remains owned by `LoadedDylibs`.
pub fn unregister_dylib_extension(world: &mut World, id: &str) -> bool {
    world.resource_mut::<ExtensionCatalog>().unregister(id)
}

/// Keep `OperatorIndex` in sync when an operator entity is spawned.
pub(crate) fn index_operator_on_add(
    trigger: On<Add<OperatorEntity>>,
    operators: Query<&OperatorEntity>,
    mut index: ResMut<OperatorIndex>,
) {
    if let Ok(op) = operators.get(trigger.event_target()) {
        index.insert(op.id, trigger.event_target());
    }
}

/// Keep `OperatorIndex` in sync and free the operator's `SystemId`s
/// when its entity is removed, so they don't leak across enable /
/// disable cycles.
pub(crate) fn deindex_and_cleanup_operator_on_remove(
    trigger: On<Remove<OperatorEntity>>,
    operators: Query<&OperatorEntity>,
    mut index: ResMut<OperatorIndex>,
    mut commands: Commands,
) {
    let Ok(op) = operators.get(trigger.event_target()) else {
        return;
    };
    info!("Unregistering operator: {}", op.id);
    index.remove(op.id);
    let (exec, inv, check) = (op.execute, op.invoke, op.availability_check);
    commands.queue(move |world: &mut World| {
        let _ = world.unregister_system(exec);
        if exec != inv {
            let _ = world.unregister_system(inv);
        }
        if let Some(c) = check {
            let _ = world.unregister_system(c);
        }
    });
}

/// Unregister a dock window from [`jackdaw_panels::WindowRegistry`] and
/// purge it from the live dock tree and every stored workspace tree when
/// its marker entity despawns, so disabling an extension visibly removes
/// its windows.
pub(crate) fn cleanup_window_on_remove(
    trigger: On<Remove<RegisteredWindow>>,
    windows: Query<&RegisteredWindow>,
    mut registry: ResMut<jackdaw_panels::WindowRegistry>,
    mut dock_tree: ResMut<jackdaw_panels::tree::DockTree>,
    mut workspaces: ResMut<jackdaw_panels::WorkspaceRegistry>,
) {
    let Ok(w) = windows.get(trigger.event_target()) else {
        return;
    };
    info!("Unregistering window: {}", w.id);
    registry.unregister(&w.id);
    dock_tree.remove_window_kind(&w.id);
    for workspace in workspaces.workspaces.iter_mut() {
        workspace.tree.remove_window_kind(&w.id);
    }
}

/// Unregister a workspace when its marker entity despawns.
pub(crate) fn cleanup_workspace_on_remove(
    trigger: On<Remove<RegisteredWorkspace>>,
    workspaces: Query<&RegisteredWorkspace>,
    mut registry: ResMut<jackdaw_panels::WorkspaceRegistry>,
) {
    if let Ok(w) = workspaces.get(trigger.event_target()) {
        registry.unregister(&w.id);
    }
}

/// Remove a panel extension section from the registry when its marker
/// entity despawns.
pub(crate) fn cleanup_window_extension_on_remove(
    trigger: On<Remove<RegisteredWindowExtension>>,
    registrations: Query<&RegisteredWindowExtension>,
    mut registry: ResMut<crate::WindowExtensionRegistry>,
) {
    if let Ok(r) = registrations.get(trigger.event_target()) {
        registry.remove(&r.window_id, r.section_index);
    }
}

pub(crate) fn cleanup_widget_on_remove(
    trigger: On<Remove<RegisteredWidgetDefinition>>,
    registrations: Query<&RegisteredWidgetDefinition>,
    mut registry: ResMut<crate::widgets::WidgetRegistry>,
) {
    if let Ok(registration) = registrations.get(trigger.event_target()) {
        registry.unregister_scoped(&registration.id, registration.registration);
    }
}

fn cleanup_resource_on_remove(
    trigger: On<Remove<ExtensionResourceOf>>,
    resource_id: Query<&ExtensionResourceOf>,
    mut commands: Commands,
) {
    let Ok(relation) = resource_id.get(trigger.entity) else {
        return;
    };
    let component_id = relation.resource_id.0;
    commands.queue(move |world: &mut World| {
        world.remove_resource_by_id(component_id);
    });
}
