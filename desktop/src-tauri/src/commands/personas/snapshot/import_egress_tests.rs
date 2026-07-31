//! NIP-49 egress guard test for boundary 7: persona snapshot engram submit.
//!
//! Extracted from `import.rs` to keep that file under the file-size ratchet.

use super::import::submit_engram_event;

const NCRYPTSEC: &str = "ncryptsec1qgg9947rlpvqu76pj5ecreduf9jxhselq2nae2kghhvd5g7dgjtcxfqtd67p9m0w57lspw8gsq6yphnm8623nsl8xn9j4jdzz84zm3frztj3z7s35vpzmqf6ksu8r89qk5z2zxfmu5gv8th8wclt0h4p";

/// An engram body carrying an ncryptsec must be rejected by the guard
/// before any network I/O (the target port is a discard address; a guard
/// error — not a connection error — proves the abort ordering).
#[tokio::test]
async fn blocks_ncryptsec_before_network() {
    let state = crate::app_state::build_app_state();
    let keys = nostr::Keys::generate();
    let body = format!("{{\"content\":\"{NCRYPTSEC}\"}}");
    let err = submit_engram_event(
        &state,
        &keys,
        body.as_bytes(),
        "http://127.0.0.1:9/events",
        None,
    )
    .await
    .unwrap_err();
    assert!(err.contains("key-backup material"), "{err}");
}
