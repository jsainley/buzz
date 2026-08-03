use super::*;
use crate::managed_agents::AgentDefinition;

fn bare_agent_record(
    persona_id: Option<&str>,
    model: Option<&str>,
    provider: Option<&str>,
) -> ManagedAgentRecord {
    use crate::managed_agents::{BackendKind, RespondTo};
    use std::collections::BTreeMap;
    ManagedAgentRecord {
        pubkey: "agent".to_string(),
        name: "Agent".to_string(),
        persona_id: persona_id.map(str::to_string),
        private_key_nsec: "".to_string(),
        auth_tag: None,
        relay_url: "ws://localhost:3000".to_string(),
        avatar_url: None,
        acp_command: "buzz-acp".to_string(),
        agent_command: "goose".to_string(),
        agent_command_override: None,
        agent_args: vec![],
        mcp_command: "".to_string(),
        turn_timeout_seconds: 300,
        idle_timeout_seconds: None,
        max_turn_duration_seconds: None,
        parallelism: 1,
        system_prompt: None,
        model: model.map(str::to_string),
        provider: provider.map(str::to_string),
        persona_source_version: None,
        env_vars: BTreeMap::new(),
        start_on_app_launch: false,
        runtime_pid: None,
        backend: BackendKind::Local,
        backend_agent_id: None,
        provider_binary_path: None,
        team_id: None,
        persona_team_dir: None,
        persona_name_in_team: None,
        created_at: "".to_string(),
        updated_at: "".to_string(),
        last_started_at: None,
        last_stopped_at: None,
        last_exit_code: None,
        last_error: None,
        last_error_code: None,
        respond_to: RespondTo::OwnerOnly,
        respond_to_allowlist: vec![],
        display_name: None,
        slug: None,
        runtime: None,
        name_pool: vec![],
        is_builtin: false,
        is_active: true,
        shared: false,
        source_team: None,
        source_team_persona_slug: None,
        catalog_source: None,
        relay_mesh: None,
        auto_restart_on_config_change: false,
        definition_respond_to: None,
        definition_respond_to_allowlist: vec![],
        definition_parallelism: None,
    }
}
fn persona_record(id: &str, model: Option<&str>, provider: Option<&str>) -> AgentDefinition {
    use std::collections::BTreeMap;
    AgentDefinition {
        id: id.to_string(),
        display_name: "Test Persona".to_string(),
        avatar_url: None,
        system_prompt: "".to_string(),
        runtime: None,
        model: model.map(str::to_string),
        provider: provider.map(str::to_string),
        name_pool: vec![],
        is_builtin: false,
        is_active: true,
        shared: false,
        source_team: None,
        source_team_persona_slug: None,
        catalog_source: None,
        env_vars: BTreeMap::new(),
        respond_to: None,
        respond_to_allowlist: vec![],
        parallelism: None,
        created_at: "".to_string(),
        updated_at: "".to_string(),
    }
}

/// Auto-archive uses the same NIP-IA wire builder as the explicit GUI action,
/// attaches owner consent, and marks a deliberate delete as `retired`.
#[test]
fn build_agent_archive_request_attaches_owner_auth_and_retired_reason() {
    use nostr::JsonUtil;

    let owner = nostr::Keys::generate();
    let agent = nostr::Keys::generate();
    let event = build_agent_archive_request(&owner, &agent.public_key().to_hex())
        .expect("build archive request");
    let json: serde_json::Value = serde_json::from_str(&event.as_json()).unwrap();
    let tags = json["tags"].as_array().unwrap();

    assert_eq!(event.kind.as_u16(), 9035);
    assert_eq!(event.pubkey, owner.public_key());
    assert!(event.verify_id());
    assert!(event.verify_signature());
    assert!(tags.iter().any(|tag| {
        tag.as_array().is_some_and(|parts| {
            parts.first().and_then(serde_json::Value::as_str) == Some("p")
                && parts.get(1).and_then(serde_json::Value::as_str)
                    == Some(agent.public_key().to_hex().as_str())
        })
    }));
    assert!(tags.iter().any(|tag| {
        tag.as_array().is_some_and(|parts| {
            parts.first().and_then(serde_json::Value::as_str) == Some("reason")
                && parts.get(1).and_then(serde_json::Value::as_str) == Some("retired")
        })
    }));
    assert!(tags.iter().any(|tag| {
        tag.as_array().is_some_and(|parts| {
            parts.first().and_then(serde_json::Value::as_str) == Some("auth")
                && parts.get(1).and_then(serde_json::Value::as_str)
                    == Some(owner.public_key().to_hex().as_str())
                && parts.len() == 4
        })
    }));
}

/// Deploy resolver uses definition model/provider, ignoring stale record.
#[test]
fn deploy_resolver_uses_definition_over_stale_record() {
    let record = bare_agent_record(Some("p1"), Some("old-model"), Some("old-prov"));
    let personas = vec![persona_record("p1", Some("new-model"), Some("new-prov"))];
    let global = crate::managed_agents::GlobalAgentConfig::default();

    let (model, provider) = resolve_deploy_model_provider(&record, &personas, &global);

    assert_eq!(
        model.as_deref(),
        Some("new-model"),
        "deploy must use definition model, not stale record snapshot"
    );
    assert_eq!(
        provider.as_deref(),
        Some("new-prov"),
        "deploy must use definition provider, not stale record snapshot"
    );
}

/// When a linked definition has blank model/provider (inherit), the deploy
/// resolver must fall through to global — stale record bytes are inert.
#[test]
fn deploy_resolver_inherits_global_when_definition_blank() {
    let record = bare_agent_record(Some("p1"), Some("stale-model"), Some("stale-prov"));
    let personas = vec![persona_record("p1", None, None)];
    let global = crate::managed_agents::GlobalAgentConfig {
        model: Some("global-model".to_string()),
        provider: Some("global-prov".to_string()),
        ..Default::default()
    };

    let (model, provider) = resolve_deploy_model_provider(&record, &personas, &global);

    assert_eq!(
        model.as_deref(),
        Some("global-model"),
        "definition blank → global; stale record ignored"
    );
    assert_eq!(
        provider.as_deref(),
        Some("global-prov"),
        "definition blank → global; stale record ignored"
    );
}

/// Deploy resolver falls back to global when both definition and record have none.
#[test]
fn deploy_resolver_falls_back_to_global_when_definition_and_record_have_none() {
    let record = bare_agent_record(Some("p1"), None, None);
    let personas = vec![persona_record("p1", None, None)];
    let global = crate::managed_agents::GlobalAgentConfig {
        model: Some("global-model".to_string()),
        provider: Some("global-prov".to_string()),
        ..Default::default()
    };

    let (model, provider) = resolve_deploy_model_provider(&record, &personas, &global);

    assert_eq!(model.as_deref(), Some("global-model"));
    assert_eq!(provider.as_deref(), Some("global-prov"));
}

/// Orphan: linked record with missing definition → the pure model/provider
/// pair resolver returns `(None, None)`. This is NOT the deploy refusal
/// boundary — `build_deploy_payload` refuses an orphan outright via
/// `.require_resolved()?` before this pair is ever computed. This test pins
/// the resolver's own orphan behavior, which readiness/hash also depend on.
#[test]
fn deploy_resolver_returns_none_for_orphaned_instance() {
    let record = bare_agent_record(Some("missing-def"), Some("stale-model"), Some("stale-prov"));
    let personas: Vec<AgentDefinition> = vec![];
    let global = crate::managed_agents::GlobalAgentConfig {
        model: Some("global-model".to_string()),
        provider: Some("global-prov".to_string()),
        ..Default::default()
    };

    let (model, provider) = resolve_deploy_model_provider(&record, &personas, &global);

    assert!(
        model.is_none(),
        "orphaned instance must not resolve to any model"
    );
    assert!(
        provider.is_none(),
        "orphaned instance must not resolve to any provider"
    );
}

#[test]
fn normalize_relay_mesh_rejects_empty_model_ref() {
    let config = RelayMeshConfig {
        model_ref: "  \t ".to_string(),
    };

    assert_eq!(
        normalize_relay_mesh(Some(&config), &BackendKind::Local).unwrap_err(),
        "Buzz shared compute model is required"
    );
}

#[test]
fn normalize_relay_mesh_rejects_non_local_backend() {
    let config = RelayMeshConfig {
        model_ref: "Qwen3".to_string(),
    };
    let backend = BackendKind::Provider {
        id: "blox".to_string(),
        config: serde_json::json!({}),
    };

    assert_eq!(
        normalize_relay_mesh(Some(&config), &backend).unwrap_err(),
        "Buzz shared compute agents must use the local backend"
    );
}

#[test]
fn normalize_relay_mesh_trims_and_preserves_valid_config() {
    let config = RelayMeshConfig {
        model_ref: "  Qwen3  ".to_string(),
    };

    assert_eq!(
        normalize_relay_mesh(Some(&config), &BackendKind::Local).unwrap(),
        Some(RelayMeshConfig {
            model_ref: "Qwen3".to_string(),
        })
    );
}

#[test]
fn deploy_refuses_resolved_relay_mesh_provider_with_padding() {
    let record = bare_agent_record(Some("p1"), None, None);
    let personas = vec![persona_record("p1", None, Some("  relay-mesh  "))];
    let global = crate::managed_agents::GlobalAgentConfig::default();

    let (_, provider) = resolve_deploy_model_provider(&record, &personas, &global);
    let error = ensure_remote_provider_supported(provider.as_deref())
        .expect_err("resolved shared-compute provider must not deploy remotely");

    assert!(error.contains("cannot be deployed remotely"), "{error}");
}

#[test]
fn created_avatar_prefers_explicit_input() {
    let resolved = resolve_created_avatar_url(
        Some(" https://x/input.png "),
        Some("https://x/persona.png".to_string()),
        "goose",
    );

    assert_eq!(resolved.as_deref(), Some("https://x/input.png"));
}

#[test]
fn created_avatar_uses_persona_before_command_fallback() {
    let resolved =
        resolve_created_avatar_url(None, Some(" https://x/persona.png ".to_string()), "goose");

    assert_eq!(resolved.as_deref(), Some("https://x/persona.png"));
}

#[test]
fn created_avatar_uses_command_fallback_without_input_or_persona() {
    use crate::managed_agents::managed_agent_avatar_url;

    let resolved = resolve_created_avatar_url(None, None, "goose");

    assert_eq!(resolved, managed_agent_avatar_url("goose"));
}

fn profile(name: Option<&str>, picture: Option<&str>) -> crate::relay::AgentProfileInfo {
    crate::relay::AgentProfileInfo {
        display_name: name.map(str::to_string),
        picture: picture.map(str::to_string),
    }
}

#[test]
fn profile_needs_sync_when_missing() {
    assert!(profile_needs_sync(None, "Duncan", Some("https://x/a.png")));
}

#[test]
fn profile_needs_sync_when_missing_even_without_expected_avatar() {
    assert!(profile_needs_sync(None, "Duncan", None));
}

#[test]
fn profile_needs_sync_when_name_diverges() {
    let existing = profile(Some("Stilgar"), Some("https://x/a.png"));
    assert!(profile_needs_sync(
        Some(&existing),
        "Duncan",
        Some("https://x/a.png")
    ));
}

#[test]
fn profile_needs_sync_when_picture_diverges() {
    let existing = profile(Some("Duncan"), Some("https://x/old.png"));
    assert!(profile_needs_sync(
        Some(&existing),
        "Duncan",
        Some("https://x/new.png")
    ));
}

#[test]
fn profile_in_sync_when_name_and_picture_match() {
    let existing = profile(Some("Duncan"), Some("https://x/a.png"));
    assert!(!profile_needs_sync(
        Some(&existing),
        "Duncan",
        Some("https://x/a.png")
    ));
}

#[test]
fn profile_in_sync_when_both_avatars_absent() {
    let existing = profile(Some("Duncan"), None);
    assert!(!profile_needs_sync(Some(&existing), "Duncan", None));
}

#[test]
fn profile_needs_sync_when_existing_name_is_none() {
    let existing = profile(None, Some("https://x/a.png"));
    assert!(profile_needs_sync(
        Some(&existing),
        "Duncan",
        Some("https://x/a.png"),
    ));
}

#[test]
fn profile_needs_sync_when_expected_avatar_absent_but_published() {
    let existing = profile(Some("Duncan"), Some("https://x/a.png"));
    assert!(profile_needs_sync(Some(&existing), "Duncan", None));
}

#[test]
fn legacy_avatar_prefers_persona_over_corrupted_relay_picture() {
    // The regression: the relay picture was overwritten with the command
    // default. The persona avatar must win so the correct avatar is restored.
    let resolved = resolve_legacy_avatar(
        Some("https://x/persona.png".to_string()),
        Some("https://x/default-icon.png".to_string()),
        "goose",
    );

    assert_eq!(resolved, "https://x/persona.png");
}

#[test]
fn legacy_avatar_falls_back_to_relay_picture_without_persona() {
    let resolved = resolve_legacy_avatar(None, Some("https://x/relay.png".to_string()), "goose");

    assert_eq!(resolved, "https://x/relay.png");
}

#[test]
fn legacy_avatar_falls_back_to_command_icon_when_no_persona_or_relay() {
    use crate::managed_agents::managed_agent_avatar_url;

    let resolved = resolve_legacy_avatar(None, None, "goose");

    assert_eq!(resolved, managed_agent_avatar_url("goose").unwrap());
}

#[test]
fn legacy_avatar_empty_when_nothing_resolves() {
    let resolved = resolve_legacy_avatar(None, None, "totally-unknown-command");

    assert!(resolved.is_empty());
}

// ── Provider deploy payload completeness ─────────────────────────────────────

/// The shared provider fixture is the contract arbiter: it must be the exact
/// richest deploy request produced by the real desktop serializers.
#[test]
fn deploy_payload_matches_the_shared_full_launch_fixture() {
    let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
        "../../crates/buzz-backend-kubernetes/tests/fixtures/provider-wire/deploy-full-launch.request.json",
    );
    let fixture: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&fixture_path)
            .unwrap_or_else(|error| panic!("read {}: {error}", fixture_path.display())),
    )
    .expect("parse shared provider fixture");
    let record: ManagedAgentRecord = serde_json::from_value(serde_json::json!({
        "pubkey": "abcd1234",
        "name": "worker",
        "private_key_nsec": "nsec1vl029mgpspedva04g90vltkh6fvh240zqtv9k0t9af8935ke9laqsnlfe5",
        "relay_url": "wss://localhost:3000",
        "auth_tag": "tag-1",
        "acp_command": "buzz-acp",
        "agent_command": "goose",
        "runtime": "goose",
        "model": "gpt-5",
        "provider": "openai",
        "env_vars": {"USER_KEY": "user-value"},
        "agent_args": [],
        "mcp_command": "",
        "turn_timeout_seconds": 300,
        "system_prompt": null,
        "idle_timeout_seconds": null,
        "max_turn_duration_seconds": null,
        "parallelism": 10,
        "respond_to": "allowlist",
        "respond_to_allowlist": ["aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"],
        "created_at": "2026-01-01T00:00:00Z",
        "updated_at": "2026-01-01T00:00:00Z"
    }))
    .expect("fixture source record");
    let descriptor = crate::managed_agents::resolve_effective_harness_descriptor(
        &record,
        &[],
        &crate::managed_agents::GlobalAgentConfig::default(),
    )
    .expect("resolve fixture source record descriptor");
    let launch = super::deploy::build_launch_block(
        &record,
        &descriptor,
        &[],
        None,
        Some("gpt-5"),
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    );
    let agent = deploy_payload_json(
        &record,
        "wss://relay.example".into(),
        Some("gpt-5".into()),
        Some("openai".into()),
        None,
        std::collections::BTreeMap::from([("USER_KEY".into(), "user-value".into())]),
        launch,
    );

    assert_eq!(
        agent, fixture["agent"],
        "desktop payload drifted from the shared provider fixture"
    );
}

#[test]
fn tauri_platform_configs_bundle_kubernetes_only_on_supported_hosts() {
    use tauri_utils::{config::parse::read_from, platform::Target};

    let config_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    for (target, expected) in [
        (Target::MacOS, true),
        (Target::Linux, true),
        (Target::Windows, false),
    ] {
        let (config, paths) = read_from(target, config_root).expect("read Tauri config");
        let external_bins = config["bundle"]["externalBin"]
            .as_array()
            .expect("bundle.externalBin array");
        let has_kubernetes = external_bins
            .iter()
            .any(|value| value == "binaries/buzz-backend-kubernetes");
        assert_eq!(
            has_kubernetes, expected,
            "unexpected Kubernetes externalBin for {target}; merged {paths:?}"
        );
    }
}

/// An OpenClaw-backed record: the provider deploy payload must carry the capped
/// parallelism (5), not the over-cap value (10).
#[test]
fn deploy_payload_openclaw_parallelism_is_capped() {
    let record: ManagedAgentRecord = serde_json::from_str(
        r#"{
            "pubkey": "abcd1234",
            "name": "openclaw-agent",
            "private_key_nsec": "nsec1fake",
            "relay_url": "wss://localhost:3000",
            "acp_command": "buzz-acp",
            "agent_command": "openclaw",
            "agent_args": [],
            "mcp_command": "",
            "turn_timeout_seconds": 320,
            "system_prompt": null,
            "parallelism": 10,
            "respond_to": "owner-only",
            "respond_to_allowlist": [],
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "last_started_at": null,
            "last_stopped_at": null,
            "last_exit_code": null,
            "last_error": null
        }"#,
    )
    .expect("openclaw sample record");

    let payload = deploy_payload_json(
        &record,
        "wss://relay.example".to_string(),
        None,
        None,
        None,
        std::collections::BTreeMap::new(),
    );

    assert_eq!(
        payload["parallelism"],
        crate::managed_agents::OPENCLAW_MAX_PARALLELISM,
        "deploy payload for OpenClaw must carry the capped value ({}), not the stored 10",
        crate::managed_agents::OPENCLAW_MAX_PARALLELISM
    );
}

/// Payload-identity regression: the deploy payload keys the cap from
/// `record.agent_command` (the serialized field), not the live descriptor.
/// When the linked persona's runtime differs from the pinned record command,
/// the payload follows the pinned `record.agent_command`.
///
/// Fixture: record.agent_command="openclaw" (pinned at create time),
/// record.runtime="goose" (what local spawn resolves via record.runtime),
/// record.parallelism=10 (over-cap). The local descriptor resolves "goose"
/// (uncapped), but the payload must serialize "openclaw" and cap to 5.
#[test]
fn deploy_payload_uses_pinned_agent_command_not_live_descriptor() {
    let mut record: ManagedAgentRecord = serde_json::from_str(
        r#"{
            "pubkey": "abcd5678",
            "name": "pinned-openclaw-agent",
            "private_key_nsec": "nsec1fake",
            "relay_url": "wss://localhost:3000",
            "acp_command": "buzz-acp",
            "agent_command": "openclaw",
            "agent_args": [],
            "mcp_command": "",
            "turn_timeout_seconds": 320,
            "system_prompt": null,
            "parallelism": 10,
            "respond_to": "owner-only",
            "respond_to_allowlist": [],
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
            "last_started_at": null,
            "last_stopped_at": null,
            "last_exit_code": null,
            "last_error": null
        }"#,
    )
    .expect("pinned openclaw record");
    // Simulate a record whose runtime field points to a DIFFERENT harness than
    // agent_command (e.g. after an inherit update that set runtime from a
    // goose persona). The local descriptor would resolve "goose" here.
    record.runtime = Some("goose".to_string());
    record.persona_id = Some("persona-goose".to_string());

    // Local descriptor: record_agent_command prefers record.runtime → "goose".
    let local_cmd = crate::managed_agents::record_agent_command(
        &record,
        &[persona_record("persona-goose", None, None)],
    );
    assert_eq!(
        local_cmd, "goose",
        "local descriptor must resolve goose (from record.runtime)"
    );
    assert_eq!(
        crate::managed_agents::effective_parallelism(&local_cmd, record.parallelism),
        10,
        "goose is uncapped — local effective parallelism is 10"
    );

    // Deploy payload: keys cap on record.agent_command ("openclaw") — pinned
    // at create time and serialized as the provider's execution target.
    let payload = deploy_payload_json(
        &record,
        "wss://relay.example".to_string(),
        None,
        None,
        None,
        std::collections::BTreeMap::new(),
    );
    assert_eq!(
        payload["agent_command"], "openclaw",
        "payload must serialize the pinned record.agent_command, not the live descriptor"
    );
    assert_eq!(
        payload["parallelism"],
        crate::managed_agents::OPENCLAW_MAX_PARALLELISM,
        "payload must cap based on the pinned agent_command (openclaw), not the live goose descriptor"
    );
}

// ── Create-mint: per-harness parallelism cap at the create boundary ───────────
//
// These tests drive the production create path in agents.rs, which resolves
// agent_command via effective_agent_command (using command_for_runtime_id) and
// then calls effective_parallelism. Removing that call from create_managed_agent
// would still let these pass, but any regression in effective_parallelism itself
// would fail them.

/// Default parallelism for an OpenClaw mint: DEFAULT_AGENT_PARALLELISM → capped at 5.
#[test]
fn create_mint_openclaw_default_parallelism_is_capped() {
    assert_eq!(
        crate::managed_agents::effective_parallelism("openclaw", DEFAULT_AGENT_PARALLELISM),
        crate::managed_agents::OPENCLAW_MAX_PARALLELISM,
        "create mint with default parallelism on OpenClaw must cap at {}",
        crate::managed_agents::OPENCLAW_MAX_PARALLELISM
    );
}

/// Definition parallelism 8 wins over default but is still capped at 5 for OpenClaw.
/// Drives resolve_mint_behavioral_defaults → effective_parallelism — a two-step seam.
#[test]
fn create_mint_openclaw_definition_parallelism_above_cap_is_clamped() {
    let mut definition = persona_record("def-openclaw", None, None);
    definition.parallelism = Some(8);
    definition.runtime = Some("openclaw".to_string());
    let minted = crate::managed_agents::resolve_mint_behavioral_defaults(
        None,
        vec![],
        None,
        Some(&definition),
    )
    .expect("resolve_mint_behavioral_defaults must succeed");
    let requested = minted.parallelism.unwrap_or(DEFAULT_AGENT_PARALLELISM);
    assert_eq!(
        crate::managed_agents::effective_parallelism("openclaw", requested),
        crate::managed_agents::OPENCLAW_MAX_PARALLELISM,
        "definition parallelism 8 above cap must be clamped to {} at mint",
        crate::managed_agents::OPENCLAW_MAX_PARALLELISM
    );
}

// ── Update + harness switch: normalize at the update boundary ─────────────────
//
// Drives the exact production sequence in update_managed_agent (agent_models.rs):
//   apply_agent_command_update → record_agent_command → normalize_instance_parallelism.
// Removing normalize_instance_parallelism from agent_models.rs skips the cap
// in production; this test would still pass since it calls the helper directly,
// so it is paired with the deploy-payload test above to cover both boundaries.

/// Harness switch goose → openclaw: normalize_instance_parallelism caps stored value.
#[test]
fn update_harness_switch_goose_to_openclaw_clamps_parallelism() {
    use crate::managed_agents::{normalize_instance_parallelism, record_agent_command};

    let mut record = bare_agent_record(None, None, None);
    record.agent_command = "goose".to_string();
    record.parallelism = 8;

    crate::managed_agents::apply_agent_command_update(&mut record, &[], "openclaw", true);
    let cmd = record_agent_command(&record, &[]);
    normalize_instance_parallelism(&mut record, &cmd);

    assert_eq!(
        record.parallelism,
        crate::managed_agents::OPENCLAW_MAX_PARALLELISM,
        "after switching to openclaw, parallelism 8 must be clamped to {}",
        crate::managed_agents::OPENCLAW_MAX_PARALLELISM
    );
}
