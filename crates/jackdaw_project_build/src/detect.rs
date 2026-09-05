//! Source detection: find a project's `JackdawExtension` impl and its
//! game `Plugin` type.
//!
//! Extension builds need a path the generated shim can name.
//! Game plugin detection feeds import/setup notes and `jd doctor`;
//! Play launches the project's cargo binary, which adds the plugin
//! itself.
//!
//! The sources are parsed, not scanned. A text scan cannot be made
//! correct here: doc comments contain `/*` (the game template's own
//! mentions `assets/*.bsn`), format strings contain braces, modules
//! nest inline, and `pub use` groups its names. Each of those defeats a
//! different heuristic, and the cost of a wrong answer is a path pasted
//! into a generated crate the user is told not to edit.
//!
//! No editor or bevy dependency, so the build pipeline can resolve a
//! shim spec from the filesystem alone.

use std::path::{Path, PathBuf};

use syn::{Item, UseTree, Visibility};

/// Detect a `JackdawExtension` impl in the project's sources, returning
/// `(lib_name, extension_type_name)`.
pub fn detect_extension(root: &Path) -> Option<(String, String)> {
    let name = scan(root, "JackdawExtension")
        .into_iter()
        .next()
        .map(|found| found.type_name)?;
    let lib_name = root
        .file_name()
        .map(|s| s.to_string_lossy().replace('-', "_"))
        .unwrap_or_default();
    Some((lib_name, name))
}

/// Find the game plugin type in the project's sources, as
/// `lib_name::path::TypeName`.
///
/// The signal is `impl Plugin for X`, which is what actually makes a
/// type a Bevy plugin. Naming alone is not enough: a crate commonly
/// declares several `*Plugin` types (`AudioPlugin`, `UiPlugin`) and the
/// first one is rarely the game's root plugin.
///
/// Returns `None` rather than guessing when the crate has no plugin,
/// when it has more than one and none is the conventional `GamePlugin`,
/// or when the one it has cannot be named from outside the crate. Use
/// [`plugin_paths`] to tell those apart.
pub fn detect_plugin(root: &Path, lib_name: &str) -> Option<String> {
    let found = plugin_paths(root);
    let chosen = match found.as_slice() {
        [only] => only,
        [] => return None,
        several => several
            .iter()
            .find(|found| found.type_name == "GamePlugin")?,
    };
    Some(format!("{lib_name}::{}", chosen.crate_path.as_ref()?))
}

/// Every type in the crate with an `impl Plugin for` block, in scan
/// order. The import flow shows these when the choice is ambiguous.
pub fn plugin_candidates(root: &Path) -> Vec<String> {
    plugin_paths(root)
        .into_iter()
        .map(|found| found.type_name)
        .collect()
}

/// A plugin impl found in a crate's sources.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundPlugin {
    /// The bare type name, for display and for matching conventions.
    pub type_name: String,
    /// How to name it from the crate root, e.g. `GamePlugin` or
    /// `game::WorldPlugin`. `None` when it cannot be named from outside
    /// the crate (a private module with no re-export), which the caller
    /// must not paste into generated code.
    pub crate_path: Option<String>,
}

/// Every plugin impl in the crate, in module-tree order from `lib.rs`.
pub fn plugin_paths(root: &Path) -> Vec<FoundPlugin> {
    scan(root, "Plugin")
}

/// Why a crate yielded no plugins, when it yielded none. Callers report
/// "your lib.rs does not parse" differently from "you have no plugin":
/// they are different problems with different next actions.
pub fn lib_parse_error(root: &Path) -> Option<String> {
    let text = std::fs::read_to_string(root.join("src/lib.rs")).ok()?;
    syn::parse_file(&text).err().map(|error| error.to_string())
}

/// One `impl <trait> for <type>` and where it lives.
struct Found {
    type_name: String,
    /// Module path from the crate root; empty for `lib.rs`.
    module: Vec<String>,
    /// Whether every module between the crate root and this item is
    /// `pub`, i.e. whether the qualified path is nameable outside.
    module_is_public: bool,
}

/// One `pub use` target: where it appears and what module it points at.
struct Reexport {
    from: Vec<String>,
    target: Vec<String>,
    /// What the `pub use` exports: `None` for a glob (everything in the
    /// target module), else `(original, exported)`. Those differ only
    /// for `pub use path::Original as Exported`, which is the one case
    /// where the name callers must write is not the type's own.
    exported: Option<(String, String)>,
}

/// Accumulated state for the module walk.
#[derive(Default)]
struct Scan {
    found: Vec<Found>,
    reexports: Vec<Reexport>,
}

/// Parse the crate rooted at `<root>/src/lib.rs` and resolve every
/// `impl <trait_name> for X`.
fn scan(root: &Path, trait_name: &str) -> Vec<FoundPlugin> {
    let src = root.join("src");
    let Ok(text) = std::fs::read_to_string(src.join("lib.rs")) else {
        return Vec::new();
    };
    let Ok(file) = syn::parse_file(&text) else {
        return Vec::new();
    };

    let mut state = Scan::default();
    walk(&src, &file.items, &[], true, trait_name, &mut state, 0);

    // A `pub use` can name a type at a shorter path, and can rescue one
    // whose module is private: `mod internal; pub use internal::Thing;`
    // is the standard facade.
    let visible = crate_root_visible(&state.reexports);

    state
        .found
        .into_iter()
        .map(|item| {
            let qualified = if item.module.is_empty() {
                Some(item.type_name.clone())
            } else if item.module_is_public {
                Some(format!("{}::{}", item.module.join("::"), item.type_name))
            } else {
                None
            };
            // A named `pub use` exports exactly one type, under
            // whatever name it gives it; a glob exports the module's
            // contents under their own names. Treating a named export
            // of some other type as making the whole module reachable
            // would claim a path that does not resolve.
            let named = state.reexports.iter().find_map(|reexport| {
                let (original, exported) = reexport.exported.as_ref()?;
                let target = resolve_target(&reexport.from, &reexport.target)?;
                (*original == item.type_name
                    && target == item.module
                    && visible.contains(&reexport.from))
                .then(|| exported.clone())
            });
            let reexported = named.or_else(|| {
                visible
                    .contains(&item.module)
                    .then(|| item.type_name.clone())
            });
            FoundPlugin {
                // Prefer the re-exported short name: it is what the
                // crate's author intended callers to use.
                crate_path: reexported.or(qualified),
                type_name: item.type_name,
            }
        })
        .collect()
}

/// Walk a module's items, recording impls and `pub use` re-exports, and
/// recursing into both inline and file submodules.
fn walk(
    src: &Path,
    items: &[Item],
    module: &[String],
    public_so_far: bool,
    trait_name: &str,
    state: &mut Scan,
    depth: usize,
) {
    // Cycles are impossible in a module tree, but a `#[path]` attribute
    // could aim a module at an ancestor; bound the walk regardless.
    const MAX_DEPTH: usize = 16;
    if depth > MAX_DEPTH {
        return;
    }
    for item in items {
        match item {
            Item::Impl(imp) => {
                let Some((_, path, _)) = &imp.trait_ else {
                    continue;
                };
                if path.segments.last().is_none_or(|s| s.ident != trait_name) {
                    continue;
                }
                // A conditionally-compiled impl may not exist in the
                // build the shim links. Recording it anyway produces a
                // reference to a plugin that is not there; leaving it
                // out only costs the user a prompt to set `plugin`.
                if is_conditional(&imp.attrs) {
                    continue;
                }
                if let Some(type_name) = type_name_of(&imp.self_ty)
                    && !state.found.iter().any(|f| f.type_name == type_name)
                {
                    state.found.push(Found {
                        type_name,
                        module: module.to_vec(),
                        module_is_public: public_so_far,
                    });
                }
            }
            Item::Use(use_item) if matches!(use_item.vis, Visibility::Public(_)) => {
                collect_reexports(
                    &use_item.tree,
                    module,
                    &mut Vec::new(),
                    &mut state.reexports,
                );
            }
            // A module that may not be compiled cannot be relied on to
            // hold a plugin the crate can export.
            Item::Mod(module_item) if is_conditional(&module_item.attrs) => {}
            Item::Mod(module_item) => {
                let mut child = module.to_vec();
                child.push(module_item.ident.to_string());
                let child_public =
                    public_so_far && matches!(module_item.vis, Visibility::Public(_));
                match &module_item.content {
                    // Inline `mod x { ... }`: the items are right here.
                    Some((_, items)) => {
                        walk(
                            src,
                            items,
                            &child,
                            child_public,
                            trait_name,
                            state,
                            depth + 1,
                        );
                    }
                    // `mod x;`: `x.rs`, `x/mod.rs`, or wherever a
                    // `#[path]` attribute points.
                    None => {
                        let relocated = path_attribute(&module_item.attrs);
                        if let Some(items) = module_file(src, &child, relocated.as_deref()) {
                            walk(
                                src,
                                &items,
                                &child,
                                child_public,
                                trait_name,
                                state,
                                depth + 1,
                            );
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Parse the file backing `mod <path>`: whatever `#[path]` names,
/// else `a/b.rs`, else `a/b/mod.rs`.
fn module_file(src: &Path, path: &[String], relocated: Option<&str>) -> Option<Vec<Item>> {
    let dir: PathBuf = path.iter().collect();
    let text = match relocated {
        // `#[path]` is relative to the directory of the file that
        // declares the module, which for a top-level `mod` is `src/`.
        Some(rel) => {
            let parent = dir.parent().map(Path::to_path_buf).unwrap_or_default();
            std::fs::read_to_string(src.join(parent).join(rel)).ok()?
        }
        None => {
            let flat = src.join(&dir).with_extension("rs");
            let nested = src.join(&dir).join("mod.rs");
            std::fs::read_to_string(&flat)
                .or_else(|_| std::fs::read_to_string(&nested))
                .ok()?
        }
    };
    syn::parse_file(&text).ok().map(|file| file.items)
}

/// The value of a `#[path = "..."]` attribute, if present.
fn path_attribute(attrs: &[syn::Attribute]) -> Option<String> {
    attrs.iter().find_map(|attr| {
        if !attr.path().is_ident("path") {
            return None;
        }
        let syn::Meta::NameValue(name_value) = &attr.meta else {
            return None;
        };
        let syn::Expr::Lit(expr) = &name_value.value else {
            return None;
        };
        let syn::Lit::Str(literal) = &expr.lit else {
            return None;
        };
        Some(literal.value())
    })
}

/// Whether an item is behind `#[cfg(...)]`, and so may not exist in the
/// build the shim links.
fn is_conditional(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|attr| attr.path().is_ident("cfg"))
}

/// The bare type name an `impl ... for <ty>` targets, dropping generics
/// and qualification: the shim names the type through the crate root,
/// not through however the impl spelled it.
fn type_name_of(ty: &syn::Type) -> Option<String> {
    let syn::Type::Path(path) = ty else {
        return None;
    };
    path.path
        .segments
        .last()
        .map(|segment| segment.ident.to_string())
}

/// Flatten a `pub use` tree into its leaves, handling nesting, groups
/// (`pub use game::{A, B}`), renames, and globs.
fn collect_reexports(
    tree: &UseTree,
    module: &[String],
    prefix: &mut Vec<String>,
    out: &mut Vec<Reexport>,
) {
    match tree {
        UseTree::Path(path) => {
            prefix.push(path.ident.to_string());
            collect_reexports(&path.tree, module, prefix, out);
            prefix.pop();
        }
        UseTree::Group(group) => {
            for tree in &group.items {
                collect_reexports(tree, module, prefix, out);
            }
        }
        // For a plain name or a glob, what matters is only that the
        // module is nameable from here; a found impl is matched by its
        // module path. A rename additionally changes what callers must
        // write, so it carries both idents.
        UseTree::Name(name) => out.push(Reexport {
            from: module.to_vec(),
            target: prefix.clone(),
            exported: Some((name.ident.to_string(), name.ident.to_string())),
        }),
        UseTree::Rename(rename) => out.push(Reexport {
            from: module.to_vec(),
            target: prefix.clone(),
            exported: Some((rename.ident.to_string(), rename.rename.to_string())),
        }),
        UseTree::Glob(_) => out.push(Reexport {
            from: module.to_vec(),
            target: prefix.clone(),
            exported: None,
        }),
    }
}

/// Module paths whose contents are named at the crate root, following
/// chains: `lib.rs: pub use a::*` plus `a/mod.rs: pub use b::*` makes
/// `a::b`'s items reachable as crate-root names.
///
/// Only crate-local targets count: `pub use bevy::prelude::*` names
/// somebody else's module however its segments are spelled, which a
/// segment-name comparison would happily match against a local
/// `mod prelude`.
fn crate_root_visible(reexports: &[Reexport]) -> Vec<Vec<String>> {
    let mut visible: Vec<Vec<String>> = vec![Vec::new()];
    // A fixpoint over re-export chains; module trees are shallow, so a
    // handful of rounds is ample.
    for _ in 0..8 {
        let before = visible.len();
        for reexport in reexports {
            // Only a glob makes a module's contents reachable wholesale.
            // A named export is handled per-type by the caller.
            if reexport.exported.is_some() || !visible.contains(&reexport.from) {
                continue;
            }
            let Some(resolved) = resolve_target(&reexport.from, &reexport.target) else {
                continue;
            };
            if !visible.contains(&resolved) {
                visible.push(resolved);
            }
        }
        if visible.len() == before {
            break;
        }
    }
    visible
}

/// Resolve a `use` path relative to the module it appears in. `None`
/// when it names nothing local.
fn resolve_target(from: &[String], target: &[String]) -> Option<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    let mut segments = target.iter();
    match segments.next().map(String::as_str) {
        Some("crate") => {}
        Some("self") => out.extend(from.iter().cloned()),
        Some("super") => {
            let mut parent = from.to_vec();
            parent.pop()?;
            out = parent;
        }
        // A bare first segment is a sibling module. It could also be an
        // external crate; those resolve to a path no module of ours
        // occupies, so they simply never match a found impl.
        Some(first) => {
            out.extend(from.iter().cloned());
            out.push(first.to_string());
        }
        None => return None,
    }
    out.extend(segments.cloned());
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(files: &[(&str, &str)]) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "jackdaw_detect_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        for (rel, text) in files {
            let path = dir.join("src").join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        dir
    }

    /// The regression that made this a parser. The game template's own
    /// doc comment says ``assets/*.bsn``, whose `/*` made a text
    /// scanner discard the rest of the file, so a freshly scaffolded
    /// project had no findable plugin and its Play button did nothing.
    #[test]
    fn a_doc_comment_containing_a_glob_does_not_hide_the_plugin() {
        let dir = project(&[(
            "lib.rs",
            "//! Scene content lives in `assets/*.bsn` files.\n\
             pub struct GamePlugin;\n\
             impl Plugin for GamePlugin {\n    fn build(&self, _: &mut App) {}\n}\n",
        )]);
        assert_eq!(
            detect_plugin(&dir, "my_game").as_deref(),
            Some("my_game::GamePlugin")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_plugin_impl_beats_a_type_that_is_merely_named_plugin() {
        let dir = project(&[(
            "lib.rs",
            "pub struct AudioPlugin;\n\
             pub struct MyGamePlugin;\n\
             impl Plugin for MyGamePlugin {\n    fn build(&self, _: &mut App) {}\n}\n",
        )]);
        assert_eq!(
            detect_plugin(&dir, "my_game").as_deref(),
            Some("my_game::MyGamePlugin")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_reexported_submodule_plugin_uses_the_short_path() {
        let dir = project(&[
            ("lib.rs", "pub mod game;\npub use game::*;\n"),
            ("game.rs", "impl Plugin for WorldPlugin {}\n"),
        ]);
        assert_eq!(
            detect_plugin(&dir, "my_game").as_deref(),
            Some("my_game::WorldPlugin")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_submodule_plugin_keeps_its_module_path() {
        let dir = project(&[
            ("lib.rs", "pub mod game;\n"),
            ("game.rs", "impl Plugin for WorldPlugin {}\n"),
        ]);
        assert_eq!(
            detect_plugin(&dir, "my_game").as_deref(),
            Some("my_game::game::WorldPlugin")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_nested_module_path_is_fully_qualified() {
        let dir = project(&[
            ("lib.rs", "pub mod a;\n"),
            ("a/mod.rs", "pub mod b;\n"),
            ("a/b.rs", "impl Plugin for DeepPlugin {}\n"),
        ]);
        assert_eq!(
            detect_plugin(&dir, "my_game").as_deref(),
            Some("my_game::a::b::DeepPlugin")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_plugin_in_a_private_module_is_not_recorded() {
        let dir = project(&[
            ("lib.rs", "mod game;\n"),
            ("game.rs", "impl Plugin for WorldPlugin {}\n"),
        ]);
        assert_eq!(detect_plugin(&dir, "my_game"), None);
        assert_eq!(plugin_candidates(&dir), ["WorldPlugin"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The shim is a separate crate, so `pub(crate)` is as unreachable
    /// to it as private.
    #[test]
    fn a_pub_crate_module_is_not_reachable_from_the_shim() {
        let dir = project(&[
            ("lib.rs", "pub(crate) mod game;\n"),
            ("game.rs", "impl Plugin for WorldPlugin {}\n"),
        ]);
        assert_eq!(detect_plugin(&dir, "my_game"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The facade: a private module re-exported at the root really is
    /// reachable, by the short name.
    #[test]
    fn a_reexport_rescues_a_private_module() {
        let dir = project(&[
            ("lib.rs", "mod internal;\npub use internal::WorldPlugin;\n"),
            ("internal.rs", "impl Plugin for WorldPlugin {}\n"),
        ]);
        assert_eq!(
            detect_plugin(&dir, "my_game").as_deref(),
            Some("my_game::WorldPlugin")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_grouped_reexport_is_understood() {
        let dir = project(&[
            ("lib.rs", "mod game;\npub use game::{WorldPlugin, Other};\n"),
            (
                "game.rs",
                "impl Plugin for WorldPlugin {}\npub struct Other;\n",
            ),
        ]);
        assert_eq!(
            detect_plugin(&dir, "my_game").as_deref(),
            Some("my_game::WorldPlugin")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A facade re-exporting through two levels.
    #[test]
    fn a_chained_reexport_is_followed() {
        let dir = project(&[
            ("lib.rs", "mod a;\npub use a::*;\n"),
            ("a/mod.rs", "mod b;\npub use b::*;\n"),
            ("a/b.rs", "impl Plugin for WorldPlugin {}\n"),
        ]);
        assert_eq!(
            detect_plugin(&dir, "my_game").as_deref(),
            Some("my_game::WorldPlugin")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `pub use bevy::prelude::*` names somebody else's module; a local
    /// `mod prelude` of the same name is not thereby re-exported.
    #[test]
    fn a_foreign_glob_import_is_not_a_reexport() {
        let dir = project(&[
            ("lib.rs", "pub use bevy::prelude::*;\nmod prelude;\n"),
            ("prelude.rs", "impl Plugin for WorldPlugin {}\n"),
        ]);
        assert_eq!(detect_plugin(&dir, "my_game"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An inline `pub mod` is a real, reachable module: the old walk
    /// declared it unreachable and told the user to make it `pub mod`,
    /// which it already was.
    #[test]
    fn an_inline_public_module_is_reachable() {
        let dir = project(&[(
            "lib.rs",
            "pub mod game {\n    impl Plugin for WorldPlugin {}\n}\n",
        )]);
        assert_eq!(
            detect_plugin(&dir, "my_game").as_deref(),
            Some("my_game::game::WorldPlugin")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_inline_private_module_is_not_reachable() {
        let dir = project(&[(
            "lib.rs",
            "mod hidden {\n    impl Plugin for WorldPlugin {}\n}\n",
        )]);
        assert_eq!(detect_plugin(&dir, "my_game"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Braces inside string literals used to break brace counting and
    /// make every later plugin look unreachable.
    #[test]
    fn braces_in_literals_do_not_confuse_the_walk() {
        let dir = project(&[(
            "lib.rs",
            "mod dbg {\n    pub fn p() { println!(\"{{\"); }\n}\n\
             impl Plugin for RealPlugin {}\n",
        )]);
        assert_eq!(
            detect_plugin(&dir, "my_game").as_deref(),
            Some("my_game::RealPlugin")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn commented_out_impls_are_not_candidates() {
        let dir = project(&[(
            "lib.rs",
            "// impl Plugin for LinePlugin {}\n\
             /*\nimpl Plugin for BlockPlugin {}\n*/\n\
             /* /* nested */ impl Plugin for NestedPlugin {} */\n\
             impl Plugin for RealPlugin {}\n",
        )]);
        assert_eq!(plugin_candidates(&dir), ["RealPlugin"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn several_plugins_resolve_to_the_conventional_one() {
        let dir = project(&[(
            "lib.rs",
            "impl Plugin for AudioPlugin {}\nimpl Plugin for GamePlugin {}\n",
        )]);
        assert_eq!(
            detect_plugin(&dir, "my_game").as_deref(),
            Some("my_game::GamePlugin")
        );
        assert_eq!(plugin_candidates(&dir), ["AudioPlugin", "GamePlugin"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn several_plugins_with_no_convention_is_not_guessed() {
        let dir = project(&[(
            "lib.rs",
            "impl Plugin for AudioPlugin {}\nimpl Plugin for UiPlugin {}\n",
        )]);
        assert_eq!(detect_plugin(&dir, "my_game"), None);
        assert_eq!(plugin_candidates(&dir), ["AudioPlugin", "UiPlugin"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_crate_with_no_plugin_reports_none() {
        let dir = project(&[("lib.rs", "pub fn helper() {}\n")]);
        assert_eq!(detect_plugin(&dir, "my_game"), None);
        assert!(plugin_candidates(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn plugin_groups_are_not_plugins() {
        let dir = project(&[(
            "lib.rs",
            "impl PluginGroup for MyPlugins {}\nimpl Plugin for RealPlugin {}\n",
        )]);
        assert_eq!(plugin_candidates(&dir), ["RealPlugin"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `#[path]` relocates a module's file; ignoring it means the
    /// module's contents are never read.
    /// `pub use X as Y` means callers write `Y`. Recording `X` hands
    /// the shim a name that does not resolve.
    #[test]
    fn a_renamed_reexport_records_the_exported_name() {
        let dir = project(&[
            (
                "lib.rs",
                "mod internal;\npub use internal::WorldPlugin as GamePlugin;\n",
            ),
            ("internal.rs", "impl Plugin for WorldPlugin {}\n"),
        ]);
        assert_eq!(
            detect_plugin(&dir, "my_game").as_deref(),
            Some("my_game::GamePlugin")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Exporting one type from a private module does not make its
    /// neighbours reachable.
    #[test]
    fn a_named_reexport_of_another_type_does_not_rescue_the_module() {
        let dir = project(&[
            ("lib.rs", "mod internal;\npub use internal::Other;\n"),
            (
                "internal.rs",
                "pub struct Other;\nimpl Plugin for WorldPlugin {}\n",
            ),
        ]);
        assert_eq!(detect_plugin(&dir, "my_game"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A module that may not be compiled cannot be relied on to hold
    /// a plugin the crate can export.
    #[test]
    fn a_conditionally_compiled_module_is_not_walked() {
        let dir = project(&[
            (
                "lib.rs",
                "#[cfg(target_os = \"nintendo64\")]\npub mod nope;\n",
            ),
            ("nope.rs", "impl Plugin for GamePlugin {}\n"),
        ]);
        assert!(plugin_candidates(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_path_attribute_module_is_followed() {
        let dir = project(&[
            ("lib.rs", "#[path = \"renamed.rs\"]\npub mod game;\n"),
            ("renamed.rs", "impl Plugin for WorldPlugin {}\n"),
        ]);
        assert_eq!(
            detect_plugin(&dir, "my_game").as_deref(),
            Some("my_game::game::WorldPlugin")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A `#[cfg]`-gated impl may not exist in the build the shim links,
    /// so naming it risks a reference to a plugin that is not there.
    /// Missing it only costs the user a prompt.
    #[test]
    fn a_conditionally_compiled_impl_is_not_offered() {
        let dir = project(&[(
            "lib.rs",
            "#[cfg(feature = \"extra\")]\nimpl Plugin for MaybePlugin {}\n",
        )]);
        assert!(plugin_candidates(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unconditional_impl_beside_a_gated_one_still_wins() {
        let dir = project(&[(
            "lib.rs",
            "#[cfg(test)]\nimpl Plugin for TestPlugin {}\n\
             impl Plugin for GamePlugin {}\n",
        )]);
        assert_eq!(
            detect_plugin(&dir, "my_game").as_deref(),
            Some("my_game::GamePlugin")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_generic_plugin_impl_is_found() {
        let dir = project(&[(
            "lib.rs",
            "impl<S: States> Plugin for StatePlugin<S> {\n    fn build(&self, _: &mut App) {}\n}\n",
        )]);
        assert_eq!(plugin_candidates(&dir), ["StatePlugin"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_qualified_impl_target_keeps_its_last_segment() {
        let dir = project(&[("lib.rs", "impl Plugin for crate::game::DeepPlugin {}\n")]);
        assert_eq!(plugin_candidates(&dir), ["DeepPlugin"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A plugin in a binary is not reachable from the library the
    /// editor links. Only `lib.rs` and what it declares is walked, so
    /// binaries never enter the scan.
    #[test]
    fn binary_sources_are_not_scanned() {
        let dir = project(&[
            ("lib.rs", "pub fn helper() {}\n"),
            ("main.rs", "impl Plugin for BinaryPlugin {}\n"),
            ("bin/tool.rs", "impl Plugin for ToolPlugin {}\n"),
        ]);
        assert!(plugin_candidates(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file that is not part of the module tree is not part of the
    /// crate, however tempting its name.
    #[test]
    fn an_undeclared_file_is_not_scanned() {
        let dir = project(&[
            ("lib.rs", "pub fn helper() {}\n"),
            ("orphan.rs", "impl Plugin for OrphanPlugin {}\n"),
        ]);
        assert!(plugin_candidates(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_extension_impl_is_found_in_a_submodule() {
        let dir = project(&[
            ("lib.rs", "pub mod ext;\n"),
            ("ext.rs", "impl JackdawExtension for MyExtension {}\n"),
        ]);
        let (_, name) = detect_extension(&dir).expect("extension detected");
        assert_eq!(name, "MyExtension");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Source that does not parse yields nothing rather than a partial,
    /// misleading answer.
    #[test]
    fn unparseable_source_yields_no_candidates() {
        let dir = project(&[("lib.rs", "impl Plugin for {{{ not rust\n")]);
        assert!(plugin_candidates(&dir).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
