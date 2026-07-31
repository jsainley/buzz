// ── Per-harness parallelism cap ───────────────────────────────────────────────
//
// Contract: requested definition, effective instance.
//
// * `AgentDefinition.parallelism` — the REQUESTED, portable value.  Never
//   clamped so it crosses device boundaries faithfully.  A device whose harness
//   cap is lower than this value mints instances at the cap but preserves the
//   definition field for export / relay round-trips.
//
// * `ManagedAgentRecord.parallelism` — the EFFECTIVE (clamped) value persisted
//   on this device.  Every instance-persistence boundary (create, update,
//   snapshot-import, inbound reconcile, snapshot-apply) calls
//   `normalize_instance_parallelism` so the stored value always equals the
//   runtime value — the field never lies.  Use the effective resolver defensively
//   at spawn, hash, and deploy for legacy records that pre-date this invariant.

/// Maximum parallelism for the OpenClaw harness.
///
/// Each buzz-acp worker spawned by the Desktop is a client of the single
/// shared OpenClaw Gateway daemon — running more than this number of workers
/// is both resource-expensive and architecturally wrong per the OpenClaw
/// design. Tyler's ruling: "try 5 and lower if needed."
pub const OPENCLAW_MAX_PARALLELISM: u32 = 5;

/// Return the maximum allowed `ManagedAgentRecord.parallelism` for the given
/// harness command, or `None` when the harness has no cap.
///
/// Keyed on [`super::discovery::normalize_command_identity`] so path prefixes, the `.exe`
/// suffix on Windows, and other cosmetic differences are ignored.
pub fn harness_max_parallelism(command: &str) -> Option<u32> {
    match super::discovery::normalize_command_identity(command).as_str() {
        "openclaw" => Some(OPENCLAW_MAX_PARALLELISM),
        _ => None,
    }
}

/// Return the effective parallelism for an instance record given the harness
/// command it will run under.
///
/// Precedence is always resolved *before* calling this function (explicit →
/// definition → `DEFAULT_AGENT_PARALLELISM`). This function only applies the
/// harness cap: `min(value, harness_max_parallelism(command))`.
///
/// For harnesses without a cap this is the identity function.
pub fn effective_parallelism(command: &str, value: u32) -> u32 {
    match harness_max_parallelism(command) {
        Some(cap) => value.min(cap),
        None => value,
    }
}

/// Apply the harness parallelism cap to an instance record in place.
///
/// This is the shared normalization helper consumed by every instance
/// persistence boundary. The `effective_command` argument is the resolved
/// harness command for this record — callers must supply the command they
/// actually execute so the policy follows the right identity.
pub(crate) fn normalize_instance_parallelism(
    record: &mut super::types::ManagedAgentRecord,
    effective_command: &str,
) {
    record.parallelism = effective_parallelism(effective_command, record.parallelism);
}

/// Return the value to emit as `BUZZ_ACP_AGENTS` for a spawn command.
///
/// This is a pure helper extracted from `spawn_agent_child` so both the
/// production path and tests can call it without spawning a process. The
/// result is `effective_parallelism(effective_command, record_parallelism)`
/// formatted as a decimal string ready for `command.env("BUZZ_ACP_AGENTS", …)`.
///
/// `effective_command` must be the already-resolved harness command (override →
/// runtime → persona runtime → default) — the same value `spawn_agent_child`
/// receives from the effective descriptor.
pub fn acp_agents_value(effective_command: &str, record_parallelism: u32) -> String {
    effective_parallelism(effective_command, record_parallelism).to_string()
}

/// Compute the effective instance parallelism for a mint or snapshot-import
/// record given a runtime id and the requested (raw) parallelism value.
///
/// This is the shared math called by:
/// - normal create mint (`agents.rs`) — after `resolve_mint_behavioral_defaults`;
/// - individual snapshot import (`personas/snapshot/import.rs`);
/// - team snapshot import (`team_snapshot.rs`).
///
/// Precedence (already applied by callers before this call):
///   explicit input → definition → [`super::DEFAULT_AGENT_PARALLELISM`].
///
/// This function only applies the harness cap on top of the already-resolved
/// requested value. The runtime id is resolved through the authoritative
/// three-tier lookup ([`super::command_for_runtime_id`]) so preset harnesses
/// (e.g. openclaw) are covered even with a cold registry.
pub fn effective_instance_parallelism(runtime_id: Option<&str>, requested: u32) -> u32 {
    let command = runtime_id
        .and_then(super::command_for_runtime_id)
        .unwrap_or_else(super::default_agent_command);
    effective_parallelism(&command, requested)
}

#[cfg(test)]
mod tests {
    use crate::managed_agents::types::ManagedAgentRecord;

    fn record_with(runtime: Option<&str>, parallelism: u32) -> ManagedAgentRecord {
        ManagedAgentRecord {
            pubkey: String::new(),
            name: "r".to_string(),
            persona_id: None,
            private_key_nsec: String::new(),
            auth_tag: None,
            relay_url: String::new(),
            avatar_url: None,
            acp_command: String::new(),
            agent_command: String::new(),
            agent_command_override: None,
            agent_args: vec![],
            mcp_command: String::new(),
            turn_timeout_seconds: 0,
            idle_timeout_seconds: None,
            max_turn_duration_seconds: None,
            parallelism,
            system_prompt: None,
            model: None,
            provider: None,
            persona_source_version: None,
            start_on_app_launch: false,
            auto_restart_on_config_change: true,
            runtime_pid: None,
            backend: Default::default(),
            backend_agent_id: None,
            provider_binary_path: None,
            team_id: None,
            persona_team_dir: None,
            persona_name_in_team: None,
            env_vars: std::collections::BTreeMap::new(),
            created_at: String::new(),
            updated_at: String::new(),
            last_started_at: None,
            last_stopped_at: None,
            last_exit_code: None,
            last_error: None,
            last_error_code: None,
            respond_to: Default::default(),
            respond_to_allowlist: vec![],
            display_name: None,
            slug: None,
            runtime: runtime.map(str::to_string),
            name_pool: Vec::new(),
            is_builtin: false,
            is_active: true,
            shared: false,
            source_team: None,
            source_team_persona_slug: None,
            catalog_source: None,
            definition_respond_to: None,
            definition_respond_to_allowlist: Vec::new(),
            definition_parallelism: None,
            relay_mesh: None,
        }
    }

    #[test]
    fn harness_max_parallelism_openclaw_returns_5() {
        assert_eq!(
            super::harness_max_parallelism("openclaw"),
            Some(super::OPENCLAW_MAX_PARALLELISM)
        );
    }

    #[test]
    fn harness_max_parallelism_openclaw_normalizes_path_prefix_and_exe() {
        assert_eq!(
            super::harness_max_parallelism("/usr/local/bin/openclaw"),
            Some(super::OPENCLAW_MAX_PARALLELISM)
        );
        assert_eq!(
            super::harness_max_parallelism("openclaw.exe"),
            Some(super::OPENCLAW_MAX_PARALLELISM)
        );
        assert_eq!(
            super::harness_max_parallelism(r"C:\Tools\openclaw.exe"),
            Some(super::OPENCLAW_MAX_PARALLELISM)
        );
    }

    #[test]
    fn harness_max_parallelism_unknown_harness_returns_none() {
        assert_eq!(super::harness_max_parallelism("goose"), None);
        assert_eq!(super::harness_max_parallelism("buzz-agent"), None);
        assert_eq!(super::harness_max_parallelism("custom-agent"), None);
        assert_eq!(super::harness_max_parallelism(""), None);
    }

    #[test]
    fn effective_parallelism_clamps_above_cap() {
        assert_eq!(
            super::effective_parallelism("openclaw", 10),
            super::OPENCLAW_MAX_PARALLELISM
        );
        assert_eq!(
            super::effective_parallelism("openclaw", 32),
            super::OPENCLAW_MAX_PARALLELISM
        );
    }

    #[test]
    fn effective_parallelism_honors_value_below_cap() {
        assert_eq!(super::effective_parallelism("openclaw", 5), 5);
        assert_eq!(super::effective_parallelism("openclaw", 1), 1);
        assert_eq!(super::effective_parallelism("openclaw", 3), 3);
    }

    #[test]
    fn effective_parallelism_identity_for_uncapped_harness() {
        assert_eq!(super::effective_parallelism("goose", 10), 10);
        assert_eq!(super::effective_parallelism("goose", 99), 99);
        assert_eq!(super::effective_parallelism("buzz-agent", 32), 32);
        assert_eq!(super::effective_parallelism("custom", 1), 1);
    }

    #[test]
    fn normalize_instance_parallelism_clamps_openclaw_record() {
        let mut record = record_with(None, 10);
        super::normalize_instance_parallelism(&mut record, "openclaw");
        assert_eq!(record.parallelism, super::OPENCLAW_MAX_PARALLELISM);
    }

    #[test]
    fn normalize_instance_parallelism_leaves_non_openclaw_unchanged() {
        let mut record = record_with(None, 10);
        super::normalize_instance_parallelism(&mut record, "goose");
        assert_eq!(record.parallelism, 10);
    }

    // ── command_for_runtime_id: authoritative three-tier lookup ──────────────

    #[test]
    fn command_for_runtime_id_resolves_openclaw_preset() {
        // OpenClaw is a preset harness (not in KNOWN_ACP_RUNTIMES) — must resolve
        // via the static PRESET_HARNESSES fallback even with a cold registry.
        assert_eq!(
            super::super::command_for_runtime_id("openclaw").as_deref(),
            Some("openclaw"),
            "command_for_runtime_id must resolve openclaw from the static preset list"
        );
    }

    #[test]
    fn command_for_runtime_id_resolves_builtin_goose() {
        // goose is a static builtin in KNOWN_ACP_RUNTIMES.
        assert_eq!(
            super::super::command_for_runtime_id("goose").as_deref(),
            Some("goose")
        );
    }

    #[test]
    fn command_for_runtime_id_unknown_returns_none() {
        assert_eq!(
            super::super::command_for_runtime_id("nonexistent-runtime-xyz"),
            None
        );
        assert_eq!(super::super::command_for_runtime_id(""), None);
    }

    // ── acp_agents_value: spawn-env helper ───────────────────────────────────
    //
    // These tests drive the pure helper extracted from spawn_agent_child.
    // Deleting or changing it breaks the test AND the production spawn env.

    /// Legacy OpenClaw record: spawn env must carry BUZZ_ACP_AGENTS=5.
    #[test]
    fn acp_agents_value_openclaw_legacy_record_is_5() {
        assert_eq!(
            super::acp_agents_value("openclaw", 10),
            "5",
            "BUZZ_ACP_AGENTS for openclaw with parallelism 10 must be \"5\""
        );
    }

    /// Non-OpenClaw harness: BUZZ_ACP_AGENTS passes through the raw value.
    #[test]
    fn acp_agents_value_goose_passes_through() {
        assert_eq!(super::acp_agents_value("goose", 10), "10");
        assert_eq!(super::acp_agents_value("goose", 99), "99");
    }

    /// OpenClaw at or below the cap: BUZZ_ACP_AGENTS equals the stored value.
    #[test]
    fn acp_agents_value_openclaw_at_cap_is_unchanged() {
        assert_eq!(super::acp_agents_value("openclaw", 5), "5");
        assert_eq!(super::acp_agents_value("openclaw", 3), "3");
    }

    // ── Two-direction override table ──────────────────────────────────────────
    //
    // Tests the full agreement chain for both override directions:
    //   wire projection (effective_parallelism) →
    //   summary (record_agent_command + effective_parallelism) →
    //   spawn-env helper (acp_agents_value)
    //
    // The summary path in runtime.rs:314 is:
    //   effective_parallelism(&descriptor.command, record.parallelism)
    //   where descriptor.command = record_agent_command(record, personas)
    //
    // For records with a materialized override or runtime (no persona context
    // needed), record_agent_command resolves correctly without a personas slice.
    // The persona-inherited direction is covered by the separate tests below.
    // Deleting effective_parallelism or changing its OpenClaw cap breaks all
    // assertions in these tests.

    /// OpenClaw runtime + Goose override: all three seams agree → 10 (uncapped).
    #[test]
    fn override_direction_openclaw_runtime_goose_override_is_uncapped_everywhere() {
        let mut record = record_with(Some("openclaw"), 10);
        record.agent_command_override = Some("goose".to_string());

        // Summary path: record_agent_command (no personas needed — override wins).
        let summary_cmd = crate::managed_agents::record_agent_command(&record, &[]);
        assert_eq!(
            summary_cmd, "goose",
            "summary descriptor must follow goose override"
        );

        // Wire projection and summary agree.
        assert_eq!(
            super::effective_parallelism(&summary_cmd, record.parallelism),
            10,
            "wire projection and summary must emit 10 (goose is uncapped)"
        );

        // Spawn-env helper agrees.
        assert_eq!(
            super::acp_agents_value(&summary_cmd, record.parallelism),
            "10",
            "BUZZ_ACP_AGENTS must be 10 (goose is uncapped)"
        );
    }

    /// Goose runtime + OpenClaw override: all three seams agree → 5 (capped).
    #[test]
    fn override_direction_goose_runtime_openclaw_override_is_capped_everywhere() {
        let mut record = record_with(Some("goose"), 10);
        record.agent_command_override = Some("openclaw".to_string());

        let summary_cmd = crate::managed_agents::record_agent_command(&record, &[]);
        assert_eq!(
            summary_cmd, "openclaw",
            "summary descriptor must follow openclaw override"
        );

        assert_eq!(
            super::effective_parallelism(&summary_cmd, record.parallelism),
            super::OPENCLAW_MAX_PARALLELISM,
            "wire projection and summary must emit 5 (openclaw override caps)"
        );
        assert_eq!(
            super::acp_agents_value(&summary_cmd, record.parallelism),
            "5",
            "BUZZ_ACP_AGENTS must be 5 (openclaw override caps)"
        );
    }

    // ── Summary parallelism: persona-inherited direction ──────────────────────
    //
    // The summary path uses record_agent_command(record, personas) which is
    // the persona-aware resolver (override → runtime → persona runtime → default).
    // For records where runtime=None was cleared by an "inherit from persona"
    // update, the summary must resolve via the live persona runtime, not the
    // stale agent_command. These tests cover the two inherit directions that
    // require a personas slice (no-personas records are covered by the override
    // table above).

    /// Build a minimal AgentDefinition for parallelism-policy tests.
    fn test_persona_def(
        id: &str,
        runtime: Option<&str>,
    ) -> crate::managed_agents::types::AgentDefinition {
        use crate::managed_agents::types::AgentDefinition;
        AgentDefinition {
            id: id.to_string(),
            display_name: String::new(),
            avatar_url: None,
            system_prompt: String::new(),
            runtime: runtime.map(str::to_string),
            model: None,
            provider: None,
            name_pool: vec![],
            is_builtin: false,
            is_active: true,
            shared: false,
            source_team: None,
            source_team_persona_slug: None,
            catalog_source: None,
            env_vars: std::collections::BTreeMap::new(),
            respond_to: None,
            respond_to_allowlist: vec![],
            parallelism: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    /// runtime=None (inherit), stale agent_command="openclaw", live persona=goose:
    /// summary must resolve goose (uncapped) → 10, not openclaw (capped) → 5.
    #[test]
    fn summary_persona_inherited_runtime_none_stale_openclaw_live_goose_is_uncapped() {
        let persona = test_persona_def("p-goose", Some("goose"));
        let mut record = record_with(None, 10); // runtime cleared by inherit
        record.persona_id = Some("p-goose".to_string());
        record.agent_command = "openclaw".to_string(); // stale
        record.agent_command_override = None;

        let summary_cmd =
            crate::managed_agents::record_agent_command(&record, std::slice::from_ref(&persona));
        assert_eq!(
            summary_cmd, "goose",
            "summary must resolve goose from live persona, not stale openclaw agent_command"
        );
        assert_eq!(
            super::effective_parallelism(&summary_cmd, record.parallelism),
            10,
            "summary parallelism must NOT be capped (goose is uncapped)"
        );
    }

    /// Inverse: runtime=None, stale agent_command="goose", live persona=openclaw:
    /// summary must resolve openclaw (capped) → 5, not goose (uncapped).
    #[test]
    fn summary_persona_inherited_runtime_none_stale_goose_live_openclaw_is_capped() {
        let persona = test_persona_def("p-openclaw", Some("openclaw"));
        let mut record = record_with(None, 10);
        record.persona_id = Some("p-openclaw".to_string());
        record.agent_command = "goose".to_string(); // stale
        record.agent_command_override = None;

        let summary_cmd =
            crate::managed_agents::record_agent_command(&record, std::slice::from_ref(&persona));
        assert_eq!(
            summary_cmd, "openclaw",
            "summary must resolve openclaw from live persona, not stale goose agent_command"
        );
        assert_eq!(
            super::effective_parallelism(&summary_cmd, record.parallelism),
            super::OPENCLAW_MAX_PARALLELISM,
            "summary parallelism must be capped to {} (live persona=openclaw)",
            super::OPENCLAW_MAX_PARALLELISM
        );
    }

    // ── Snapshot-import parallelism computation ───────────────────────────────
    //
    // These tests drive `effective_instance_parallelism`, the shared helper
    // consumed by both individual and team snapshot imports. Deleting or
    // breaking that helper breaks these tests.

    #[test]
    fn snapshot_import_openclaw_instance_is_capped_to_5() {
        assert_eq!(
            super::effective_instance_parallelism(Some("openclaw"), 10),
            super::OPENCLAW_MAX_PARALLELISM,
            "snapshot import: OpenClaw instance must be minted at effective cap, not requested 10"
        );
    }

    #[test]
    fn snapshot_import_unknown_runtime_falls_back_to_default_and_is_uncapped() {
        assert_eq!(
            super::effective_instance_parallelism(Some("unknown-runtime-xyz"), 10),
            10,
            "unknown runtime falls back to default command, no cap applied"
        );
        assert_eq!(
            super::effective_instance_parallelism(None, 10),
            10,
            "None runtime falls back to default command, no cap applied"
        );
    }

    #[test]
    fn snapshot_import_openclaw_below_cap_is_honored() {
        assert_eq!(
            super::effective_instance_parallelism(Some("openclaw"), 3),
            3,
            "snapshot import: explicit value below cap must be honored"
        );
    }

    // ── Snapshot export: requested-definition / effective-instance contract ───

    /// Build a minimal record for snapshot export tests that does not require
    /// the full `minimal_record()` fixture from `agent_snapshot.rs`.
    fn snapshot_record(
        runtime: Option<&str>,
        parallelism: u32,
        definition_parallelism: Option<u32>,
    ) -> ManagedAgentRecord {
        use crate::managed_agents::types::{BackendKind, RespondTo};
        use std::collections::BTreeMap;
        let mut r = record_with(runtime, parallelism);
        r.name = "snap-test".to_string();
        r.definition_parallelism = definition_parallelism;
        r.backend = BackendKind::Local;
        r.respond_to = RespondTo::OwnerOnly;
        r.env_vars = BTreeMap::new();
        r
    }

    /// Snapshot export carries the REQUESTED definition parallelism (not clamped).
    #[test]
    fn snapshot_export_openclaw_definition_keeps_requested_parallelism() {
        use crate::managed_agents::agent_snapshot::{build_snapshot, MemoryLevel};
        let record = snapshot_record(Some("openclaw"), 5, Some(10));
        let snapshot = build_snapshot(&record, MemoryLevel::None, vec![], None);
        assert_eq!(
            snapshot.definition.parallelism,
            Some(10),
            "snapshot export must carry the REQUESTED definition parallelism (10), not 5"
        );
    }

    /// When no definition_parallelism is stored, export falls back to the effective instance.
    #[test]
    fn snapshot_export_openclaw_no_definition_parallelism_exports_effective_instance() {
        use crate::managed_agents::agent_snapshot::{build_snapshot, MemoryLevel};
        let record = snapshot_record(Some("openclaw"), 5, None);
        let snapshot = build_snapshot(&record, MemoryLevel::None, vec![], None);
        assert_eq!(
            snapshot.definition.parallelism,
            Some(5),
            "snapshot export falls back to effective instance when no definition_parallelism stored"
        );
    }
}
