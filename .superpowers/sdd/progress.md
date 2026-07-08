# BSN Migration - Subagent-Driven Development Ledger

Plan: /home/joe/Workspace/jackdaw/.scratch/bsn-migration/issues/01-replace-jsn-with-bsn.md
Worktree: /home/joe/Workspace/jackdaw/.claude/worktrees/bsn-migration
Branch: feat/bsn-migration (based on feat/native-feathers-widgets-019 @ 0c2ff34)
Baseline: `cargo check --workspace --all-targets` clean (exit 0) @ 0c2ff34. Full test baseline skipped per Joe (background runs kept getting reaped on session boundaries; per-task verification + final whole-branch suite instead). Known pre-existing flakes: operator_undo, navmesh (test-only, already red on main). Pre-existing harmless warning: duplicate example target name `basic` in jackdaw and bevy_window_chrome.

Rules for this run:
- Local checkpoint commits per task are approved by Joe (2026-07-08). Never push. Joe squashes into one PR commit himself.
- Commit subjects: one line, describe the code change, no task/stage references, no co-author trailers.
- Global constraints for all subagents: no em dashes anywhere; no brand names (other engines/editors) in code, comments, or docs; comments describe what code does now (no plan/process references, no "Phase N"/"Stage N"); no macros for DRY shortcuts; production systems/observers return Result<T, BevyError> and bubble with ?, tests may expect; prefer embedded_asset! over runtime loading; match on principal discriminant (no tuple matches with wildcarded operand); observers/events over per-frame polling for reactive UI; native feathers styling for any UI.

Model plan (implementers): 1-4 sonnet, 5 opus, 6 fable, 7 opus, 8 fable, 9 opus, 10 sonnet, 11 opus, 12 sonnet, 13 opus, 14 opus, 15 fable, 16 opus, 17 opus, 18 opus, 19 fable, 20 sonnet, 21 opus, 22 opus, 23 sonnet, 24 sonnet, 25 sonnet. Reviewers: sonnet for mechanical diffs, opus for integration-heavy diffs. Escalate one tier on BLOCKED.

## Progress

Task 1: complete (commit aee5303, review clean). Deferred minor: jackdaw_scene_types/src doc still says `use jackdaw_jsn::EditorCategory` (ignore doctest) and SkipSerialization doc says "never lands in .jsn" - fix at Task 4 (consumer repoint) or Task 25 (docs).
Task 2: complete (commit c96afb0). Moved types.rs, brush_chunks.rs, mesh_rebuild.rs (+ jd_grid.png embedded asset) into jackdaw_scene_types with re-exports from jackdaw_jsn; render feature mirrors jackdaw_jsn (default=render enables jackdaw_geometry/render). Workspace check clean, 52 tests pass. Note: --no-default-features build of scene_types fails on render-gated jackdaw_geometry symbols in types.rs - this matches original jackdaw_jsn behavior (types.rs always needed render), NOT a regression. Review pending (running).

Process notes:
- Subagent commits are hook-blocked; controller commits after review. Tell implementers explicitly NOT to run git commit AND not to background their cargo verify (background jobs get reaped on session boundaries) - run cargo foreground.
- Do NOT `git checkout` progress.md; edit it in place. (Lost Task 1 entry once this way; recovered from git log.)
