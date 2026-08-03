//! Parallelism normalization and stale-pin tests for `apply_persona_snapshot`.
//!
//! Extracted to a sibling module to keep the main `tests` module within the
//! file-size ratchet while covering the parallelism cap and preset stale-pin
//! drop behavior in full.

use super::tests::{sample_persona, sample_record};
use crate::managed_agents::persona_events::apply_persona_snapshot;
use crate::managed_agents::types::AgentDefinition;

// ── Parallelism normalization ─────────────────────────────────────────────────

#[test]
fn apply_persona_snapshot_normalizes_openclaw_parallelism() {
    let mut record = sample_record();
    record.parallelism = 10;
    apply_persona_snapshot(
        &mut record,
        &AgentDefinition {
            runtime: Some("openclaw".into()),
            ..sample_persona()
        },
    );
    assert_eq!(
        record.parallelism,
        crate::managed_agents::OPENCLAW_MAX_PARALLELISM
    );
}

/// Persona runtime=None: must NOT cap regardless of stale agent_command value,
/// because record_agent_command falls back to the default (uncapped) command.
#[test]
fn apply_persona_snapshot_persona_runtime_none_is_not_capped() {
    // stale openclaw
    let mut record = sample_record();
    record.agent_command = "openclaw".to_string();
    record.parallelism = 10;
    apply_persona_snapshot(
        &mut record,
        &AgentDefinition {
            runtime: None,
            ..sample_persona()
        },
    );
    assert_eq!(
        record.parallelism, 10,
        "persona runtime=None must not cap via stale openclaw agent_command"
    );

    // stale goose (same expectation — default-command fallback, goose is uncapped anyway)
    let mut record2 = sample_record();
    record2.agent_command = "goose".to_string();
    record2.parallelism = 10;
    apply_persona_snapshot(
        &mut record2,
        &AgentDefinition {
            runtime: None,
            ..sample_persona()
        },
    );
    assert_eq!(
        record2.parallelism, 10,
        "persona runtime=None + stale goose must not cap"
    );
}

// ── Stale-pin drop: OpenClaw↔Goose (preset↔builtin) ─────────────────────────

/// Persona→OpenClaw: stale Goose override dropped, parallelism capped.
/// Regression for the original preset stale-pin fix.
#[test]
fn apply_persona_snapshot_goose_to_openclaw_drops_stale_goose_pin() {
    let mut record = sample_record();
    record.parallelism = 10;
    record.agent_command_override = Some("goose".to_string());
    apply_persona_snapshot(
        &mut record,
        &AgentDefinition {
            runtime: Some("openclaw".to_string()),
            ..sample_persona()
        },
    );
    assert_eq!(
        record.agent_command_override, None,
        "stale goose pin must be dropped"
    );
    assert_eq!(
        record.parallelism,
        crate::managed_agents::OPENCLAW_MAX_PARALLELISM
    );
}

/// Persona→Goose: stale OpenClaw override dropped, parallelism uncapped.
#[test]
fn apply_persona_snapshot_openclaw_to_goose_drops_stale_openclaw_pin() {
    let mut record = sample_record();
    record.parallelism = 5;
    record.agent_command_override = Some("openclaw".to_string());
    apply_persona_snapshot(
        &mut record,
        &AgentDefinition {
            runtime: Some("goose".to_string()),
            ..sample_persona()
        },
    );
    assert_eq!(
        record.agent_command_override, None,
        "stale openclaw pin must be dropped"
    );
    assert_eq!(record.parallelism, 5, "goose has no cap");
}

// ── Stale-pin drop: alias pin (command ≠ id) ─────────────────────────────────

/// Persona→OpenClaw; record has a stale `claude-code-acp` alias pin (id="claude",
/// command="claude-agent-acp"). The canonical resolver must recognise the alias
/// as the Claude harness and drop it when the persona switches to a different
/// harness (OpenClaw).
///
/// This is the correctness case that motivated the `canonical_harness_command`
/// resolver: `command_for_runtime_id("claude-code-acp")` returns `None`
/// (id-only lookup) so the old code treated the alias as a custom/unknown pin
/// and kept it — the agent kept running Claude uncapped instead of OpenClaw.
#[test]
fn apply_persona_snapshot_claude_alias_pin_to_openclaw_drops_stale_alias() {
    let mut record = sample_record();
    record.parallelism = 10;
    // "claude-code-acp" is an alias of the Claude runtime (id="claude").
    record.agent_command_override = Some("claude-code-acp".to_string());
    apply_persona_snapshot(
        &mut record,
        &AgentDefinition {
            runtime: Some("openclaw".to_string()),
            ..sample_persona()
        },
    );
    assert_eq!(
        record.agent_command_override, None,
        "stale claude-code-acp alias pin must be dropped when persona switches to openclaw"
    );
    assert_eq!(
        record.parallelism,
        crate::managed_agents::OPENCLAW_MAX_PARALLELISM,
        "parallelism must be capped after dropping stale Claude alias for OpenClaw"
    );
}

// ── Stale-pin keep: same harness, path/alias override ───────────────────────

/// Same-harness case: record has an explicit path override pointing at the same
/// harness as the new persona runtime. The pin must NOT be dropped — it is a
/// deliberate per-instance configuration (e.g. a specific goose binary path).
#[test]
fn apply_persona_snapshot_same_harness_path_pin_is_kept() {
    let mut record = sample_record();
    record.parallelism = 3;
    // Explicit path override for goose — same harness as the persona runtime.
    record.agent_command_override = Some("/usr/local/bin/goose".to_string());
    apply_persona_snapshot(
        &mut record,
        &AgentDefinition {
            runtime: Some("goose".to_string()),
            ..sample_persona()
        },
    );
    assert_eq!(
        record.agent_command_override.as_deref(),
        Some("/usr/local/bin/goose"),
        "same-harness path override must NOT be dropped"
    );
}
