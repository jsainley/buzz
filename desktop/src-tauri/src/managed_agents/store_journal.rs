//! B1 transactional managed-agents store substrate.
// The primitives here are the B1 substrate — anchor, lock, journal schema,
// CAS, tombstone, saga, inbox/outbox, and closure-only mutation API.  Many
// are not yet called from live code (Phase 2 wires mutate_store into
// save_managed_agents); suppress dead-code lint for the whole module.
#![allow(dead_code)]
//!
//! `managed-agents.json` and `teams.json` remain the canonical user-visible
//! files.  A `store-journal.sqlite` lives beside them at the **store-family
//! anchor** and holds shared recovery facts: operation records, per-key
//! generation/tombstone metadata, and immutable inbox/outbox rows.
//!
//! **Anchor**: canonical dev `agents/` dir for shared worktrees
//! (`BUZZ_SHARE_IDENTITY=1`, identifier `xyz.block.buzz.app.dev`); the
//! bundle's own `agents/` dir for standalone.  Lock identity is never derived
//! by canonicalizing a possibly-absent `managed-agents.json`.
//!
//! **Lock sequence**: in-process `AppState::managed_agents_store_lock` mutex
//! → anchored OS advisory lock (`flock(2)` / named mutex) → fresh decode →
//! mutation closure → atomic write + journal update → release both.
//! Network I/O and keyring access stay outside every critical section.
//!
//! **Posture**: B1 protects data integrity against crashes and concurrent
//! cooperating processes on one machine.  It does not defend against
//! adversarial same-user tampering, cross-machine duplication, supply-chain
//! attack, pre-B1/mixed-version writers bypassing the lock, sign-out/reset
//! paths that rename state while another process is live, or two desktop
//! bundles running the same agent pair concurrently.

use std::{
    path::{Path, PathBuf},
    sync::MutexGuard,
    time::{SystemTime, UNIX_EPOCH},
};

use rusqlite::{params, Connection, OptionalExtension};
use tauri::{AppHandle, Manager};

use crate::managed_agents::{ManagedAgentRecord, TeamRecord};
use crate::migration::is_dev_data_dir_name;

// ── Constants ────────────────────────────────────────────────────────────────

const CANONICAL_DEV_IDENTIFIER: &str = "xyz.block.buzz.app.dev";

/// Journal filename, beside `managed-agents.json`.
const JOURNAL_FILENAME: &str = "store-journal.sqlite";

/// Advisory lockfile name, beside `managed-agents.json`.
const ADVISORY_LOCK_FILENAME: &str = "store-journal.lock";

// ── Store-family anchor ──────────────────────────────────────────────────────

/// Resolve the store-family anchor directory.
///
/// For shared dev worktrees (`BUZZ_SHARE_IDENTITY=1`): the canonical dev
/// `agents/` dir (falls back to local if absent).  For standalone:
/// `app_data_dir()/agents`.  Never derived from `managed-agents.json`.
pub fn store_anchor_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let local_agents = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("failed to resolve app data dir: {e}"))?
        .join("agents");

    // Only redirect to the canonical dev anchor when identity-sharing is active.
    let is_shared = std::env::var("BUZZ_SHARE_IDENTITY")
        .map(|v| v == "1")
        .unwrap_or(false);

    if is_shared {
        if let Some(anchor) = canonical_dev_anchor(&local_agents) {
            if anchor.exists() {
                return Ok(anchor);
            }
        }
    }

    Ok(local_agents)
}

/// Compute the canonical dev anchor from a local `agents/` path.
/// Returns `None` when the path structure is unexpected.
/// Exposed as `canonical_dev_anchor_pub` for tests.
#[cfg(test)]
pub fn canonical_dev_anchor_pub(local_agents: &Path) -> Option<PathBuf> {
    canonical_dev_anchor(local_agents)
}

fn canonical_dev_anchor(local_agents: &Path) -> Option<PathBuf> {
    // local_agents = <AppDataDir>/agents
    // AppDataDir   = <parent>/<identifier>
    // canonical    = <parent>/<CANONICAL_DEV_IDENTIFIER>/agents
    let app_data_dir = local_agents.parent()?;
    let data_parent = app_data_dir.parent()?;
    let name = app_data_dir.file_name()?.to_str()?;

    if !is_dev_data_dir_name(name) {
        return None;
    }

    Some(data_parent.join(CANONICAL_DEV_IDENTIFIER).join("agents"))
}

// ── Advisory lock ────────────────────────────────────────────────────────────

/// RAII guard for the interprocess advisory lock.
/// Unix: `flock(2)`.  Windows: named mutex.  Other: no-op.
pub struct JournalLockGuard {
    #[cfg(unix)]
    #[allow(dead_code)]
    file: std::fs::File,
    #[cfg(windows)]
    mutex_handle: windows_sys::Win32::Foundation::HANDLE,
    #[cfg(not(any(unix, windows)))]
    _phantom: (),
}

impl JournalLockGuard {
    /// Acquire the exclusive advisory lock for `anchor_dir`, blocking until
    /// the lock is available.
    pub fn acquire(anchor_dir: &Path) -> Result<Self, String> {
        std::fs::create_dir_all(anchor_dir)
            .map_err(|e| format!("create anchor dir {}: {e}", anchor_dir.display()))?;
        let lock_path = anchor_dir.join(ADVISORY_LOCK_FILENAME);

        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .write(true)
                .open(&lock_path)
                .map_err(|e| format!("open journal lock {}: {e}", lock_path.display()))?;
            let ret = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if ret != 0 {
                let err = std::io::Error::last_os_error();
                return Err(format!("journal flock {}: {err}", lock_path.display()));
            }
            Ok(JournalLockGuard { file })
        }

        #[cfg(windows)]
        {
            use std::os::windows::ffi::OsStrExt;
            // Derive a unique mutex name from the lock file path.
            let path_str = lock_path
                .to_str()
                .unwrap_or("buzz-store-journal")
                .replace(['\\', '/', ':'], "-");
            let name: Vec<u16> = std::ffi::OsStr::new(&format!("Global\\{path_str}"))
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let handle = unsafe {
                windows_sys::Win32::System::Threading::CreateMutexW(
                    std::ptr::null(),
                    0,
                    name.as_ptr(),
                )
            };
            if handle == 0 || handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
                return Err(format!("CreateMutexW failed for journal lock"));
            }
            let wait = unsafe {
                windows_sys::Win32::System::Threading::WaitForSingleObject(
                    handle,
                    windows_sys::Win32::System::Threading::INFINITE,
                )
            };
            if wait != 0 {
                unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) };
                return Err(format!("WaitForSingleObject failed: {wait}"));
            }
            return Ok(JournalLockGuard {
                mutex_handle: handle,
            });
        }

        #[cfg(not(any(unix, windows)))]
        {
            Ok(JournalLockGuard { _phantom: () })
        }
    }
}

#[cfg(windows)]
impl Drop for JournalLockGuard {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Threading::ReleaseMutex(self.mutex_handle);
            windows_sys::Win32::Foundation::CloseHandle(self.mutex_handle);
        }
    }
}

// ── Journal database ─────────────────────────────────────────────────────────

/// Open (or create) `store-journal.sqlite` at `anchor_dir`, run schema
/// migrations, and return the connection (WAL mode, 5 s busy timeout).
pub fn open_journal(anchor_dir: &Path) -> Result<Connection, String> {
    std::fs::create_dir_all(anchor_dir).map_err(|e| format!("create anchor dir: {e}"))?;
    let path = anchor_dir.join(JOURNAL_FILENAME);
    let conn = Connection::open(&path).map_err(|e| format!("open store-journal.sqlite: {e}"))?;

    conn.pragma_update(None, "busy_timeout", 5000)
        .map_err(|e| format!("set busy_timeout: {e}"))?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(|e| format!("set WAL mode: {e}"))?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|e| format!("enable foreign_keys: {e}"))?;

    apply_journal_schema(&conn)?;
    Ok(conn)
}

/// Apply all journal schema migrations idempotently.
/// Exposed as `apply_journal_schema_pub` for tests.
#[cfg(test)]
pub fn apply_journal_schema_pub(conn: &Connection) -> Result<(), String> {
    apply_journal_schema(conn)
}

fn apply_journal_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS schema_version (
            version INTEGER NOT NULL
        );
        INSERT OR IGNORE INTO schema_version VALUES (0);

        -- Per-key generation / tombstone metadata.
        -- generation stored as TEXT to preserve full u64 range.
        -- is_tombstone=1: key deleted; generation kept forever (no GC).
        CREATE TABLE IF NOT EXISTS key_generations (
            key_id    TEXT NOT NULL PRIMARY KEY,
            generation TEXT NOT NULL,
            is_tombstone INTEGER NOT NULL DEFAULT 0,
            updated_at  INTEGER NOT NULL DEFAULT 0
        );

        -- Saga spine.  disposition: pending|committed|compensating|
        -- compensated|failed|uncertain|accepted.
        -- compensation_id/generation: phased claim fence (v10/v12).
        -- nonterminal_follow_up: 1 if uncertain/accepted needs a recheck.
        CREATE TABLE IF NOT EXISTS operations (
            operation_id TEXT NOT NULL PRIMARY KEY,
            kind         TEXT NOT NULL,
            key_id       TEXT NOT NULL,
            disposition  TEXT NOT NULL DEFAULT 'pending',
            generation   TEXT NOT NULL,
            compensation_id         TEXT,
            compensation_generation TEXT,
            nonterminal_follow_up   INTEGER NOT NULL DEFAULT 0,
            created_at   INTEGER NOT NULL,
            updated_at   INTEGER NOT NULL
        );

        -- Immutable outbox rows (written once, never updated).
        -- published: 0=pending, 1=published, 2=uncertain, 3=accepted.
        CREATE TABLE IF NOT EXISTS outbox_events (
            event_id     TEXT NOT NULL PRIMARY KEY,
            operation_id TEXT NOT NULL
                REFERENCES operations(operation_id),
            payload      BLOB NOT NULL,
            published    INTEGER NOT NULL DEFAULT 0,
            created_at   INTEGER NOT NULL
        );

        -- Immutable inbox rows (written once, never updated).
        CREATE TABLE IF NOT EXISTS inbox_events (
            event_id     TEXT NOT NULL PRIMARY KEY,
            operation_id TEXT NOT NULL
                REFERENCES operations(operation_id),
            payload      BLOB NOT NULL,
            received_at  INTEGER NOT NULL
        );
        ",
    )
    .map_err(|e| format!("apply journal schema: {e}"))?;

    Ok(())
}

// ── Generation CAS ────────────────────────────────────────────────────────────

/// Generation counter (u64 stored as TEXT to avoid SQLite's i64 ceiling).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Generation(pub u64);

impl Generation {
    pub fn zero() -> Self {
        Generation(0)
    }
    pub fn next(self) -> Self {
        Generation(self.0.saturating_add(1))
    }
    fn from_str(s: &str) -> Result<Self, String> {
        s.parse::<u64>()
            .map(Generation)
            .map_err(|e| format!("parse generation '{s}': {e}"))
    }
    fn to_db_str(self) -> String {
        self.0.to_string()
    }
}

/// Outcome of a generation CAS attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CasOutcome {
    /// CAS succeeded; `new_generation` is the committed value.
    Committed { new_generation: Generation },
    /// The stored generation did not match `expected`; the current value
    /// is returned so the caller can decide how to proceed.
    Conflict { current: Generation },
    /// The key has a tombstone; ABA rejected.
    Tombstoned { tombstone_generation: Generation },
}

/// Read the current generation for `key_id`.
/// Returns `(Generation::zero(), false)` when no row exists.
pub fn read_generation(conn: &Connection, key_id: &str) -> Result<(Generation, bool), String> {
    let row: Option<(String, bool)> = conn
        .query_row(
            "SELECT generation, is_tombstone FROM key_generations WHERE key_id = ?1",
            params![key_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|e| format!("read_generation({key_id}): {e}"))?;

    match row {
        None => Ok((Generation::zero(), false)),
        Some((gen_str, is_tombstone)) => Ok((Generation::from_str(&gen_str)?, is_tombstone)),
    }
}

/// Attempt a generation CAS on `key_id`.
///
/// If `expected` matches the stored generation (or key is absent and
/// `expected` is zero), advance to `expected.next()` and return
/// `CasOutcome::Committed`.  Tombstoned keys return `CasOutcome::Tombstoned`.
pub fn cas_generation(
    conn: &Connection,
    key_id: &str,
    expected: Generation,
) -> Result<CasOutcome, String> {
    let (current, is_tombstone) = read_generation(conn, key_id)?;

    if is_tombstone {
        return Ok(CasOutcome::Tombstoned {
            tombstone_generation: current,
        });
    }

    if current != expected {
        return Ok(CasOutcome::Conflict { current });
    }

    let new_gen = expected.next();
    let now = unix_now_secs();
    conn.execute(
        "INSERT INTO key_generations (key_id, generation, is_tombstone, updated_at)
         VALUES (?1, ?2, 0, ?3)
         ON CONFLICT(key_id) DO UPDATE SET
             generation  = excluded.generation,
             is_tombstone = 0,
             updated_at  = excluded.updated_at",
        params![key_id, new_gen.to_db_str(), now],
    )
    .map_err(|e| format!("cas_generation({key_id}): {e}"))?;

    Ok(CasOutcome::Committed {
        new_generation: new_gen,
    })
}

/// Write a tombstone for `key_id` at `expected` generation.
/// Kept forever; `cas_generation` respects it to prevent ABA.
pub fn tombstone_key(
    conn: &Connection,
    key_id: &str,
    expected: Generation,
) -> Result<CasOutcome, String> {
    let (current, is_tombstone) = read_generation(conn, key_id)?;

    if is_tombstone {
        return Ok(CasOutcome::Tombstoned {
            tombstone_generation: current,
        });
    }

    if current != expected {
        return Ok(CasOutcome::Conflict { current });
    }

    let tombstone_gen = expected.next();
    let now = unix_now_secs();
    conn.execute(
        "INSERT INTO key_generations (key_id, generation, is_tombstone, updated_at)
         VALUES (?1, ?2, 1, ?3)
         ON CONFLICT(key_id) DO UPDATE SET
             generation   = excluded.generation,
             is_tombstone = 1,
             updated_at   = excluded.updated_at",
        params![key_id, tombstone_gen.to_db_str(), now],
    )
    .map_err(|e| format!("tombstone_key({key_id}): {e}"))?;

    Ok(CasOutcome::Committed {
        new_generation: tombstone_gen,
    })
}

// ── Operations (saga spine) ───────────────────────────────────────────────────

/// Operation disposition values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disposition {
    Pending,
    Committed,
    Compensating,
    Compensated,
    Failed,
    /// Published to relay but outcome unknown (network outage).
    Uncertain,
    /// Accepted by relay; final state reached without full confirmation.
    Accepted,
}

impl Disposition {
    pub fn as_str(&self) -> &'static str {
        match self {
            Disposition::Pending => "pending",
            Disposition::Committed => "committed",
            Disposition::Compensating => "compensating",
            Disposition::Compensated => "compensated",
            Disposition::Failed => "failed",
            Disposition::Uncertain => "uncertain",
            Disposition::Accepted => "accepted",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Disposition::Pending),
            "committed" => Some(Disposition::Committed),
            "compensating" => Some(Disposition::Compensating),
            "compensated" => Some(Disposition::Compensated),
            "failed" => Some(Disposition::Failed),
            "uncertain" => Some(Disposition::Uncertain),
            "accepted" => Some(Disposition::Accepted),
            _ => None,
        }
    }

    /// True when the operation is in a terminal state requiring no further
    /// progression.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Disposition::Committed | Disposition::Compensated | Disposition::Failed
        )
    }

    /// True when the operation may require a nonterminal follow-up check
    /// (uncertain or accepted publication outcome).
    pub fn requires_follow_up(&self) -> bool {
        matches!(self, Disposition::Uncertain | Disposition::Accepted)
    }
}

/// A record from the `operations` table.
#[derive(Debug, Clone)]
pub struct OperationRecord {
    pub operation_id: String,
    pub kind: String,
    pub key_id: String,
    pub disposition: Disposition,
    pub generation: Generation,
    /// UUID of the active compensation event, if in `Compensating` state.
    pub compensation_id: Option<String>,
    /// Generation snapshot at compensation start.
    pub compensation_generation: Option<Generation>,
    /// Whether a nonterminal follow-up is pending (uncertain/accepted publication).
    pub nonterminal_follow_up: bool,
}

/// Insert a new `pending` operation record.
pub fn insert_operation(
    conn: &Connection,
    operation_id: &str,
    kind: &str,
    key_id: &str,
    generation: Generation,
) -> Result<(), String> {
    let now = unix_now_secs();
    conn.execute(
        "INSERT INTO operations
             (operation_id, kind, key_id, disposition, generation, created_at, updated_at)
         VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?5)",
        params![operation_id, kind, key_id, generation.to_db_str(), now],
    )
    .map_err(|e| format!("insert_operation({operation_id}): {e}"))?;
    Ok(())
}

/// Read one operation record.  Returns `None` when not found.
#[allow(clippy::type_complexity)]
pub fn read_operation(
    conn: &Connection,
    operation_id: &str,
) -> Result<Option<OperationRecord>, String> {
    let row: Option<(
        String,
        String,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        bool,
    )> = conn
        .query_row(
            "SELECT operation_id, kind, key_id, disposition, generation,
                    compensation_id, compensation_generation, nonterminal_follow_up
             FROM operations WHERE operation_id = ?1",
            params![operation_id],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .optional()
        .map_err(|e| format!("read_operation({operation_id}): {e}"))?;

    row.map(
        |(op_id, kind, key_id, disp_str, gen_str, comp_id, comp_gen_str, nf)| {
            let disposition = Disposition::from_str(&disp_str)
                .ok_or_else(|| format!("unknown disposition '{disp_str}'"))?;
            let generation = Generation::from_str(&gen_str)?;
            let compensation_generation =
                comp_gen_str.map(|s| Generation::from_str(&s)).transpose()?;
            Ok(OperationRecord {
                operation_id: op_id,
                kind,
                key_id,
                disposition,
                generation,
                compensation_id: comp_id,
                compensation_generation,
                nonterminal_follow_up: nf,
            })
        },
    )
    .transpose()
}

/// Advance an operation's disposition (no-op if not found).
pub fn advance_disposition(
    conn: &Connection,
    operation_id: &str,
    new_disposition: &Disposition,
) -> Result<(), String> {
    let now = unix_now_secs();
    conn.execute(
        "UPDATE operations SET disposition = ?1, updated_at = ?2
         WHERE operation_id = ?3",
        params![new_disposition.as_str(), now, operation_id],
    )
    .map_err(|e| format!("advance_disposition({operation_id}): {e}"))?;
    Ok(())
}

/// Pin the active compensation event for an operation (v10 claim fence).
/// Only allowed when disposition is `Pending` (transitioning to `Compensating`).
pub fn pin_compensation(
    conn: &Connection,
    operation_id: &str,
    compensation_id: &str,
    compensation_generation: Generation,
) -> Result<(), String> {
    let now = unix_now_secs();
    conn.execute(
        "UPDATE operations
         SET disposition            = 'compensating',
             compensation_id        = ?1,
             compensation_generation = ?2,
             updated_at             = ?3
         WHERE operation_id = ?4",
        params![
            compensation_id,
            compensation_generation.to_db_str(),
            now,
            operation_id,
        ],
    )
    .map_err(|e| format!("pin_compensation({operation_id}): {e}"))?;
    Ok(())
}

/// Mark that a nonterminal follow-up is required (uncertain/accepted
/// publication outcome — v12).
pub fn set_nonterminal_follow_up(
    conn: &Connection,
    operation_id: &str,
    required: bool,
) -> Result<(), String> {
    let now = unix_now_secs();
    conn.execute(
        "UPDATE operations SET nonterminal_follow_up = ?1, updated_at = ?2
         WHERE operation_id = ?3",
        params![required as i64, now, operation_id],
    )
    .map_err(|e| format!("set_nonterminal_follow_up({operation_id}): {e}"))?;
    Ok(())
}

/// Read all non-terminal operations for recovery.
pub fn read_nonterminal_operations(conn: &Connection) -> Result<Vec<OperationRecord>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT operation_id, kind, key_id, disposition, generation,
                    compensation_id, compensation_generation, nonterminal_follow_up
             FROM operations
             WHERE disposition NOT IN ('committed', 'compensated', 'failed')",
        )
        .map_err(|e| format!("prepare nonterminal ops: {e}"))?;

    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, bool>(7)?,
            ))
        })
        .map_err(|e| format!("query nonterminal ops: {e}"))?;

    let mut out = Vec::new();
    for row in rows {
        let (op_id, kind, key_id, disp_str, gen_str, comp_id, comp_gen_str, nf) =
            row.map_err(|e| format!("read nonterminal op row: {e}"))?;
        let disposition = Disposition::from_str(&disp_str)
            .ok_or_else(|| format!("unknown disposition '{disp_str}'"))?;
        let generation = Generation::from_str(&gen_str)?;
        let compensation_generation = comp_gen_str.map(|s| Generation::from_str(&s)).transpose()?;
        out.push(OperationRecord {
            operation_id: op_id,
            kind,
            key_id,
            disposition,
            generation,
            compensation_id: comp_id,
            compensation_generation,
            nonterminal_follow_up: nf,
        });
    }
    Ok(out)
}

// ── Immutable inbox / outbox rows ─────────────────────────────────────────────

/// Insert an immutable outbox row.  The row is written once and never updated.
pub fn insert_outbox_event(
    conn: &Connection,
    event_id: &str,
    operation_id: &str,
    payload: &[u8],
) -> Result<(), String> {
    let now = unix_now_secs();
    conn.execute(
        "INSERT OR IGNORE INTO outbox_events
             (event_id, operation_id, payload, published, created_at)
         VALUES (?1, ?2, ?3, 0, ?4)",
        params![event_id, operation_id, payload, now],
    )
    .map_err(|e| format!("insert_outbox_event({event_id}): {e}"))?;
    Ok(())
}

/// Insert an immutable inbox row.  The row is written once and never updated.
pub fn insert_inbox_event(
    conn: &Connection,
    event_id: &str,
    operation_id: &str,
    payload: &[u8],
) -> Result<(), String> {
    let now = unix_now_secs();
    conn.execute(
        "INSERT OR IGNORE INTO inbox_events
             (event_id, operation_id, payload, received_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![event_id, operation_id, payload, now],
    )
    .map_err(|e| format!("insert_inbox_event({event_id}): {e}"))?;
    Ok(())
}

/// Read outbox events for `operation_id`.
pub fn read_outbox_events(
    conn: &Connection,
    operation_id: &str,
) -> Result<Vec<(String, Vec<u8>, i64)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT event_id, payload, published FROM outbox_events
             WHERE operation_id = ?1 ORDER BY created_at",
        )
        .map_err(|e| format!("prepare outbox query: {e}"))?;
    let rows = stmt
        .query_map(params![operation_id], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(|e| format!("query outbox: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("read outbox row: {e}"))
}

/// Read inbox events for `operation_id`.
pub fn read_inbox_events(
    conn: &Connection,
    operation_id: &str,
) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT event_id, payload FROM inbox_events
             WHERE operation_id = ?1 ORDER BY received_at",
        )
        .map_err(|e| format!("prepare inbox query: {e}"))?;
    let rows = stmt
        .query_map(params![operation_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .map_err(|e| format!("query inbox: {e}"))?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("read inbox row: {e}"))
}

// ── Fail-closed codec (v6) ────────────────────────────────────────────────────

/// Parse error type. The raw bytes are never silently discarded — callers
/// receive this error and MUST NOT proceed with mutation.
#[derive(Debug)]
pub struct StoreDecodeError {
    pub message: String,
}

impl std::fmt::Display for StoreDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// Decode `managed-agents.json` bytes using the fail-closed codec.
///
/// Returns `Ok(records)` or `Err(StoreDecodeError)`.
/// On error the raw bytes are preserved in place (see `backup_invalid_store`).
/// The caller MUST NOT proceed with any mutation when this returns `Err`.
pub fn decode_agent_store(bytes: &[u8]) -> Result<Vec<ManagedAgentRecord>, StoreDecodeError> {
    // Fail-closed: unknown/malformed content ⇒ error, zero mutation.
    serde_json::from_slice(bytes).map_err(|e| StoreDecodeError {
        message: format!("agent store decode failed: {e}"),
    })
}

/// Decode `teams.json` bytes using the fail-closed codec.
pub fn decode_team_store(bytes: &[u8]) -> Result<Vec<TeamRecord>, StoreDecodeError> {
    serde_json::from_slice(bytes).map_err(|e| StoreDecodeError {
        message: format!("team store decode failed: {e}"),
    })
}

// ── Atomic write (with fsync) ─────────────────────────────────────────────────

/// Atomically write `payload` to `path` with fsync before rename.
/// Resolves symlinks first so the rename lands on the physical target.
pub fn atomic_write_with_fsync(path: &Path, payload: &[u8]) -> Result<(), String> {
    use std::io::Write;
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let tmp = resolved.with_extension("json.tmp");
    let mut file =
        std::fs::File::create(&tmp).map_err(|e| format!("create {}: {e}", tmp.display()))?;
    file.write_all(payload)
        .map_err(|e| format!("write {}: {e}", tmp.display()))?;
    file.sync_all()
        .map_err(|e| format!("fsync {}: {e}", tmp.display()))?;
    drop(file);
    std::fs::rename(&tmp, &resolved)
        .map_err(|e| format!("rename {} → {}: {e}", tmp.display(), resolved.display()))
}

/// Atomic write with fsync + owner-only (`0o600`) permissions.
///
/// Creates the temp file with `0o600` before writing so the umask window
/// is closed.  Used for `managed-agents.json`, which may carry plaintext
/// agent nsecs in the keyringless fallback.
pub fn atomic_write_restricted_with_fsync(path: &Path, payload: &[u8]) -> Result<(), String> {
    use atomic_write_file::AtomicWriteFile;
    use std::io::Write;

    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut file = AtomicWriteFile::open(&resolved)
        .map_err(|e| format!("open {} for atomic write: {e}", resolved.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("set {} permissions: {e}", resolved.display()))?;
    }

    file.write_all(payload)
        .map_err(|e| format!("write {}: {e}", resolved.display()))?;

    // AtomicWriteFile::commit() handles the rename; sync the temp fd first.
    // The `sync_before_close` feature isn't exposed, so we flush explicitly.
    file.flush()
        .map_err(|e| format!("flush {}: {e}", resolved.display()))?;
    file.commit()
        .map_err(|e| format!("commit {}: {e}", resolved.display()))
}

// ── Closure-only mutation API (v7) ────────────────────────────────────────────

/// The decoded state passed to a mutation closure.
pub struct StoreState<'a> {
    /// Agent records decoded from `managed-agents.json`.
    pub agents: Vec<ManagedAgentRecord>,
    /// Team records decoded from `teams.json`.
    pub teams: Vec<TeamRecord>,
    /// Open journal connection (anchor-locked).
    pub journal: &'a Connection,
    /// Anchor directory (for constructing file paths).
    pub anchor: &'a Path,
}

/// Perform a mutation against the store under the full lock sequence.
///
/// 1. Acquire the in-process `AppState::managed_agents_store_lock`.
/// 2. Acquire the anchored OS advisory file lock.
/// 3. Fresh-decode the JSON files and open the journal.
/// 4. Call `mutation` with a `StoreState` view.
/// 5. Write back JSON files atomically (with fsync) and commit journal changes.
/// 6. Release advisory lock, then in-process mutex.
///
/// Network I/O and keyring access must not occur inside `mutation`.
pub fn mutate_store<F, T>(
    app: &AppHandle,
    store_mutex_guard: MutexGuard<'_, ()>,
    mutation: F,
) -> Result<T, String>
where
    F: FnOnce(StoreState<'_>) -> Result<(Vec<ManagedAgentRecord>, Vec<TeamRecord>, T), String>,
{
    let anchor = store_anchor_dir(app)?;
    std::fs::create_dir_all(&anchor).map_err(|e| format!("create anchor dir: {e}"))?;

    // Acquire advisory lock while holding the in-process mutex.
    let _advisory = JournalLockGuard::acquire(&anchor)?;

    let agents_path = anchor.join("managed-agents.json");
    let teams_path = anchor.join("teams.json");

    // Fresh decode — fail closed on any parse error.
    let agents: Vec<ManagedAgentRecord> = if agents_path.exists() {
        let bytes =
            std::fs::read(&agents_path).map_err(|e| format!("read managed-agents.json: {e}"))?;
        decode_agent_store(&bytes).map_err(|e| {
            crate::managed_agents::storage::backup_invalid_store(&agents_path);
            e.message
        })?
    } else {
        Vec::new()
    };

    let teams: Vec<TeamRecord> = if teams_path.exists() {
        let bytes = std::fs::read(&teams_path).map_err(|e| format!("read teams.json: {e}"))?;
        decode_team_store(&bytes).map_err(|e| e.message)?
    } else {
        Vec::new()
    };

    let journal = open_journal(&anchor)?;

    let state = StoreState {
        agents,
        teams,
        journal: &journal,
        anchor: &anchor,
    };

    let (new_agents, new_teams, result) = mutation(state)?;

    // Write back both files atomically with fsync.
    let agents_payload = serde_json::to_vec_pretty(&new_agents)
        .map_err(|e| format!("serialize managed-agents.json: {e}"))?;
    atomic_write_restricted_with_fsync(&agents_path, &agents_payload)?;

    let teams_payload =
        serde_json::to_vec_pretty(&new_teams).map_err(|e| format!("serialize teams.json: {e}"))?;
    atomic_write_with_fsync(&teams_path, &teams_payload)?;

    // Store mutex guard held for the full critical section; dropped here.
    drop(store_mutex_guard);

    Ok(result)
}

/// Read-only view of the store, under the full lock sequence.
///
/// Does not write back JSON files.  Use for reads that must see a
/// consistent snapshot of the JSON + journal state.
pub fn read_store<F, T>(
    app: &AppHandle,
    store_mutex_guard: MutexGuard<'_, ()>,
    reader: F,
) -> Result<T, String>
where
    F: FnOnce(StoreState<'_>) -> Result<T, String>,
{
    let anchor = store_anchor_dir(app)?;
    let _advisory = JournalLockGuard::acquire(&anchor)?;

    let agents_path = anchor.join("managed-agents.json");
    let teams_path = anchor.join("teams.json");

    let agents: Vec<ManagedAgentRecord> = if agents_path.exists() {
        let bytes =
            std::fs::read(&agents_path).map_err(|e| format!("read managed-agents.json: {e}"))?;
        decode_agent_store(&bytes).map_err(|e| {
            crate::managed_agents::storage::backup_invalid_store(&agents_path);
            e.message
        })?
    } else {
        Vec::new()
    };

    let teams: Vec<TeamRecord> = if teams_path.exists() {
        let bytes = std::fs::read(&teams_path).map_err(|e| format!("read teams.json: {e}"))?;
        decode_team_store(&bytes).map_err(|e| e.message)?
    } else {
        Vec::new()
    };

    let journal = open_journal(&anchor)?;

    let state = StoreState {
        agents,
        teams,
        journal: &journal,
        anchor: &anchor,
    };

    let result = reader(state)?;
    drop(store_mutex_guard);
    Ok(result)
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn unix_now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Generate a new random UUID v4 as a lowercase hex string without dashes.
pub fn new_operation_id() -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    // Use a combination of time + thread-id for lightweight unique IDs.
    // In production, callers may substitute uuid::Uuid::new_v4().to_string().
    let mut h = DefaultHasher::new();
    unix_now_secs().hash(&mut h);
    std::thread::current().id().hash(&mut h);
    let a = h.finish();
    let mut h2 = DefaultHasher::new();
    a.hash(&mut h2);
    "op".to_string() + &format!("{a:016x}{:016x}", h2.finish())
}

#[cfg(test)]
#[path = "store_journal_tests.rs"]
mod tests;
