//! Migrate a bin-only project's `main.rs` into a `GamePlugin` library.
//!
//! After `jackdaw init` writes a `src/lib.rs` stub for a project that had no
//! library target, the editor still can't see the user's components, because
//! reflected types only register from the linked library, not the binary. This
//! module lifts the user's game code out of `main.rs` into the library's
//! `GamePlugin` so the editor picks it up, matching the shape the new-project
//! template ships with:
//!
//! - component / resource / system definitions move into `src/lib.rs`,
//! - their `App` builder calls (`add_systems`, `insert_resource`, ...) move into
//!   `GamePlugin::build`, with gameplay `Update`-family systems gated behind
//!   `play_gate::is_playing` so they don't run while editing,
//! - `main.rs` keeps `DefaultPlugins`, ambient plugins, and `run()`, and gains
//!   one `add_plugins(<crate>::GamePlugin)`.
//!
//! It is a best-effort transform over the common Bevy `main` shapes (a single
//! `App::new()....run()` chain, or `let mut app = App::new(); ...; app.run();`).
//! Anything it can't follow confidently is reported as
//! [`MigrationError::Unsupported`] and nothing is written; the caller falls back
//! to the manual "move your code yourself" guidance. The generated files are
//! re-parsed before the plan is returned, so a malformed result is an error
//! rather than corrupt source on disk.

use std::path::Path;

use bevy::app::AppExit;
use proc_macro2::LineColumn;
use syn::spanned::Spanned;

/// A proposed migration: the new file contents plus a human-readable summary.
/// Nothing is written until [`apply_migration`] runs.
#[derive(Debug)]
pub struct MigrationPlan {
    /// Proposed full contents of `src/lib.rs` (replaces the stub).
    pub lib_rs: String,
    /// Proposed full contents of `src/main.rs`.
    pub main_rs: String,
    /// Item names moved into the library (`Player`, `move_player`, ...).
    pub moved_items: Vec<String>,
    /// `App` builder calls relocated into `GamePlugin::build`.
    pub moved_calls: Vec<String>,
    /// Things left in `main.rs` on purpose, or caveats worth surfacing.
    pub notes: Vec<String>,
}

/// Why a migration could not be produced. On any of these the caller writes
/// nothing and keeps the manual guidance.
#[derive(Debug)]
pub enum MigrationError {
    NoMainRs,
    Io(String),
    Parse(String),
    /// The `main.rs` shape isn't one we can rewrite safely; the string explains
    /// what tripped it up so the user can migrate by hand.
    Unsupported(String),
}

impl std::fmt::Display for MigrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MigrationError::NoMainRs => write!(f, "no src/main.rs to migrate"),
            MigrationError::Parse(e) => write!(f, "could not parse src/main.rs: {e}"),
            MigrationError::Io(e) | MigrationError::Unsupported(e) => write!(f, "{e}"),
        }
    }
}

/// CLI entry point for `jackdaw migrate`. Run from a project directory; previews
/// the migration by default, writes only with `--apply`.
#[expect(
    clippy::print_stdout,
    clippy::print_stderr,
    reason = "CLI subcommand writes its results and errors to the terminal"
)]
pub fn run_migrate_cli(args: &[String]) -> AppExit {
    let apply = args.iter().any(|a| a == "--apply");
    let root = match std::env::current_dir() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("jackdaw migrate: {e}");
            return AppExit::error();
        }
    };
    let crate_name = match crate_name_of(&root) {
        Ok(n) => n,
        Err(e) => {
            eprintln!("jackdaw migrate: {e}");
            return AppExit::error();
        }
    };
    let plan = match plan_migration(&root, &crate_name) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("jackdaw migrate: {e}");
            return AppExit::error();
        }
    };

    println!("jackdaw migrate: {}", root.display());
    if !plan.moved_items.is_empty() {
        println!("  move into src/lib.rs: {}", plan.moved_items.join(", "));
    }
    if !plan.moved_calls.is_empty() {
        println!("  into GamePlugin::build: {}", plan.moved_calls.join(", "));
    }
    for note in &plan.notes {
        println!("  note: {note}");
    }

    if !apply {
        println!("\n--- proposed src/lib.rs ---\n{}", plan.lib_rs);
        println!("--- proposed src/main.rs ---\n{}", plan.main_rs);
        println!("Run `jackdaw migrate --apply` to write these (original main.rs -> main.rs.bak).");
        return AppExit::Success;
    }

    match apply_migration(&root, &plan) {
        Ok(()) => {
            println!("\nApplied. Original saved as src/main.rs.bak.");
            AppExit::Success
        }
        Err(e) => {
            eprintln!("jackdaw migrate: {e}");
            AppExit::error()
        }
    }
}

/// Read the library crate name (`snake_case`) from the project's Cargo.toml.
pub fn crate_name_of(root: &Path) -> Result<String, MigrationError> {
    let text = std::fs::read_to_string(root.join("Cargo.toml"))
        .map_err(|_| MigrationError::Io("no Cargo.toml in the current directory".into()))?;
    let doc: toml_edit::DocumentMut = text
        .parse()
        .map_err(|e| MigrationError::Parse(format!("Cargo.toml: {e}")))?;
    let name = doc
        .get("package")
        .and_then(|p| p.get("name"))
        .and_then(|n| n.as_str())
        .ok_or_else(|| MigrationError::Unsupported("Cargo.toml has no [package] name".into()))?;
    Ok(name.replace('-', "_"))
}

/// Build a migration plan for the project at `root` without touching any files.
/// `crate_name` is the library crate name (`snake_case`) the editor links.
pub fn plan_migration(root: &Path, crate_name: &str) -> Result<MigrationPlan, MigrationError> {
    let main_path = root.join("src/main.rs");
    if !main_path.is_file() {
        return Err(MigrationError::NoMainRs);
    }
    let source =
        std::fs::read_to_string(&main_path).map_err(|e| MigrationError::Io(format!("{e}")))?;
    let file = syn::parse_file(&source).map_err(|e| MigrationError::Parse(format!("{e}")))?;

    let offsets = LineOffsets::new(&source);

    // Locate `fn main`.
    let main_fn = file
        .items
        .iter()
        .enumerate()
        .find_map(|(i, item)| match item {
            syn::Item::Fn(f) if f.sig.ident == "main" => Some((i, f)),
            _ => None,
        })
        .ok_or_else(|| MigrationError::Unsupported("src/main.rs has no `fn main`".into()))?;
    let (_main_idx, main_fn) = main_fn;

    let (builder, anchor) = find_builder(&main_fn.block)?;
    let classified = classify_builder(&builder, anchor, &offsets)?;

    // Items to move: everything top-level except `fn main`, `use`s, and
    // `extern crate`. `use`s are duplicated into the library instead. Moved
    // items are deleted from `main.rs` (recorded as line ranges).
    let mut moved_items = Vec::new();
    let mut moved_item_text = Vec::new();
    let mut use_text = Vec::new();
    let mut item_deletes: Vec<(usize, usize)> = Vec::new();
    for item in &file.items {
        match item {
            syn::Item::Fn(f) if f.sig.ident == "main" => {}
            syn::Item::Use(_) => {
                use_text.push(slice_item(&source, &offsets, item, false)?.0);
            }
            syn::Item::ExternCrate(_) => {}
            _ => {
                let (text, name) = slice_item(&source, &offsets, item, true)?;
                if let Some(name) = name {
                    moved_items.push(name);
                }
                moved_item_text.push(text);

                let s = offsets.to_byte(item.span().start());
                let e = offsets.to_byte(item.span().end());
                let del_start = line_start_of(&source, s);
                let del_end = consume_trailing_blank(&source, end_of_line(&source, e));
                item_deletes.push((del_start, del_end));
            }
        }
    }

    let lib_rs = render_lib(crate_name, &use_text, &classified.moved, &moved_item_text);
    let main_rs = render_main(&source, &offsets, crate_name, &classified, &item_deletes)?;

    // Never hand back source we can't even parse.
    syn::parse_file(&lib_rs)
        .map_err(|e| MigrationError::Unsupported(format!("generated lib.rs didn't parse: {e}")))?;
    syn::parse_file(&main_rs)
        .map_err(|e| MigrationError::Unsupported(format!("generated main.rs didn't parse: {e}")))?;

    let mut notes = Vec::new();
    if !classified.kept_plugins.is_empty() {
        notes.push(format!(
            "left in main.rs (ambient plugins): {}",
            classified.kept_plugins.join(", ")
        ));
    }
    if classified.moved.is_empty() {
        notes.push("no game systems/resources found to move; only definitions relocated".into());
    }
    if classified.default_plugins.is_some() {
        notes.push("wrapped DefaultPlugins with maybe_windowless so embedded Play works".into());
    } else {
        notes.push(
            "no DefaultPlugins call found; add `maybe_windowless` by hand for embedded Play".into(),
        );
    }

    Ok(MigrationPlan {
        lib_rs,
        main_rs,
        moved_items,
        moved_calls: classified.moved.iter().map(|c| c.label.clone()).collect(),
        notes,
    })
}

/// Write a plan to disk: `src/lib.rs` (replaces the stub) and `src/main.rs`
/// (original saved as `src/main.rs.bak`).
pub fn apply_migration(root: &Path, plan: &MigrationPlan) -> Result<(), MigrationError> {
    let main_path = root.join("src/main.rs");
    let bak_path = root.join("src/main.rs.bak");
    let original =
        std::fs::read_to_string(&main_path).map_err(|e| MigrationError::Io(format!("{e}")))?;
    std::fs::write(&bak_path, original).map_err(|e| MigrationError::Io(format!("{e}")))?;
    std::fs::write(&main_path, &plan.main_rs).map_err(|e| MigrationError::Io(format!("{e}")))?;
    std::fs::write(root.join("src/lib.rs"), &plan.lib_rs)
        .map_err(|e| MigrationError::Io(format!("{e}")))?;
    Ok(())
}

/// One relocated `App` builder call, e.g. `add_systems(Update, move_player)`.
struct MovedCall {
    /// The `app.<...>;` line emitted into `GamePlugin::build`.
    rendered: String,
    /// Short form for the summary (the method name + first arg).
    label: String,
}

struct Classified {
    /// Calls to relocate into `GamePlugin::build`, in source order.
    moved: Vec<MovedCall>,
    /// `[dot_start, end)` byte ranges in `main.rs` to delete (the moved calls).
    delete_ranges: Vec<(usize, usize)>,
    /// Byte offset of the `.run(` dot (chained) or `app.run(` statement start
    /// (let form), where `add_plugins(GamePlugin)` is injected.
    inject_at: usize,
    /// Whether the builder is the `let mut app` statement form.
    let_form: bool,
    /// `add_plugins(..)` left in main (`DefaultPlugins` / ambient), for the notes.
    kept_plugins: Vec<String>,
    /// Line-start byte of the builder statement (`App::new()` / `let mut app`),
    /// where the `let default_plugins = ...` pie-wrap is inserted.
    builder_stmt_start: usize,
    /// The kept `add_plugins(DefaultPlugins...)` argument: its `[start, end)`
    /// byte range and verbatim text, used to wrap it with `maybe_windowless` so
    /// embedded Play works without hand-editing. `None` if no `DefaultPlugins`
    /// `add_plugins` call was found.
    default_plugins: Option<(usize, usize, String)>,
}

/// The builder expression we found in `fn main`, plus enough context to rewrite.
enum Builder {
    /// A `App::new().a().b().run()` method-call chain. Holds the `run()` call.
    Chain(syn::ExprMethodCall),
    /// `let mut app = App::new(); app.a(); ...; app.run();`. Holds each
    /// `app.method(..)` statement call and the `app.run()` call.
    Let {
        calls: Vec<syn::ExprMethodCall>,
        run: syn::ExprMethodCall,
    },
}

/// Find the `App` builder in `fn main`'s body. Returns it plus the span of the
/// builder's anchor (`App::new()` for a chain, the `let mut app` statement for
/// the let form), whose line is where the pie-wrap `let`s are inserted.
fn find_builder(block: &syn::Block) -> Result<(Builder, proc_macro2::Span), MigrationError> {
    // Chained form: a trailing expr or expr-statement that is a method-call
    // chain bottoming out at `App::new()`.
    for stmt in &block.stmts {
        let syn::Stmt::Expr(expr, _) = stmt else {
            continue;
        };
        if let syn::Expr::MethodCall(mc) = expr
            && chain_roots_at_app_new(expr)
        {
            if mc.method != "run" {
                return Err(MigrationError::Unsupported(
                    "the App builder chain doesn't end in `.run()`; migrate by hand".into(),
                ));
            }
            let anchor = find_app_new_span(expr).unwrap_or_else(|| expr.span());
            return Ok((Builder::Chain(mc.clone()), anchor));
        }
    }

    // Let form: `let mut app = App::new();` then `app.method(..);` then
    // `app.run();`.
    let app = block.stmts.iter().find_map(|stmt| {
        let syn::Stmt::Local(local) = stmt else {
            return None;
        };
        let init = local.init.as_ref()?;
        if expr_is_app_new(&init.expr)
            && let syn::Pat::Ident(p) = &local.pat
        {
            return Some((p.ident.clone(), local.span()));
        }
        None
    });
    if let Some((app_ident, anchor)) = app {
        let mut calls = Vec::new();
        let mut run = None;
        for stmt in &block.stmts {
            let syn::Stmt::Expr(expr, _) = stmt else {
                continue;
            };
            if let syn::Expr::MethodCall(mc) = expr
                && expr_is_ident(&mc.receiver, &app_ident)
            {
                if mc.method == "run" {
                    run = Some(mc.clone());
                } else {
                    calls.push(mc.clone());
                }
            }
        }
        let run = run.ok_or_else(|| {
            MigrationError::Unsupported(
                "found `App::new()` but no `app.run()`; migrate by hand".into(),
            )
        })?;
        return Ok((Builder::Let { calls, run }, anchor));
    }

    Err(MigrationError::Unsupported(
        "couldn't find an `App::new()...run()` builder in main; migrate by hand".into(),
    ))
}

/// True if `expr` is a method-call chain whose innermost receiver is
/// `App::new()`.
fn chain_roots_at_app_new(expr: &syn::Expr) -> bool {
    match expr {
        syn::Expr::MethodCall(mc) => chain_roots_at_app_new(&mc.receiver),
        other => expr_is_app_new(other),
    }
}

/// Span of the `App::new()` call at the root of a builder chain.
fn find_app_new_span(expr: &syn::Expr) -> Option<proc_macro2::Span> {
    match expr {
        syn::Expr::MethodCall(mc) => find_app_new_span(&mc.receiver),
        e if expr_is_app_new(e) => Some(e.span()),
        _ => None,
    }
}

fn expr_is_app_new(expr: &syn::Expr) -> bool {
    let syn::Expr::Call(call) = expr else {
        return false;
    };
    let syn::Expr::Path(p) = &*call.func else {
        return false;
    };
    let segs: Vec<String> = p
        .path
        .segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect();
    matches!(segs.as_slice(), [.., a, b] if a == "App" && b == "new")
}

fn expr_is_ident(expr: &syn::Expr, ident: &syn::Ident) -> bool {
    matches!(expr, syn::Expr::Path(p) if p.path.is_ident(ident))
}

/// Classify each builder call into keep-in-main vs move-to-GamePlugin and record
/// the byte edits needed to rewrite `main.rs`.
fn classify_builder(
    builder: &Builder,
    anchor: proc_macro2::Span,
    offsets: &LineOffsets<'_>,
) -> Result<Classified, MigrationError> {
    let mut moved = Vec::new();
    let mut delete_ranges = Vec::new();
    let mut kept_plugins = Vec::new();
    let mut default_plugins = None;

    let mut handle = |mc: &syn::ExprMethodCall| -> Result<(), MigrationError> {
        let method = mc.method.to_string();
        if keep_in_main(&method, mc, offsets) {
            if method == "add_plugins" {
                let arg = first_arg_text(mc, offsets);
                // Capture the DefaultPlugins argument so it can be wrapped with
                // `maybe_windowless` for embedded Play. First one wins.
                if default_plugins.is_none()
                    && arg.trim_start().starts_with("DefaultPlugins")
                    && let Some(first) = mc.args.first()
                {
                    let start = offsets.to_byte(first.span().start());
                    let end = offsets.to_byte(first.span().end());
                    default_plugins = Some((start, end, arg.clone()));
                }
                kept_plugins.push(arg);
            }
            return Ok(());
        }
        // Move it. Record the delete range (`.method(..)` for chains, whole
        // statement handled by caller for the let form via the same range +
        // trailing `;`).
        let dot = offsets.to_byte(mc.dot_token.span().start());
        let end = offsets.to_byte(mc.span().end());
        delete_ranges.push((dot, end));
        moved.push(render_moved_call(mc, offsets));
        Ok(())
    };

    let (inject_at, let_form) = match builder {
        Builder::Chain(run) => {
            // Walk inner calls (everything between App::new() and run()).
            let mut chain = Vec::new();
            collect_chain(&run.receiver, &mut chain);
            for mc in &chain {
                handle(mc)?;
            }
            // Inject `.add_plugins(GamePlugin)` just before `.run()`.
            (offsets.to_byte(run.dot_token.span().start()), false)
        }
        Builder::Let { calls, run } => {
            for mc in calls {
                handle(mc)?;
            }
            // Inject `app.add_plugins(GamePlugin);` before the `app.run()`
            // statement. Delete-ranges for moved statements get widened to the
            // full statement (incl. the trailing `;`) below.
            (offsets.to_byte(run.span().start()), true)
        }
    };

    let builder_stmt_start = line_start_of(offsets.src, offsets.to_byte(anchor.start()));

    Ok(Classified {
        moved,
        delete_ranges,
        inject_at,
        let_form,
        kept_plugins,
        builder_stmt_start,
        default_plugins,
    })
}

/// Collect a method-call chain's calls (excluding the terminal one already held)
/// from innermost to outermost, skipping the `App::new()` root.
fn collect_chain(expr: &syn::Expr, out: &mut Vec<syn::ExprMethodCall>) {
    if let syn::Expr::MethodCall(mc) = expr {
        collect_chain(&mc.receiver, out);
        out.push(mc.clone());
    }
}

/// Calls that stay in `main.rs`: app/window infrastructure and the
/// `DefaultPlugins`/`MinimalPlugins` set (ambient plugins).
fn keep_in_main(method: &str, mc: &syn::ExprMethodCall, offsets: &LineOffsets<'_>) -> bool {
    match method {
        "set_error_handler" | "set_runner" | "run" => true,
        "add_plugins" => {
            let arg = first_arg_text(mc, offsets);
            arg.contains("DefaultPlugins")
                || arg.contains("MinimalPlugins")
                || arg.contains("WindowPlugin")
        }
        _ => false,
    }
}

/// Render a moved builder call as a `app.<method>(<args>);` line, gating
/// gameplay-schedule systems behind `play_gate::is_playing`. Argument text is
/// sliced verbatim from the source so the user's formatting is preserved.
fn render_moved_call(mc: &syn::ExprMethodCall, offsets: &LineOffsets<'_>) -> MovedCall {
    let method = mc.method.to_string();
    let args: Vec<&syn::Expr> = mc.args.iter().collect();

    let rendered = if method == "add_systems" && args.len() == 2 && is_gated_schedule(args[0]) {
        let schedule = offsets.slice_span(args[0].span());
        let systems = offsets.slice_span(args[1].span());
        format!("app.add_systems({schedule}, ({systems}).run_if(play_gate::is_playing));")
    } else {
        let arg_text = offsets.slice_args(mc);
        format!("app.{method}({arg_text});")
    };

    let label = format!("{method}({})", first_arg_text(mc, offsets));
    MovedCall { rendered, label }
}

/// Gameplay schedules whose systems should only run during Play.
fn is_gated_schedule(expr: &syn::Expr) -> bool {
    use quote::ToTokens;
    let text = expr.to_token_stream().to_string();
    const GATED: &[&str] = &[
        "Update",
        "FixedUpdate",
        "PreUpdate",
        "PostUpdate",
        "FixedPreUpdate",
        "FixedPostUpdate",
        "FixedFirst",
        "FixedLast",
    ];
    let last = text.rsplit("::").next().unwrap_or(&text).trim();
    GATED.contains(&last)
}

/// Verbatim source text of a call's first argument, for summaries and the
/// ambient-plugin check.
fn first_arg_text(mc: &syn::ExprMethodCall, offsets: &LineOffsets<'_>) -> String {
    mc.args
        .first()
        .map(|a| offsets.slice_span(a.span()).to_string())
        .unwrap_or_default()
}

/// Render the new `src/lib.rs`: header, imports, `play_gate`, the filled
/// `GamePlugin`, then the verbatim moved item definitions.
fn render_lib(
    crate_name: &str,
    use_text: &[String],
    moved_calls: &[MovedCall],
    moved_items: &[String],
) -> String {
    let mut out = String::new();
    out.push_str(
        "//! Gameplay shared between the standalone binary (`cargo run`) and the\n\
         //! editor binary (`cargo editor`). Migrated from `main.rs` by `jackdaw`.\n\
         //!\n\
         //! Component, resource, and system definitions live here so the editor\n\
         //! can discover their reflected types; `main.rs` keeps only the window\n\
         //! and ambient-plugin setup.\n\n",
    );

    // Imports: always bevy + runtime preludes, then whatever main imported.
    out.push_str("use bevy::prelude::*;\n");
    out.push_str("use jackdaw_runtime::prelude::*;\n");
    let mut seen_bevy_prelude = false;
    for u in use_text {
        let trimmed = u.trim();
        if trimmed.contains("bevy :: prelude") || trimmed.contains("bevy::prelude") {
            seen_bevy_prelude = true;
        }
        // Skip the two we already emit to avoid duplicate-import errors.
        if trimmed == "use bevy::prelude::*;" || trimmed == "use jackdaw_runtime::prelude::*;" {
            continue;
        }
        out.push_str(u);
        if !u.ends_with('\n') {
            out.push('\n');
        }
    }
    let _ = seen_bevy_prelude;
    out.push('\n');

    // GamePlugin.
    out.push_str(
        "/// Your game's Bevy plugin. The editor links this so your components\n\
         /// show up in the inspector; the standalone binary adds it too. Gameplay\n\
         /// systems gated by [`play_gate::is_playing`] run only during Play.\n\
         #[derive(Default)]\n\
         pub struct GamePlugin;\n\n\
         impl Plugin for GamePlugin {\n\
         \x20   fn build(&self, app: &mut App) {\n",
    );
    if moved_calls.is_empty() {
        out.push_str("        // TODO: add your systems and resources here.\n");
    } else {
        for call in moved_calls {
            out.push_str("        ");
            out.push_str(&call.rendered);
            out.push('\n');
        }
    }
    out.push_str("    }\n}\n\n");

    // play_gate, matching the template so `.run_if(play_gate::is_playing)`
    // resolves in both editor and standalone builds.
    out.push_str(PLAY_GATE_MODULE);
    out.push('\n');

    // Moved item definitions, verbatim.
    for (i, item) in moved_items.iter().enumerate() {
        out.push_str(item.trim_end());
        out.push('\n');
        if i + 1 < moved_items.len() {
            out.push('\n');
        }
    }
    let _ = crate_name;
    out
}

const PLAY_GATE_MODULE: &str = "\
/// Bridges the editor's `PlayState` to gameplay without forcing a `jackdaw` dep\n\
/// in standalone builds. Always `true` without the `editor` feature; gates on\n\
/// `PlayState::Playing` when the editor is compiled in.\n\
pub mod play_gate {\n\
\x20   #[cfg(feature = \"editor\")]\n\
\x20   pub fn is_playing(\n\
\x20       state: bevy::prelude::Res<bevy::state::state::State<jackdaw::prelude::PlayState>>,\n\
\x20   ) -> bool {\n\
\x20       matches!(*state.get(), jackdaw::prelude::PlayState::Playing)\n\
\x20   }\n\
\n\
\x20   #[cfg(not(feature = \"editor\"))]\n\
\x20   pub fn is_playing() -> bool {\n\
\x20       true\n\
\x20   }\n\
}\n";

/// Rewrite `main.rs` in place by byte surgery: delete the moved builder calls,
/// inject `add_plugins(<crate>::GamePlugin)`, preserving everything else
/// (comments, ambient plugins, let-bindings) verbatim.
fn render_main(
    source: &str,
    offsets: &LineOffsets<'_>,
    crate_name: &str,
    c: &Classified,
    item_deletes: &[(usize, usize)],
) -> Result<String, MigrationError> {
    let mut edits: Vec<(usize, usize, String)> = Vec::new();

    // Remove the top-level item definitions that moved into the library.
    for &(start, end) in item_deletes {
        edits.push((start, end, String::new()));
    }

    for &(start, end) in &c.delete_ranges {
        if c.let_form {
            // A whole statement (`    app.add_systems(..);`): drop the line.
            let stmt_start = line_start_of(source, start);
            let stmt_end = swallow_to_eol(source, end);
            edits.push((stmt_start, stmt_end, String::new()));
        } else if alone_on_line(source, start, end) {
            // A `.add_systems(..)` segment alone on its line: drop the line so
            // no blank/trailing-whitespace residue is left in the chain.
            let line_start = line_start_of(source, start);
            let line_end = end_of_line(source, end);
            edits.push((line_start, line_end, String::new()));
        } else {
            edits.push((start, end, String::new()));
        }
    }

    // Inject `add_plugins(GamePlugin)` with the same indentation as the `.run()`
    // (chain) or `app.run()` (let) line it sits next to.
    let indent = indent_at(source, c.inject_at);
    let (inject_at, inject) = if c.let_form {
        (
            line_start_of(source, c.inject_at),
            format!("{indent}app.add_plugins({crate_name}::GamePlugin);\n"),
        )
    } else {
        (
            c.inject_at,
            format!(".add_plugins({crate_name}::GamePlugin)\n{indent}"),
        )
    };
    edits.push((inject_at, inject_at, inject));

    // Wrap DefaultPlugins with `maybe_windowless` (under the `pie` feature) so
    // the game runs inside the editor's Game panel during Play. Replace the
    // add_plugins argument with `default_plugins` and bind it just before the
    // builder, mirroring the new-project template.
    if let Some((arg_start, arg_end, arg_text)) = &c.default_plugins {
        edits.push((*arg_start, *arg_end, "default_plugins".to_string()));
        let bi = indent_at(source, c.builder_stmt_start);
        let wrap = format!(
            "{bi}let default_plugins = {arg_text};\n\
             {bi}#[cfg(feature = \"pie\")]\n\
             {bi}let default_plugins = jackdaw_runtime::maybe_windowless(default_plugins);\n\n",
        );
        edits.push((c.builder_stmt_start, c.builder_stmt_start, wrap));
    }

    // Apply edits right-to-left so earlier offsets stay valid.
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    let mut out = source.to_string();
    for (start, end, text) in edits {
        if start > out.len() || end > out.len() || start > end {
            return Err(MigrationError::Unsupported(
                "internal: computed an out-of-range edit; migrate by hand".into(),
            ));
        }
        out.replace_range(start..end, &text);
    }
    let _ = offsets;
    Ok(out)
}

/// True if `[start, end)` is the only non-whitespace content on its line.
fn alone_on_line(s: &str, start: usize, end: usize) -> bool {
    let line_start = line_start_of(s, start);
    let before = &s[line_start..start];
    let eol = end_of_line(s, end);
    let after = &s[end..eol];
    before.chars().all(char::is_whitespace)
        && after
            .trim_end_matches('\n')
            .chars()
            .all(char::is_whitespace)
}

/// Byte offset just past the newline ending the line that contains `byte`.
fn end_of_line(s: &str, byte: usize) -> usize {
    s[byte..]
        .find('\n')
        .map(|i| byte + i + 1)
        .unwrap_or(s.len())
}

/// If the line starting at `pos` is blank, consume it (so removing an item
/// doesn't leave a stack of empty lines behind). Consumes at most one.
fn consume_trailing_blank(s: &str, pos: usize) -> usize {
    if pos >= s.len() {
        return pos;
    }
    let eol = end_of_line(s, pos);
    if s[pos..eol].trim().is_empty() {
        eol
    } else {
        pos
    }
}

/// Byte offset of the start of the line containing `byte`.
fn line_start_of(s: &str, byte: usize) -> usize {
    s[..byte].rfind('\n').map(|i| i + 1).unwrap_or(0)
}

/// From `byte`, consume up to and including the next `;` and trailing newline.
fn swallow_to_eol(s: &str, byte: usize) -> usize {
    let bytes = s.as_bytes();
    let mut i = byte;
    while i < bytes.len() && bytes[i] != b';' {
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b';' {
        i += 1;
    }
    if i < bytes.len() && bytes[i] == b'\n' {
        i += 1;
    }
    i
}

/// Leading-whitespace string of the line containing `byte`.
fn indent_at(s: &str, byte: usize) -> String {
    let start = line_start_of(s, byte);
    s[start..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

/// Slice an item's verbatim source text (comments + formatting preserved). When
/// `make_pub`, prepends `pub ` to a private item by inserting before its
/// keyword. Returns the text and the item's name (for the summary).
fn slice_item(
    source: &str,
    offsets: &LineOffsets<'_>,
    item: &syn::Item,
    make_pub: bool,
) -> Result<(String, Option<String>), MigrationError> {
    let start = offsets.to_byte(item.span().start());
    let end = offsets.to_byte(item.span().end());
    if start > end || end > source.len() {
        return Err(MigrationError::Unsupported(
            "internal: item span out of range; migrate by hand".into(),
        ));
    }
    let mut text = source[start..end].to_string();

    let (name, keyword_span, is_private) = item_vis_info(item);
    if make_pub
        && is_private
        && let Some(kw) = keyword_span
    {
        let kw_byte = offsets.to_byte(kw);
        if kw_byte >= start && kw_byte <= end {
            let rel = kw_byte - start;
            text.insert_str(rel, "pub ");
        }
    }
    Ok((text, name))
}

/// Item name, the span of its visibility keyword anchor (where `pub` goes), and
/// whether it is currently private. Items without visibility (impl blocks,
/// macro invocations) report `is_private = false`.
fn item_vis_info(item: &syn::Item) -> (Option<String>, Option<LineColumn>, bool) {
    macro_rules! vis {
        ($v:expr) => {
            matches!($v, syn::Visibility::Inherited)
        };
    }
    match item {
        syn::Item::Struct(i) => (
            Some(i.ident.to_string()),
            Some(i.struct_token.span().start()),
            vis!(i.vis),
        ),
        syn::Item::Enum(i) => (
            Some(i.ident.to_string()),
            Some(i.enum_token.span().start()),
            vis!(i.vis),
        ),
        syn::Item::Fn(i) => (
            Some(i.sig.ident.to_string()),
            Some(i.sig.fn_token.span().start()),
            vis!(i.vis),
        ),
        syn::Item::Const(i) => (
            Some(i.ident.to_string()),
            Some(i.const_token.span().start()),
            vis!(i.vis),
        ),
        syn::Item::Static(i) => (
            Some(i.ident.to_string()),
            Some(i.static_token.span().start()),
            vis!(i.vis),
        ),
        syn::Item::Type(i) => (
            Some(i.ident.to_string()),
            Some(i.type_token.span().start()),
            vis!(i.vis),
        ),
        syn::Item::Trait(i) => (
            Some(i.ident.to_string()),
            Some(i.trait_token.span().start()),
            vis!(i.vis),
        ),
        syn::Item::Mod(i) => (
            Some(i.ident.to_string()),
            Some(i.mod_token.span().start()),
            vis!(i.vis),
        ),
        syn::Item::Union(i) => (
            Some(i.ident.to_string()),
            Some(i.union_token.span().start()),
            vis!(i.vis),
        ),
        syn::Item::Impl(i) => (
            i.trait_
                .as_ref()
                .and_then(|(_, p, _)| p.segments.last())
                .map(|s| format!("impl {}", s.ident)),
            None,
            false,
        ),
        _ => (None, None, false),
    }
}

/// Maps `proc_macro2` 1-based line / 0-based-char-column locations to byte
/// offsets in the source, so spans can slice the original text. Column counts
/// characters (not bytes), so the conversion is char-aware to stay correct (and
/// panic-free) on source containing non-ASCII comments or string literals.
struct LineOffsets<'a> {
    src: &'a str,
    /// Byte offset where each line starts (line N at index N-1).
    line_starts: Vec<usize>,
}

impl<'a> LineOffsets<'a> {
    fn new(source: &'a str) -> Self {
        let mut line_starts = vec![0];
        for (i, b) in source.bytes().enumerate() {
            if b == b'\n' {
                line_starts.push(i + 1);
            }
        }
        LineOffsets {
            src: source,
            line_starts,
        }
    }

    fn to_byte(&self, lc: LineColumn) -> usize {
        if lc.line == 0 || lc.line > self.line_starts.len() {
            return self.src.len();
        }
        let line_start = self.line_starts[lc.line - 1];
        let rest = &self.src[line_start..];
        // Advance `column` characters from the line start, landing on a char
        // boundary; past the end clamps to the source length.
        rest.char_indices()
            .nth(lc.column)
            .map(|(i, _)| line_start + i)
            .unwrap_or(self.src.len())
    }

    /// Verbatim source text covered by `span`.
    fn slice_span(&self, span: proc_macro2::Span) -> &'a str {
        let start = self.to_byte(span.start());
        let end = self.to_byte(span.end());
        self.src.get(start..end).unwrap_or("")
    }

    /// Verbatim source text inside a method call's parentheses (all arguments
    /// as written), trimmed of surrounding whitespace.
    fn slice_args(&self, mc: &syn::ExprMethodCall) -> &'a str {
        let open = self.to_byte(mc.paren_token.span.open().end());
        let close = self.to_byte(mc.paren_token.span.close().start());
        self.src.get(open..close).unwrap_or("").trim()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(main_rs: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "jackdaw_migrate_{}_{}",
            std::process::id(),
            // Vary by content length so parallel tests don't collide.
            main_rs.len()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), main_rs).unwrap();
        std::fs::write(dir.join("src/lib.rs"), "pub struct GamePlugin;\n").unwrap();
        dir
    }

    const CHAINED: &str = r#"use bevy::prelude::*;

#[derive(Component, Reflect)]
#[reflect(Component)]
struct Player {
    // movement speed in m/s
    speed: f32,
}

fn move_player(time: Res<Time>, mut q: Query<(&Player, &mut Transform)>) {
    for (p, mut t) in &mut q {
        t.translation.x += p.speed * time.delta_secs();
    }
}

fn setup(mut commands: Commands) {
    commands.spawn(Camera3d::default());
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_systems(Startup, setup)
        .add_systems(Update, move_player)
        .insert_resource(ClearColor(Color::BLACK))
        .run();
}
"#;

    #[test]
    fn migrates_chained_builder() {
        let dir = project(CHAINED);
        let plan = plan_migration(&dir, "my_game").expect("plan");

        // Definitions moved and made public.
        assert!(plan.lib_rs.contains("pub struct Player"));
        assert!(plan.lib_rs.contains("pub fn move_player"));
        assert!(plan.lib_rs.contains("pub fn setup"));
        // Comment inside the struct is preserved verbatim.
        assert!(plan.lib_rs.contains("movement speed in m/s"));

        // Builder calls relocated into GamePlugin::build.
        assert!(plan.lib_rs.contains("app.add_systems(Startup, setup)"));
        assert!(plan.lib_rs.contains("app.insert_resource(ClearColor"));
        // Update systems gated behind play_gate.
        assert!(
            plan.lib_rs
                .contains("app.add_systems(Update, (move_player).run_if(play_gate::is_playing));")
        );
        // Startup systems are NOT gated.
        assert!(!plan.lib_rs.contains("Startup, (setup).run_if"));

        // main keeps DefaultPlugins + run, gains add_plugins(GamePlugin), and
        // dropped the moved calls.
        assert!(plan.main_rs.contains("add_plugins(my_game::GamePlugin)"));
        assert!(plan.main_rs.contains(".run();"));
        assert!(!plan.main_rs.contains("add_systems"));
        assert!(!plan.main_rs.contains("insert_resource"));
        // The moved definitions are gone from main (now only in the library).
        assert!(
            !plan.main_rs.contains("struct Player"),
            "Player moved out of main"
        );
        assert!(
            !plan.main_rs.contains("fn move_player"),
            "move_player moved out of main"
        );
        assert!(
            !plan.main_rs.contains("fn setup"),
            "setup moved out of main"
        );

        // Embedded Play is wired without hand-editing: DefaultPlugins is bound,
        // wrapped with maybe_windowless under the pie feature, and added.
        assert!(
            plan.main_rs
                .contains("let default_plugins = DefaultPlugins;")
        );
        assert!(plan.main_rs.contains("#[cfg(feature = \"pie\")]"));
        assert!(
            plan.main_rs
                .contains("jackdaw_runtime::maybe_windowless(default_plugins)")
        );
        assert!(plan.main_rs.contains("add_plugins(default_plugins)"));
        assert!(syn::parse_file(&plan.main_rs).is_ok());

        // Both outputs are valid Rust.
        assert!(syn::parse_file(&plan.lib_rs).is_ok());
        assert!(syn::parse_file(&plan.main_rs).is_ok());

        let _ = std::fs::remove_dir_all(&dir);
    }

    const LET_FORM: &str = r#"use bevy::prelude::*;

#[derive(Resource)]
struct Score(u32);

fn tick(mut s: ResMut<Score>) {
    s.0 += 1;
}

fn main() {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins);
    app.insert_resource(Score(0));
    app.add_systems(Update, tick);
    app.run();
}
"#;

    #[test]
    fn migrates_let_form_builder() {
        let dir = project(LET_FORM);
        let plan = plan_migration(&dir, "scorer").expect("plan");

        assert!(plan.lib_rs.contains("pub struct Score"));
        assert!(plan.lib_rs.contains("pub fn tick"));
        assert!(
            plan.lib_rs
                .contains("app.add_systems(Update, (tick).run_if(play_gate::is_playing));")
        );
        assert!(plan.lib_rs.contains("app.insert_resource(Score(0))"));

        // main keeps the let binding + DefaultPlugins + run, injects GamePlugin,
        // and drops the moved statements.
        assert!(plan.main_rs.contains("let mut app = App::new();"));
        assert!(plan.main_rs.contains("app.add_plugins(default_plugins);"));
        assert!(
            plan.main_rs
                .contains("app.add_plugins(scorer::GamePlugin);")
        );
        assert!(plan.main_rs.contains("app.run();"));
        assert!(!plan.main_rs.contains("add_systems"));
        assert!(!plan.main_rs.contains("insert_resource"));
        assert!(
            !plan.main_rs.contains("struct Score"),
            "Score moved out of main"
        );
        assert!(!plan.main_rs.contains("fn tick"), "tick moved out of main");
        // Embedded Play wiring for the let-form too.
        assert!(
            plan.main_rs
                .contains("let default_plugins = DefaultPlugins;")
        );
        assert!(
            plan.main_rs
                .contains("jackdaw_runtime::maybe_windowless(default_plugins)")
        );

        assert!(syn::parse_file(&plan.main_rs).is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn apply_writes_files_and_backup() {
        let dir = project(CHAINED);
        let plan = plan_migration(&dir, "my_game").expect("plan");
        apply_migration(&dir, &plan).expect("apply");

        let main_rs = std::fs::read_to_string(dir.join("src/main.rs")).unwrap();
        assert!(main_rs.contains("add_plugins(my_game::GamePlugin)"));
        let lib_rs = std::fs::read_to_string(dir.join("src/lib.rs")).unwrap();
        assert!(lib_rs.contains("pub struct Player"));
        // Original preserved.
        let bak = std::fs::read_to_string(dir.join("src/main.rs.bak")).unwrap();
        assert_eq!(bak, CHAINED);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn declines_unsupported_shape() {
        // No App::new() at all.
        let dir = project("fn main() {\n    println!(\"hi\");\n}\n");
        let err = plan_migration(&dir, "weird").unwrap_err();
        assert!(matches!(err, MigrationError::Unsupported(_)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn declines_when_no_main() {
        let dir =
            std::env::temp_dir().join(format!("jackdaw_migrate_nomain_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(matches!(
            plan_migration(&dir, "x"),
            Err(MigrationError::NoMainRs)
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
