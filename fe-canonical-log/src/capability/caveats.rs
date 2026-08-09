//! The six-key caveat map and its attenuation rule (SPEC-3 §2.3, §2.4 step 5).

use crate::cbor::CborValue;
use crate::envelope::{Hash32, Identifier32, Scope};

use super::{
    hashes_to_cbor, identifiers_to_cbor, is_strictly_ascending, optional_byte_array_at,
    optional_u16_array_at, optional_unsigned_at, optional_unsigned_to_cbor, require_numeric_keys,
    CapabilityError,
};

const CONTEXT: &str = "caveats";

/// One row of the §2.3 caveat map, named for attenuation diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CaveatField {
    /// Key 0, maximum plaintext intent payload bytes.
    MaxPayloadBytes,
    /// Key 1, maximum encrypted segment bytes the audience may seal.
    MaxSegmentBytes,
    /// Key 2, allowlist of operation payload schema hashes.
    AllowedSchemaHashes,
    /// Key 3, allowlist of resource identifiers within `grant_scope`.
    ResourceAllowlist,
    /// Key 4, allowlist of operation kinds the audience may append.
    AllowedOperationKinds,
    /// Key 5, maximum previews per second accepted from this audience.
    MaxPreviewHz,
}

/// The §2.3 caveat map; `None` means this certificate adds no restriction for that row.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Caveats {
    /// Maximum plaintext intent payload bytes.
    pub max_payload_bytes: Option<u64>,
    /// Maximum encrypted segment bytes the audience may seal.
    pub max_segment_bytes: Option<u64>,
    /// Strictly ascending allowlist of payload schema hashes.
    pub allowed_schema_hashes: Option<Vec<Hash32>>,
    /// Strictly ascending allowlist of resource identifiers inside `grant_scope`.
    pub resource_allowlist: Option<Vec<Identifier32>>,
    /// Strictly ascending allowlist of non-zero operation kinds.
    pub allowed_operation_kinds: Option<Vec<u16>>,
    /// Maximum previews per second accepted from this audience.
    pub max_preview_hz: Option<u16>,
}

impl Caveats {
    /// The caveat map that adds no restriction at all.
    pub fn unrestricted() -> Self {
        Self::default()
    }

    /// Enforces §2.1 rule 4 array ordering, the non-zero kind rule, and §2.3 key 3 containment.
    pub fn validate(&self, grant_scope: &Scope) -> Result<(), CapabilityError> {
        if let Some(hashes) = &self.allowed_schema_hashes {
            if !is_strictly_ascending(hashes) {
                return Err(CapabilityError::UnsortedCaveatArray {
                    field: CaveatField::AllowedSchemaHashes,
                });
            }
        }
        if let Some(resources) = &self.resource_allowlist {
            if !is_strictly_ascending(resources) {
                return Err(CapabilityError::UnsortedCaveatArray {
                    field: CaveatField::ResourceAllowlist,
                });
            }
            if let Some(scoped_resource) = grant_scope.resource_id() {
                if resources
                    .iter()
                    .any(|resource| *resource != scoped_resource)
                {
                    return Err(CapabilityError::ResourceOutsideGrantScope);
                }
            }
        }
        if let Some(kinds) = &self.allowed_operation_kinds {
            if !is_strictly_ascending(kinds) {
                return Err(CapabilityError::UnsortedCaveatArray {
                    field: CaveatField::AllowedOperationKinds,
                });
            }
            if kinds.contains(&0) {
                return Err(CapabilityError::ZeroOperationKindInCaveat);
            }
        }
        Ok(())
    }

    /// The first row where `child` is less restrictive than this map, or `None` if it attenuates.
    ///
    /// A numeric row must satisfy `child <= parent`; an array row must satisfy
    /// `child ⊆ parent`, the empty array being the maximal narrowing; and a non-null parent
    /// row may never become `null` in a child.
    pub fn attenuation_violation(&self, child: &Caveats) -> Option<CaveatField> {
        if !numeric_attenuates(self.max_payload_bytes, child.max_payload_bytes) {
            return Some(CaveatField::MaxPayloadBytes);
        }
        if !numeric_attenuates(self.max_segment_bytes, child.max_segment_bytes) {
            return Some(CaveatField::MaxSegmentBytes);
        }
        if !array_attenuates(
            self.allowed_schema_hashes.as_deref(),
            child.allowed_schema_hashes.as_deref(),
        ) {
            return Some(CaveatField::AllowedSchemaHashes);
        }
        if !array_attenuates(
            self.resource_allowlist.as_deref(),
            child.resource_allowlist.as_deref(),
        ) {
            return Some(CaveatField::ResourceAllowlist);
        }
        if !array_attenuates(
            self.allowed_operation_kinds.as_deref(),
            child.allowed_operation_kinds.as_deref(),
        ) {
            return Some(CaveatField::AllowedOperationKinds);
        }
        if !numeric_attenuates(
            self.max_preview_hz.map(u64::from),
            child.max_preview_hz.map(u64::from),
        ) {
            return Some(CaveatField::MaxPreviewHz);
        }
        None
    }

    /// Reports whether `child` restricts at least as much as this map on every row.
    pub fn attenuates(&self, child: &Caveats) -> bool {
        self.attenuation_violation(child).is_none()
    }

    /// Encodes the §2.3 six-key caveat map.
    pub fn to_cbor(&self, grant_scope: &Scope) -> Result<CborValue, CapabilityError> {
        self.validate(grant_scope)?;
        Ok(CborValue::Map(vec![
            (
                CborValue::Uint(0),
                optional_unsigned_to_cbor(self.max_payload_bytes),
            ),
            (
                CborValue::Uint(1),
                optional_unsigned_to_cbor(self.max_segment_bytes),
            ),
            (
                CborValue::Uint(2),
                match &self.allowed_schema_hashes {
                    Some(hashes) => hashes_to_cbor(hashes),
                    None => CborValue::Null,
                },
            ),
            (
                CborValue::Uint(3),
                match &self.resource_allowlist {
                    Some(resources) => identifiers_to_cbor(resources),
                    None => CborValue::Null,
                },
            ),
            (
                CborValue::Uint(4),
                match &self.allowed_operation_kinds {
                    Some(kinds) => CborValue::Array(
                        kinds
                            .iter()
                            .map(|kind| CborValue::Uint(u64::from(*kind)))
                            .collect(),
                    ),
                    None => CborValue::Null,
                },
            ),
            (
                CborValue::Uint(5),
                optional_unsigned_to_cbor(self.max_preview_hz.map(u64::from)),
            ),
        ]))
    }

    /// Decodes the §2.3 map, rejecting an unknown caveat key.
    pub fn from_cbor(value: &CborValue, grant_scope: &Scope) -> Result<Self, CapabilityError> {
        require_numeric_keys(value, 6, CONTEXT)?;
        let raw_preview_hz = optional_unsigned_at(value, 5, CONTEXT)?;
        let max_preview_hz = match raw_preview_hz {
            Some(raw) => {
                Some(
                    u16::try_from(raw).map_err(|_| CapabilityError::IntegerOutOfRange {
                        context: CONTEXT,
                        key: 5,
                        value: raw,
                    })?,
                )
            }
            None => None,
        };
        let caveats = Self {
            max_payload_bytes: optional_unsigned_at(value, 0, CONTEXT)?,
            max_segment_bytes: optional_unsigned_at(value, 1, CONTEXT)?,
            allowed_schema_hashes: optional_byte_array_at::<32>(value, 2, CONTEXT)?
                .map(|raw| raw.into_iter().map(Hash32).collect()),
            resource_allowlist: optional_byte_array_at::<32>(value, 3, CONTEXT)?
                .map(|raw| raw.into_iter().map(Identifier32).collect()),
            allowed_operation_kinds: optional_u16_array_at(value, 4, CONTEXT)?,
            max_preview_hz,
        };
        caveats.validate(grant_scope)?;
        Ok(caveats)
    }
}

/// A non-null parent bound survives into the child and is never raised.
fn numeric_attenuates(parent: Option<u64>, child: Option<u64>) -> bool {
    match (parent, child) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(parent), Some(child)) => child <= parent,
    }
}

/// A non-null parent allowlist survives into the child and is never widened.
fn array_attenuates<T: PartialEq>(parent: Option<&[T]>, child: Option<&[T]>) -> bool {
    match (parent, child) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(parent), Some(child)) => child.iter().all(|item| parent.contains(item)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identifier(filler: u8) -> Identifier32 {
        Identifier32([filler; 32])
    }

    fn hash(filler: u8) -> Hash32 {
        Hash32([filler; 32])
    }

    fn verse_scope() -> Scope {
        Scope::verse_wide(identifier(0x01))
    }

    fn restrictive() -> Caveats {
        Caveats {
            max_payload_bytes: Some(4096),
            max_segment_bytes: Some(65536),
            allowed_schema_hashes: Some(vec![hash(0x10), hash(0x20)]),
            resource_allowlist: Some(vec![identifier(0x30), identifier(0x40)]),
            allowed_operation_kinds: Some(vec![1, 4]),
            max_preview_hz: Some(30),
        }
    }

    #[test]
    fn caveats_round_trip_through_canonical_cbor() {
        let caveats = restrictive();
        let encoded = caveats.to_cbor(&verse_scope()).expect("encode");
        assert_eq!(
            Caveats::from_cbor(&encoded, &verse_scope()).expect("decode"),
            caveats
        );

        let unrestricted = Caveats::unrestricted();
        let encoded = unrestricted.to_cbor(&verse_scope()).expect("encode");
        assert_eq!(
            Caveats::from_cbor(&encoded, &verse_scope()).expect("decode"),
            unrestricted
        );
    }

    #[test]
    fn an_unknown_caveat_key_is_rejected() {
        let mut entries = match restrictive().to_cbor(&verse_scope()).expect("encode") {
            CborValue::Map(entries) => entries,
            _ => unreachable!(),
        };
        entries.push((CborValue::Uint(6), CborValue::Uint(1)));
        assert_eq!(
            Caveats::from_cbor(&CborValue::Map(entries), &verse_scope()),
            Err(CapabilityError::UnexpectedMapKeys {
                context: "caveats",
                expected_key_count: 6,
            })
        );
    }

    #[test]
    fn caveat_arrays_must_be_strictly_ascending_and_duplicate_free() {
        let mut unsorted = restrictive();
        unsorted.allowed_schema_hashes = Some(vec![hash(0x20), hash(0x10)]);
        assert_eq!(
            unsorted.validate(&verse_scope()),
            Err(CapabilityError::UnsortedCaveatArray {
                field: CaveatField::AllowedSchemaHashes,
            })
        );

        let mut duplicated = restrictive();
        duplicated.allowed_operation_kinds = Some(vec![4, 4]);
        assert_eq!(
            duplicated.validate(&verse_scope()),
            Err(CapabilityError::UnsortedCaveatArray {
                field: CaveatField::AllowedOperationKinds,
            })
        );

        let mut zero_kind = restrictive();
        zero_kind.allowed_operation_kinds = Some(vec![0, 1]);
        assert_eq!(
            zero_kind.validate(&verse_scope()),
            Err(CapabilityError::ZeroOperationKindInCaveat)
        );

        let mut unsorted_resources = restrictive();
        unsorted_resources.resource_allowlist = Some(vec![identifier(0x40), identifier(0x30)]);
        assert_eq!(
            unsorted_resources.validate(&verse_scope()),
            Err(CapabilityError::UnsortedCaveatArray {
                field: CaveatField::ResourceAllowlist,
            })
        );
    }

    #[test]
    fn a_resource_allowlist_may_not_escape_a_resource_scoped_grant() {
        let scope = Scope::new(
            identifier(0x01),
            Some(identifier(0x02)),
            Some(identifier(0x30)),
        )
        .expect("resource scope");
        let mut caveats = Caveats {
            resource_allowlist: Some(vec![identifier(0x30)]),
            ..Caveats::unrestricted()
        };
        assert_eq!(caveats.validate(&scope), Ok(()));

        caveats.resource_allowlist = Some(vec![identifier(0x30), identifier(0x99)]);
        assert_eq!(
            caveats.validate(&scope),
            Err(CapabilityError::ResourceOutsideGrantScope)
        );
    }

    #[test]
    fn a_null_parent_row_permits_any_child_row() {
        let parent = Caveats::unrestricted();
        assert!(parent.attenuates(&restrictive()));
        assert!(parent.attenuates(&Caveats::unrestricted()));
    }

    #[test]
    fn a_non_null_parent_row_may_never_become_null_in_a_child() {
        let parent = restrictive();
        let mut child = restrictive();
        child.max_payload_bytes = None;
        assert_eq!(
            parent.attenuation_violation(&child),
            Some(CaveatField::MaxPayloadBytes)
        );

        let mut child = restrictive();
        child.allowed_schema_hashes = None;
        assert_eq!(
            parent.attenuation_violation(&child),
            Some(CaveatField::AllowedSchemaHashes)
        );
    }

    #[test]
    fn numeric_rows_only_shrink_and_array_rows_only_narrow() {
        let parent = restrictive();

        let mut narrower = restrictive();
        narrower.max_payload_bytes = Some(1024);
        narrower.max_preview_hz = Some(1);
        narrower.allowed_schema_hashes = Some(vec![hash(0x10)]);
        narrower.allowed_operation_kinds = Some(Vec::new());
        assert!(parent.attenuates(&narrower), "the empty set is a narrowing");

        let mut wider = restrictive();
        wider.max_payload_bytes = Some(4097);
        assert_eq!(
            parent.attenuation_violation(&wider),
            Some(CaveatField::MaxPayloadBytes)
        );

        let mut wider = restrictive();
        wider.max_preview_hz = Some(31);
        assert_eq!(
            parent.attenuation_violation(&wider),
            Some(CaveatField::MaxPreviewHz)
        );

        let mut wider = restrictive();
        wider.allowed_schema_hashes = Some(vec![hash(0x10), hash(0x20), hash(0x30)]);
        assert_eq!(
            parent.attenuation_violation(&wider),
            Some(CaveatField::AllowedSchemaHashes)
        );

        let mut wider = restrictive();
        wider.resource_allowlist = Some(vec![identifier(0x30), identifier(0x99)]);
        assert_eq!(
            parent.attenuation_violation(&wider),
            Some(CaveatField::ResourceAllowlist)
        );

        let mut wider = restrictive();
        wider.allowed_operation_kinds = Some(vec![1, 2, 4]);
        assert_eq!(
            parent.attenuation_violation(&wider),
            Some(CaveatField::AllowedOperationKinds)
        );

        let mut wider = restrictive();
        wider.max_segment_bytes = Some(65537);
        assert_eq!(
            parent.attenuation_violation(&wider),
            Some(CaveatField::MaxSegmentBytes)
        );
    }
}
