//! The §2.2 rule 4 branch-control payload and its four normative field numbers.

use crate::cbor::{
    decode_canonical as decode_cbor, encode_canonical_checked as encode_cbor_checked, CborValue,
};
use crate::envelope::{Hash32, Identifier32};
use crate::frontier::SortedFrontier;

use super::BranchError;

/// Field number of `action` (§2.2 rule 4).
pub const ACTION_KEY: u64 = 0;
/// Field number of `target_branch_id` (§2.2 rule 4).
pub const TARGET_BRANCH_ID_KEY: u64 = 1;
/// Field number of `selected_frontier` (§2.2 rule 4).
pub const SELECTED_FRONTIER_KEY: u64 = 2;
/// Field number of `source_branch_id` (§2.2 rule 4).
pub const SOURCE_BRANCH_ID_KEY: u64 = 3;

const FIELD_COUNT: u64 = 4;

/// The four §2.2 rule 4 branch-control actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BranchControlAction {
    /// `0`: register a new branch against its admitted `branch_genesis`.
    Create,
    /// `1`: freeze the target's committed selection and accumulate evidence only.
    Pause,
    /// `2`: adopt a replay-verified frontier on a tracking branch.
    Retarget,
    /// `3`: pin an immutable selection derived from a source branch.
    Detach,
}

impl BranchControlAction {
    /// The wire discriminant.
    pub const fn to_u64(self) -> u64 {
        match self {
            Self::Create => 0,
            Self::Pause => 1,
            Self::Retarget => 2,
            Self::Detach => 3,
        }
    }

    /// Classifies a wire discriminant, rejecting every value outside 0..=3.
    pub fn from_u64(action: u64) -> Result<Self, BranchError> {
        match action {
            0 => Ok(Self::Create),
            1 => Ok(Self::Pause),
            2 => Ok(Self::Retarget),
            3 => Ok(Self::Detach),
            other => Err(BranchError::UnknownAction { action: other }),
        }
    }

    /// True for the one action whose `source_branch_id` is null.
    pub const fn is_create(self) -> bool {
        matches!(self, Self::Create)
    }
}

/// The decrypted §2.2 rule 4 payload of a branch-control operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchControlPayload {
    /// Which control action the operation performs.
    pub action: BranchControlAction,
    /// The branch the action creates or modifies.
    pub target_branch_id: Identifier32,
    /// The strictly sorted, duplicate-free, non-empty selected frontier.
    pub selected_frontier: SortedFrontier,
    /// The branch the selection derives from; null only for create.
    pub source_branch_id: Option<Identifier32>,
}

impl BranchControlPayload {
    /// Enforces §2.2 rule 4: `source_branch_id` is null exactly when the action is create.
    pub fn validate(&self) -> Result<(), BranchError> {
        match (self.action.is_create(), self.source_branch_id.is_some()) {
            (true, true) => Err(BranchError::SourceBranchForbidden),
            (false, false) => Err(BranchError::SourceBranchRequired {
                action: self.action,
            }),
            _ => Ok(()),
        }
    }

    /// Encodes the four-key §2.2 rule 4 map.
    pub fn to_cbor(&self) -> Result<CborValue, BranchError> {
        self.validate()?;
        Ok(CborValue::Map(vec![
            (
                CborValue::Uint(ACTION_KEY),
                CborValue::Uint(self.action.to_u64()),
            ),
            (
                CborValue::Uint(TARGET_BRANCH_ID_KEY),
                CborValue::Bytes(self.target_branch_id.0.to_vec()),
            ),
            (
                CborValue::Uint(SELECTED_FRONTIER_KEY),
                CborValue::Array(
                    self.selected_frontier
                        .as_slice()
                        .iter()
                        .map(|op_id| CborValue::Bytes(op_id.0.to_vec()))
                        .collect(),
                ),
            ),
            (
                CborValue::Uint(SOURCE_BRANCH_ID_KEY),
                match self.source_branch_id {
                    Some(source) => CborValue::Bytes(source.0.to_vec()),
                    None => CborValue::Null,
                },
            ),
        ]))
    }

    /// Decodes the four-key §2.2 rule 4 map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, BranchError> {
        require_field_keys(value)?;
        let payload = Self {
            action: BranchControlAction::from_u64(unsigned_at(value, ACTION_KEY)?)?,
            target_branch_id: Identifier32(bytes_at::<32>(value, TARGET_BRANCH_ID_KEY)?),
            selected_frontier: frontier_at(value, SELECTED_FRONTIER_KEY)?,
            source_branch_id: optional_identifier_at(value, SOURCE_BRANCH_ID_KEY)?,
        };
        payload.validate()?;
        Ok(payload)
    }

    /// Encodes to canonical CBOR bytes; these are the bytes the payload AAD commits to.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, BranchError> {
        Ok(encode_cbor_checked(&self.to_cbor()?)?)
    }

    /// Decodes canonical CBOR bytes, rejecting any non-canonical encoding.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, BranchError> {
        Self::from_cbor(&decode_cbor(bytes)?)
    }
}

fn require_field_keys(value: &CborValue) -> Result<(), BranchError> {
    let entries = value.as_map().ok_or(BranchError::ExpectedMap)?;
    if entries.len() as u64 != FIELD_COUNT {
        return Err(BranchError::UnexpectedMapKeys);
    }
    for key in 0..FIELD_COUNT {
        if value.get_uint_key(key).is_none() {
            return Err(BranchError::MissingKey { key });
        }
    }
    Ok(())
}

fn entry(value: &CborValue, key: u64) -> Result<&CborValue, BranchError> {
    value
        .get_uint_key(key)
        .ok_or(BranchError::MissingKey { key })
}

fn unsigned_at(value: &CborValue, key: u64) -> Result<u64, BranchError> {
    entry(value, key)?
        .as_uint()
        .ok_or(BranchError::ExpectedUnsignedInteger { key })
}

fn bytes_at<const LENGTH: usize>(value: &CborValue, key: u64) -> Result<[u8; LENGTH], BranchError> {
    let raw = entry(value, key)?
        .as_bytes()
        .ok_or(BranchError::ExpectedByteString { key })?;
    raw.try_into().map_err(|_| BranchError::WrongByteLength {
        key,
        expected: LENGTH,
        actual: raw.len(),
    })
}

/// Reads the frontier array, rejecting an encoding that is not already strictly ascending.
///
/// [`SortedFrontier::try_new`] sorts its input, which is right for a locally built selection
/// and wrong on the wire: an unsorted array would decode into a frontier whose re-encoding
/// differs from the received bytes, so the payload hash and the selection would disagree.
fn frontier_at(value: &CborValue, key: u64) -> Result<SortedFrontier, BranchError> {
    let members = entry(value, key)?
        .as_array()
        .ok_or(BranchError::ExpectedArray { key })?
        .iter()
        .enumerate()
        .map(|(index, member)| {
            let raw = member
                .as_bytes()
                .ok_or(BranchError::ExpectedByteString { key })?;
            Ok(Hash32(raw.try_into().map_err(|_| {
                BranchError::WrongByteLength {
                    key: index as u64,
                    expected: 32,
                    actual: raw.len(),
                }
            })?))
        })
        .collect::<Result<Vec<Hash32>, BranchError>>()?;
    if let Some(predecessor_index) = members.windows(2).position(|pair| pair[0] >= pair[1]) {
        return Err(BranchError::FrontierNotStrictlyAscending {
            index: predecessor_index + 1,
        });
    }
    Ok(SortedFrontier::try_new(members)?)
}

fn optional_identifier_at(
    value: &CborValue,
    key: u64,
) -> Result<Option<Identifier32>, BranchError> {
    let slot = entry(value, key)?;
    if slot.is_null() {
        return Ok(None);
    }
    let raw = slot
        .as_bytes()
        .ok_or(BranchError::ExpectedNullOrByteString { key })?;
    Ok(Some(Identifier32(raw.try_into().map_err(|_| {
        BranchError::WrongByteLength {
            key,
            expected: 32,
            actual: raw.len(),
        }
    })?)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontier::FrontierError;

    fn op_id(filler: u8) -> Hash32 {
        Hash32([filler; 32])
    }

    fn identifier(filler: u8) -> Identifier32 {
        Identifier32([filler; 32])
    }

    fn retarget_payload() -> BranchControlPayload {
        BranchControlPayload {
            action: BranchControlAction::Retarget,
            target_branch_id: identifier(0x01),
            selected_frontier: SortedFrontier::try_new([op_id(0x30), op_id(0x20)])
                .expect("frontier"),
            source_branch_id: Some(identifier(0x02)),
        }
    }

    #[test]
    fn payload_round_trips_through_canonical_bytes_with_the_spec_field_numbers() {
        let payload = retarget_payload();
        let bytes = payload.encode_canonical().expect("encode");
        let decoded = BranchControlPayload::decode_canonical(&bytes).expect("decode");

        assert_eq!(decoded, payload);
        assert_eq!(decoded.encode_canonical().expect("re-encode"), bytes);

        let value = payload.to_cbor().expect("cbor");
        assert_eq!(value.as_map().map(<[_]>::len), Some(4));
        assert_eq!(
            value.get_uint_key(ACTION_KEY).and_then(CborValue::as_uint),
            Some(2)
        );
        assert_eq!(
            value
                .get_uint_key(TARGET_BRANCH_ID_KEY)
                .and_then(CborValue::as_bytes),
            Some(&identifier(0x01).0[..])
        );
        assert_eq!(
            value
                .get_uint_key(SELECTED_FRONTIER_KEY)
                .and_then(CborValue::as_array)
                .map(<[_]>::len),
            Some(2)
        );
        assert_eq!(
            value
                .get_uint_key(SOURCE_BRANCH_ID_KEY)
                .and_then(CborValue::as_bytes),
            Some(&identifier(0x02).0[..])
        );
    }

    #[test]
    fn every_action_discriminant_round_trips_and_others_are_refused() {
        for (discriminant, action) in [
            (0, BranchControlAction::Create),
            (1, BranchControlAction::Pause),
            (2, BranchControlAction::Retarget),
            (3, BranchControlAction::Detach),
        ] {
            assert_eq!(BranchControlAction::from_u64(discriminant), Ok(action));
            assert_eq!(action.to_u64(), discriminant);
        }
        assert_eq!(
            BranchControlAction::from_u64(4),
            Err(BranchError::UnknownAction { action: 4 })
        );
    }

    #[test]
    fn source_branch_is_null_exactly_for_create() {
        let mut create = retarget_payload();
        create.action = BranchControlAction::Create;
        assert_eq!(create.validate(), Err(BranchError::SourceBranchForbidden));
        assert_eq!(
            create.encode_canonical(),
            Err(BranchError::SourceBranchForbidden)
        );

        create.source_branch_id = None;
        assert_eq!(create.validate(), Ok(()));

        for action in [
            BranchControlAction::Pause,
            BranchControlAction::Retarget,
            BranchControlAction::Detach,
        ] {
            let sourceless = BranchControlPayload {
                action,
                source_branch_id: None,
                ..retarget_payload()
            };
            assert_eq!(
                sourceless.validate(),
                Err(BranchError::SourceBranchRequired { action })
            );
        }
    }

    #[test]
    fn a_frontier_array_that_is_not_strictly_ascending_is_refused_on_decode() {
        let mut entries = match retarget_payload().to_cbor().expect("cbor") {
            CborValue::Map(entries) => entries,
            _ => unreachable!(),
        };
        entries[SELECTED_FRONTIER_KEY as usize] = (
            CborValue::Uint(SELECTED_FRONTIER_KEY),
            CborValue::Array(vec![
                CborValue::Bytes(op_id(0x30).0.to_vec()),
                CborValue::Bytes(op_id(0x20).0.to_vec()),
            ]),
        );
        assert_eq!(
            BranchControlPayload::from_cbor(&CborValue::Map(entries.clone())),
            Err(BranchError::FrontierNotStrictlyAscending { index: 1 })
        );

        entries[SELECTED_FRONTIER_KEY as usize] = (
            CborValue::Uint(SELECTED_FRONTIER_KEY),
            CborValue::Array(vec![
                CborValue::Bytes(op_id(0x20).0.to_vec()),
                CborValue::Bytes(op_id(0x20).0.to_vec()),
            ]),
        );
        assert_eq!(
            BranchControlPayload::from_cbor(&CborValue::Map(entries.clone())),
            Err(BranchError::FrontierNotStrictlyAscending { index: 1 })
        );

        entries[SELECTED_FRONTIER_KEY as usize] = (
            CborValue::Uint(SELECTED_FRONTIER_KEY),
            CborValue::Array(Vec::new()),
        );
        assert_eq!(
            BranchControlPayload::from_cbor(&CborValue::Map(entries)),
            Err(BranchError::Frontier(FrontierError::Empty))
        );
    }

    #[test]
    fn an_unlisted_or_missing_field_is_refused() {
        let mut entries = match retarget_payload().to_cbor().expect("cbor") {
            CborValue::Map(entries) => entries,
            _ => unreachable!(),
        };
        let complete = entries.clone();
        entries.pop();
        assert_eq!(
            BranchControlPayload::from_cbor(&CborValue::Map(entries)),
            Err(BranchError::UnexpectedMapKeys)
        );

        let mut renamed = complete.clone();
        renamed[SOURCE_BRANCH_ID_KEY as usize] = (CborValue::Uint(9), CborValue::Null);
        assert_eq!(
            BranchControlPayload::from_cbor(&CborValue::Map(renamed)),
            Err(BranchError::MissingKey {
                key: SOURCE_BRANCH_ID_KEY
            })
        );

        let mut extra = complete;
        extra.push((CborValue::Uint(4), CborValue::Uint(0)));
        assert_eq!(
            BranchControlPayload::from_cbor(&CborValue::Map(extra)),
            Err(BranchError::UnexpectedMapKeys)
        );
    }
}
