use std::{fs::File, path::PathBuf};

use crate::prelude::*;
use bevy::prelude::*;
use jackdaw_api_internal::lifecycle::OperatorEntity;

pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<DocumentOperatorsOp>();
    ctx.register_menu_entry::<DocumentOperatorsOp>(TopLevelMenu::Tools);
}

#[operator(
    id = "operators.document",
    label = "Document Operators",
    description = "Writes all available operators into a document",
    // since the output of this operator is a file system operation,
    // the snapshot system can't really do anything to undo it.
    allows_undo = false
)]
fn document_operators(
    _params: In<OperatorParameters>,
    ops: Query<&OperatorEntity>,
) -> OperatorResult {
    let mut md = "## Available Operators\n\n".to_string();
    for op in ops.iter() {
        md.push_str(&format!("### {} (`{}`)\n\n", op.label(), op.id()));
        let flags = [
            op.allows_undo().then_some("undo"),
            op.is_modal().then_some("modal"),
        ]
        .into_iter()
        .flatten()
        .map(|t| format!("*{t}*"))
        .collect::<Vec<_>>()
        .join(", ");
        if !flags.is_empty() {
            md.push_str(&format!("**flags**: {flags}\n\n"));
        }
        md.push_str(&format!("{}\n\n", op.description()));
    }
    let path = PathBuf::from("operators.md");
    let mut file = match File::create(&path) {
        Ok(file) => file,
        Err(e) => {
            error!("Failed to create operators.md: {e}");
            return OperatorResult::Cancelled;
        }
    };
    // done after creation because path.canonicalize() fails if the path doesn't exist
    let canonical_path = path
        .canonicalize()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "operators.md".to_string());
    match std::io::Write::write_all(&mut file, md.as_bytes()) {
        Ok(_) => {
            info!("Exported all available operators to {canonical_path}",);
            OperatorResult::Finished
        }
        Err(e) => {
            error!("Failed to write operators to {canonical_path}: {e}",);
            OperatorResult::Cancelled
        }
    }
}
