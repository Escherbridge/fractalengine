//! SPEC-7 commit/preview wire protocol: transport-agnostic message types.
//!
//! No socket, no axum, no tower. See `src/wire/AGENTS.md` for the provisional field-number
//! table and the two undefined product numbers this module leaves to the caller.

pub mod commit;
pub mod cursor;
pub mod error;
pub mod frame;
pub mod preview;
pub mod preview_limiter;
pub mod session;
pub mod snapshot;
pub mod subscription;
/// Always compiled (not `#[cfg(test)]`-gated): `tests/spec7_conformance.rs` is a separate
/// crate that links only the library's public surface, so these fakes must be real exports.
pub mod test_support;

use crate::cbor::CborValue;
use crate::envelope::{Author, Hash32, Identifier32, Scope};

use self::error::WireError;
use self::session::SessionAuthorizationTable;

/// §6 rule 3 `session_generation_invalid`: stops a frame before commit, replay, snapshot, or
/// preview work. See `src/wire/AGENTS.md` §the-session-generation-gate.
pub fn require_current_session_generation(
    sessions: &SessionAuthorizationTable,
    generation: u64,
) -> Result<(), WireError> {
    if sessions.is_generation_valid(generation) {
        Ok(())
    } else {
        Err(WireError::StaleSessionGeneration { generation })
    }
}

/// Requires that `entries` carries exactly the keys in `expected`, no more and no fewer.
pub(crate) fn require_exact_keys(
    value: &CborValue,
    expected: &[u64],
    context: &'static str,
) -> Result<(), WireError> {
    let entries = value.as_map().ok_or(WireError::ExpectedMap { context })?;
    if entries.len() != expected.len() {
        return Err(WireError::UnexpectedKeys { context });
    }
    for key in expected {
        if value.get_uint_key(*key).is_none() {
            return Err(WireError::MissingKey { context, key: *key });
        }
    }
    Ok(())
}

/// Requires that `entries` carries none of `forbidden` — defense in depth for preview isolation.
pub(crate) fn require_no_keys(
    value: &CborValue,
    forbidden: &[(u64, &'static str)],
) -> Result<(), WireError> {
    for (key, name) in forbidden {
        if value.get_uint_key(*key).is_some() {
            return Err(WireError::ForbiddenPreviewField { field: name });
        }
    }
    Ok(())
}

pub(crate) fn get<'a>(
    value: &'a CborValue,
    key: u64,
    context: &'static str,
) -> Result<&'a CborValue, WireError> {
    value
        .get_uint_key(key)
        .ok_or(WireError::MissingKey { context, key })
}

pub(crate) fn uint_at(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<u64, WireError> {
    get(value, key, context)?
        .as_uint()
        .ok_or(WireError::WrongShape { context, key })
}

pub(crate) fn u32_at(value: &CborValue, key: u64, context: &'static str) -> Result<u32, WireError> {
    let raw = uint_at(value, key, context)?;
    u32::try_from(raw).map_err(|_| WireError::WrongShape { context, key })
}

pub(crate) fn bytes_at<'a>(
    value: &'a CborValue,
    key: u64,
    context: &'static str,
) -> Result<&'a [u8], WireError> {
    get(value, key, context)?
        .as_bytes()
        .ok_or(WireError::WrongShape { context, key })
}

pub(crate) fn fixed_bytes_at<const LENGTH: usize>(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<[u8; LENGTH], WireError> {
    let raw = bytes_at(value, key, context)?;
    <[u8; LENGTH]>::try_from(raw).map_err(|_| WireError::WrongByteLength {
        context,
        key,
        expected: LENGTH,
        actual: raw.len(),
    })
}

pub(crate) fn hash32_at(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<Hash32, WireError> {
    Ok(Hash32(fixed_bytes_at::<32>(value, key, context)?))
}

pub(crate) fn identifier32_at(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<Identifier32, WireError> {
    Ok(Identifier32(fixed_bytes_at::<32>(value, key, context)?))
}

pub(crate) fn scope_at(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<Scope, WireError> {
    Ok(Scope::from_cbor(get(value, key, context)?)?)
}

pub(crate) fn scope_entry(scope: &Scope) -> Result<CborValue, WireError> {
    Ok(scope.to_cbor()?)
}

pub(crate) fn author_at(
    value: &CborValue,
    key: u64,
    context: &'static str,
) -> Result<Author, WireError> {
    Ok(Author::from_cbor(get(value, key, context)?)?)
}
