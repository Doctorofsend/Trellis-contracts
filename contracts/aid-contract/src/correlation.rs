use soroban_sdk::{Bytes, BytesN, Env};

use crate::storage;

/// Returns a zero-filled correlation ID for use as a default/placeholder.
pub fn zero_correlation_id(env: &Env) -> BytesN<32> {
    BytesN::from_array(env, &[0; 32])
}

/// Normalizes an optional free-form correlation ID into a deterministic
/// `Option<Bytes>` suitable for event emission and storage.
///
/// When `None` or empty, returns `None` (no correlation). Otherwise the raw
/// bytes are passed through unchanged — validation is intentionally lenient
/// here so callers can attach arbitrary opaque context.
pub fn normalize(_env: &Env, raw: Option<Bytes>) -> Option<Bytes> {
    match raw {
        Some(b) if b.len() > 0 => Some(b),
        _ => None,
    }
}

/// Persists the correlation ID for an aid record so it can be loaded during
/// claim/refund without requiring the caller to resupply it.
pub fn store(env: &Env, aid_id: u64, correlation_id: &Option<Bytes>) {
    if let Some(cid) = correlation_id {
        env.storage()
            .persistent()
            .set(&storage::DataKey::CorrelationId(aid_id), cid);
    }
}

/// Loads the stored correlation ID for an aid record, if one was attached
/// at creation time.
pub fn load(env: &Env, aid_id: u64) -> Option<Bytes> {
    env.storage()
        .persistent()
        .get(&storage::DataKey::CorrelationId(aid_id))
}
