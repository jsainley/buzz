//! OpenClaw parallelism contract tests for the individual snapshot import path.
//! Extracted from `import.rs` to keep that file within the line-size ratchet.

use super::import::{resolve_snapshot_import_behavior, snapshot_instance_parallelism};
use crate::managed_agents::OPENCLAW_MAX_PARALLELISM;

/// `resolve_snapshot_import_behavior` returns the REQUESTED parallelism
/// from the snapshot unchanged — the cap is applied only when constructing
/// the instance record inside `confirm_agent_snapshot_import`.
#[test]
fn individual_import_openclaw_definition_stores_requested_parallelism() {
    let behavior = resolve_snapshot_import_behavior(
        Some("owner-only"),
        &[],
        Some(10), // requested value above the OpenClaw cap
        false,
    )
    .expect("resolve_snapshot_import_behavior must succeed");

    assert_eq!(
        behavior.parallelism,
        Some(10),
        "individual import: behavior.parallelism must be 10 (not clamped)"
    );
}

/// The instance parallelism path in `confirm_agent_snapshot_import` calls
/// `snapshot_instance_parallelism(runtime_id, requested)`. This test drives
/// that shared production seam — removing or changing it fails this test.
#[test]
fn individual_import_openclaw_instance_parallelism_is_capped() {
    assert_eq!(
        snapshot_instance_parallelism(Some("openclaw"), Some(10)),
        OPENCLAW_MAX_PARALLELISM,
        "individual import: OpenClaw instance parallelism must be capped at {}",
        OPENCLAW_MAX_PARALLELISM
    );
}

/// Non-OpenClaw harnesses pass through unchanged via snapshot_instance_parallelism.
#[test]
fn individual_import_goose_instance_parallelism_is_not_capped() {
    assert_eq!(
        snapshot_instance_parallelism(Some("goose"), Some(10)),
        10,
        "goose individual import: parallelism 10 must not be capped"
    );
}
