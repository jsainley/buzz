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

    fn persona_def(
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

    // ── Policy table: harness_max_parallelism / effective_parallelism / normalize ─

    #[test]
    fn policy_table() {
        let cap = super::OPENCLAW_MAX_PARALLELISM;

        // harness_max_parallelism: openclaw variants → Some(cap); others → None.
        assert_eq!(super::harness_max_parallelism("openclaw"), Some(cap));
        assert_eq!(
            super::harness_max_parallelism("/usr/local/bin/openclaw"),
            Some(cap)
        );
        assert_eq!(super::harness_max_parallelism("openclaw.exe"), Some(cap));
        assert_eq!(
            super::harness_max_parallelism(r"C:\Tools\openclaw.exe"),
            Some(cap)
        );
        assert_eq!(super::harness_max_parallelism("goose"), None);
        assert_eq!(super::harness_max_parallelism("buzz-agent"), None);
        assert_eq!(super::harness_max_parallelism(""), None);

        // effective_parallelism: openclaw clamps above cap, honors at/below; goose passes through.
        assert_eq!(super::effective_parallelism("openclaw", cap + 5), cap);
        assert_eq!(super::effective_parallelism("openclaw", cap), cap);
        assert_eq!(super::effective_parallelism("openclaw", cap - 2), cap - 2);
        assert_eq!(super::effective_parallelism("goose", 99), 99);
        assert_eq!(super::effective_parallelism("buzz-agent", 32), 32);

        // normalize_instance_parallelism: writes effective parallelism back to the record.
        let mut r = record_with(None, cap + 5);
        super::normalize_instance_parallelism(&mut r, "openclaw");
        assert_eq!(r.parallelism, cap);
        let mut r2 = record_with(None, cap + 5);
        super::normalize_instance_parallelism(&mut r2, "goose");
        assert_eq!(r2.parallelism, cap + 5);
    }

    // ── command_for_runtime_id: three-tier lookup ─────────────────────────────

    #[test]
    fn command_for_runtime_id_covers_builtin_preset_and_unknown() {
        // goose: static builtin; openclaw: static preset (cold registry).
        assert_eq!(
            super::super::command_for_runtime_id("goose").as_deref(),
            Some("goose")
        );
        assert_eq!(
            super::super::command_for_runtime_id("openclaw").as_deref(),
            Some("openclaw"),
            "openclaw must resolve from the static preset list even with a cold registry"
        );
        assert_eq!(
            super::super::command_for_runtime_id("nonexistent-runtime-xyz"),
            None
        );
        assert_eq!(super::super::command_for_runtime_id(""), None);
    }

    // ── acp_agents_value: spawn-env seam ─────────────────────────────────────
    //
    // Drives the pure helper extracted from spawn_agent_child.
    // Deleting or changing it breaks this test AND the production spawn env.

    /// Legacy OpenClaw record (parallelism 10, above cap): BUZZ_ACP_AGENTS must be "5".
    #[test]
    fn acp_agents_value_openclaw_above_cap_is_capped() {
        assert_eq!(
            super::acp_agents_value("openclaw", 10),
            "5",
            "BUZZ_ACP_AGENTS for openclaw with parallelism 10 must be \"5\""
        );
        assert_eq!(super::acp_agents_value("goose", 10), "10");
    }

    // ── Override-direction: summary seam agreement ───────────────────────────
    //
    // Tests effective_parallelism and record_agent_command agreement for
    // both override directions. Removing either direction loses the seam
    // test for that cap/uncap path through the summary resolver.

    /// OpenClaw runtime + Goose override: summary resolves goose → uncapped (10).
    #[test]
    fn override_direction_openclaw_runtime_goose_override_is_uncapped() {
        let mut record = record_with(Some("openclaw"), 10);
        record.agent_command_override = Some("goose".to_string());
        let cmd = crate::managed_agents::record_agent_command(&record, &[]);
        assert_eq!(cmd, "goose");
        assert_eq!(super::effective_parallelism(&cmd, record.parallelism), 10);
    }

    /// Goose runtime + OpenClaw override: summary resolves openclaw → capped (5).
    #[test]
    fn override_direction_goose_runtime_openclaw_override_is_capped() {
        let mut record = record_with(Some("goose"), 10);
        record.agent_command_override = Some("openclaw".to_string());
        let cmd = crate::managed_agents::record_agent_command(&record, &[]);
        assert_eq!(cmd, "openclaw");
        assert_eq!(
            super::effective_parallelism(&cmd, record.parallelism),
            super::OPENCLAW_MAX_PARALLELISM
        );
    }

    // ── Summary: persona-inherited runtime (runtime=None) ────────────────────
    //
    // Covers the case where runtime was cleared by an "inherit from persona"
    // update: summary must resolve via the LIVE persona, not stale agent_command.

    /// Stale agent_command="openclaw", live persona=goose → summary resolves goose → uncapped.
    #[test]
    fn summary_persona_inherited_stale_openclaw_live_goose_is_uncapped() {
        let persona = persona_def("p-goose", Some("goose"));
        let mut record = record_with(None, 10);
        record.persona_id = Some("p-goose".to_string());
        record.agent_command = "openclaw".to_string();
        let cmd =
            crate::managed_agents::record_agent_command(&record, std::slice::from_ref(&persona));
        assert_eq!(
            cmd, "goose",
            "live persona must win over stale agent_command"
        );
        assert_eq!(super::effective_parallelism(&cmd, record.parallelism), 10);
    }

    /// Stale agent_command="goose", live persona=openclaw → summary resolves openclaw → capped.
    #[test]
    fn summary_persona_inherited_stale_goose_live_openclaw_is_capped() {
        let persona = persona_def("p-openclaw", Some("openclaw"));
        let mut record = record_with(None, 10);
        record.persona_id = Some("p-openclaw".to_string());
        record.agent_command = "goose".to_string();
        let cmd =
            crate::managed_agents::record_agent_command(&record, std::slice::from_ref(&persona));
        assert_eq!(
            cmd, "openclaw",
            "live persona must win over stale agent_command"
        );
        assert_eq!(
            super::effective_parallelism(&cmd, record.parallelism),
            super::OPENCLAW_MAX_PARALLELISM
        );
    }

    // ── effective_instance_parallelism: snapshot-import seam ─────────────────
    //
    // Drives the shared helper consumed by individual and team snapshot imports.

    #[test]
    fn snapshot_import_effective_instance_parallelism_table() {
        let cap = super::OPENCLAW_MAX_PARALLELISM;
        // openclaw above cap → capped; below cap → honored; unknown/None → pass-through.
        assert_eq!(
            super::effective_instance_parallelism(Some("openclaw"), cap + 5),
            cap
        );
        assert_eq!(
            super::effective_instance_parallelism(Some("openclaw"), cap - 2),
            cap - 2
        );
        assert_eq!(
            super::effective_instance_parallelism(Some("unknown-runtime-xyz"), 10),
            10
        );
        assert_eq!(super::effective_instance_parallelism(None, 10), 10);
    }

    // ── Snapshot export: requested-definition / effective-instance contract ───

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

    /// Export carries the REQUESTED definition parallelism (not clamped).
    #[test]
    fn snapshot_export_definition_parallelism_is_requested_not_clamped() {
        use crate::managed_agents::agent_snapshot::{build_snapshot, MemoryLevel};
        // definition_parallelism=Some(10) stored → exported as 10, not the instance cap 5.
        let snap = build_snapshot(
            &snapshot_record(Some("openclaw"), 5, Some(10)),
            MemoryLevel::None,
            vec![],
            None,
        );
        assert_eq!(snap.definition.parallelism, Some(10));
        // No definition_parallelism stored → falls back to effective instance.
        let snap2 = build_snapshot(
            &snapshot_record(Some("openclaw"), 5, None),
            MemoryLevel::None,
            vec![],
            None,
        );
        assert_eq!(snap2.definition.parallelism, Some(5));
    }
}
