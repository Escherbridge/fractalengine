//! Blinded swarm-topic labels and their keyed BLAKE3 derivation (SPEC-3 §6.1).

use data_encoding::BASE32_NOPAD;

use crate::cbor::{decode_canonical as decode_cbor, encode_canonical_checked, CborValue};
use crate::envelope::Scope;

use super::verbs::ObjectClass;
use super::{require_numeric_keys, scope_at, u8_at, unsigned_at, CapabilityError};

/// The only topic-label version v1 admits.
pub const TOPIC_VERSION: u64 = 1;

/// NUL-terminated ASCII domain separator for topic-key derivation (§6).
pub const TOPIC_KEY_DOMAIN: &[u8] = b"fe-topic-key-v1\0";

/// NUL-terminated ASCII domain separator for the topic MAC (§6.1 rule 3).
pub const TOPIC_DOMAIN: &[u8] = b"fe-topic-v1\0";

/// Prefix every derived topic name carries.
pub const TOPIC_NAME_PREFIX: &str = "fe-topic-v1/";

const CONTEXT: &str = "topic_label";

/// One of the three §6.1 replication lanes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TopicLane {
    /// Verse-wide causal headers.
    Header,
    /// Petal- or resource-affine encrypted payloads.
    Payload,
    /// Availability and seeding announcements.
    Availability,
}

impl TopicLane {
    /// Every lane in wire order.
    pub const ALL: [TopicLane; 3] = [
        TopicLane::Header,
        TopicLane::Payload,
        TopicLane::Availability,
    ];

    /// The §6.1 wire value for this lane.
    pub const fn code(self) -> u64 {
        match self {
            TopicLane::Header => 0,
            TopicLane::Payload => 1,
            TopicLane::Availability => 2,
        }
    }

    /// The lane a wire value names, or `None` for any other value.
    pub const fn from_code(code: u64) -> Option<Self> {
        match code {
            0 => Some(TopicLane::Header),
            1 => Some(TopicLane::Payload),
            2 => Some(TopicLane::Availability),
            _ => None,
        }
    }
}

/// The five-key §6.1 topic label.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TopicLabel {
    /// Which replication lane the topic serves.
    pub lane: TopicLane,
    /// The single §3.2 class bit the topic carries.
    pub object_class: ObjectClass,
    /// The scope the topic blinds.
    pub scope: Scope,
    /// The current epoch of the membership/topic-key scope.
    pub topic_epoch: u64,
}

impl TopicLabel {
    /// Encodes the five-key §6.1 label map.
    pub fn to_cbor(&self) -> Result<CborValue, CapabilityError> {
        Ok(CborValue::Map(vec![
            (CborValue::Uint(0), CborValue::Uint(TOPIC_VERSION)),
            (CborValue::Uint(1), CborValue::Uint(self.lane.code())),
            (
                CborValue::Uint(2),
                CborValue::Uint(u64::from(self.object_class.bit())),
            ),
            (CborValue::Uint(3), self.scope.to_cbor()?),
            (CborValue::Uint(4), CborValue::Uint(self.topic_epoch)),
        ]))
    }

    /// Decodes the five-key §6.1 label map.
    pub fn from_cbor(value: &CborValue) -> Result<Self, CapabilityError> {
        require_numeric_keys(value, 5, CONTEXT)?;
        let topic_version = unsigned_at(value, 0, CONTEXT)?;
        if topic_version != TOPIC_VERSION {
            return Err(CapabilityError::UnsupportedCapabilityVersion {
                version: topic_version,
            });
        }
        let lane_code = unsigned_at(value, 1, CONTEXT)?;
        let class_bit = u8_at(value, 2, CONTEXT)?;
        Ok(Self {
            lane: TopicLane::from_code(lane_code)
                .ok_or(CapabilityError::UnknownTopicLane { lane: lane_code })?,
            object_class: ObjectClass::from_bit(class_bit)
                .ok_or(CapabilityError::UnknownObjectClassBits { bits: class_bit })?,
            scope: scope_at(value, 3, CONTEXT)?,
            topic_epoch: unsigned_at(value, 4, CONTEXT)?,
        })
    }

    /// Encodes the canonical label bytes the topic MAC covers.
    pub fn encode_canonical(&self) -> Result<Vec<u8>, CapabilityError> {
        Ok(encode_canonical_checked(&self.to_cbor()?)?)
    }

    /// Decodes canonical label bytes, rejecting any non-canonical encoding.
    pub fn decode_canonical(bytes: &[u8]) -> Result<Self, CapabilityError> {
        Self::from_cbor(&decode_cbor(bytes)?)
    }
}

/// `canonical_scope_key_context`: the deterministic-CBOR pair of a scope map and its epoch.
///
/// §6 describes this pair in prose without a key table; the two-element array shape is
/// provisional and is recorded in `src/capability/AGENTS.md`.
pub fn scope_key_context(scope: &Scope, epoch: u64) -> Result<Vec<u8>, CapabilityError> {
    let pair = CborValue::Array(vec![scope.to_cbor()?, CborValue::Uint(epoch)]);
    Ok(encode_canonical_checked(&pair)?)
}

/// `topic_key = BLAKE3_keyed(scope_key, "fe-topic-key-v1" || 0x00 || scope_key_context)`.
pub fn derive_topic_key(
    scope_key: &[u8; 32],
    scope: &Scope,
    epoch: u64,
) -> Result<[u8; 32], CapabilityError> {
    let context = scope_key_context(scope, epoch)?;
    Ok(*blake3::keyed_hash(scope_key, &domain_preimage(TOPIC_KEY_DOMAIN, &context)).as_bytes())
}

/// `topic_digest = BLAKE3_keyed(topic_key, "fe-topic-v1" || 0x00 || canonical_topic_label)`.
pub fn derive_topic_digest(
    topic_key: &[u8; 32],
    label: &TopicLabel,
) -> Result<[u8; 32], CapabilityError> {
    let label_bytes = label.encode_canonical()?;
    Ok(*blake3::keyed_hash(topic_key, &domain_preimage(TOPIC_DOMAIN, &label_bytes)).as_bytes())
}

/// `topic_name = "fe-topic-v1/" || lowercase unpadded RFC 4648 base32(topic_digest)`.
pub fn topic_name(topic_digest: &[u8; 32]) -> String {
    format!(
        "{TOPIC_NAME_PREFIX}{}",
        BASE32_NOPAD.encode(topic_digest).to_lowercase()
    )
}

/// The whole §6.1 pipeline: scope key to topic key to digest to name.
///
/// The topic key is derived from the label's own scope and epoch, so a lane change rotates
/// the digest while a scope or epoch change rotates the key underneath it as well.
pub fn derive_topic_name(
    scope_key: &[u8; 32],
    label: &TopicLabel,
) -> Result<String, CapabilityError> {
    let topic_key = derive_topic_key(scope_key, &label.scope, label.topic_epoch)?;
    Ok(topic_name(&derive_topic_digest(&topic_key, label)?))
}

fn domain_preimage(domain: &[u8], body: &[u8]) -> Vec<u8> {
    debug_assert!(
        domain.ends_with(&[0]),
        "a domain separator must carry its terminating NUL byte"
    );
    let mut preimage = Vec::with_capacity(domain.len() + body.len());
    preimage.extend_from_slice(domain);
    preimage.extend_from_slice(body);
    preimage
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::Identifier32;
    use crate::segment::discovery_labels::{
        authorize_lane_subscription, BlindedTopic, BlindedTopicDerivation,
    };
    use crate::segment::hashseq::LaneKey;
    use crate::segment::payload_shard::PayloadTopicScope;
    use crate::segment::relay_policy::{PeerIdentity, RelayAuthorizationView};
    use crate::segment::SegmentError;

    fn identifier(filler: u8) -> Identifier32 {
        Identifier32([filler; 32])
    }

    fn verse_scope() -> Scope {
        Scope::verse_wide(identifier(0x11))
    }

    fn petal_scope() -> Scope {
        Scope::new(identifier(0x11), Some(identifier(0x22)), None).expect("petal scope")
    }

    fn scope_key() -> [u8; 32] {
        [0x5c; 32]
    }

    fn header_label() -> TopicLabel {
        TopicLabel {
            lane: TopicLane::Header,
            object_class: ObjectClass::Operation,
            scope: verse_scope(),
            topic_epoch: 7,
        }
    }

    #[test]
    fn a_topic_label_round_trips_through_canonical_cbor() {
        let label = header_label();
        let bytes = label.encode_canonical().expect("encode");
        assert_eq!(TopicLabel::decode_canonical(&bytes).expect("decode"), label);
        assert_eq!(
            label
                .to_cbor()
                .expect("cbor")
                .as_map()
                .map(|entries| entries.len()),
            Some(5)
        );
    }

    #[test]
    fn a_topic_label_rejects_an_unknown_lane_or_class_bit() {
        let mut entries = match header_label().to_cbor().expect("cbor") {
            CborValue::Map(entries) => entries,
            _ => unreachable!(),
        };
        entries[1] = (CborValue::Uint(1), CborValue::Uint(3));
        assert_eq!(
            TopicLabel::from_cbor(&CborValue::Map(entries.clone())),
            Err(CapabilityError::UnknownTopicLane { lane: 3 })
        );

        entries[1] = (CborValue::Uint(1), CborValue::Uint(0));
        entries[2] = (CborValue::Uint(2), CborValue::Uint(0x03));
        assert_eq!(
            TopicLabel::from_cbor(&CborValue::Map(entries)),
            Err(CapabilityError::UnknownObjectClassBits { bits: 0x03 }),
            "a class field carries exactly one §3.2 bit"
        );
    }

    /// The committed SPEC-3 vector; code changes to match it, never the reverse.
    fn fixture_topics() -> Vec<serde_json::Value> {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/capability-chain-v1.json");
        let text = std::fs::read_to_string(path).expect("read the capability fixture");
        let document: serde_json::Value =
            serde_json::from_str(&text).expect("parse the capability fixture");
        document["topics"]
            .as_array()
            .expect("topic vectors")
            .clone()
    }

    fn fixture_bytes32(value: &serde_json::Value) -> [u8; 32] {
        hex::decode(value.as_str().expect("hexadecimal string"))
            .expect("valid hexadecimal")
            .try_into()
            .expect("32 bytes")
    }

    #[test]
    fn blinded_topic_derivation_is_deterministic_and_lane_separated() {
        for vector in fixture_topics() {
            let vector_scope = Scope::new(
                Identifier32(fixture_bytes32(&vector["scope"]["verse_id_hex"])),
                if vector["scope"]["petal_id_hex"].is_null() {
                    None
                } else {
                    Some(Identifier32(fixture_bytes32(
                        &vector["scope"]["petal_id_hex"],
                    )))
                },
                if vector["scope"]["resource_id_hex"].is_null() {
                    None
                } else {
                    Some(Identifier32(fixture_bytes32(
                        &vector["scope"]["resource_id_hex"],
                    )))
                },
            )
            .expect("fixture scope");
            let vector_label = TopicLabel {
                lane: TopicLane::from_code(vector["lane"].as_u64().expect("lane"))
                    .expect("known lane"),
                object_class: ObjectClass::from_bit(
                    u8::try_from(vector["object_class"].as_u64().expect("class")).expect("u8"),
                )
                .expect("known class"),
                scope: vector_scope,
                topic_epoch: vector["topic_epoch"].as_u64().expect("epoch"),
            };
            let vector_scope_key = fixture_bytes32(&vector["scope_key_hex"]);

            assert_eq!(
                hex::encode(
                    scope_key_context(&vector_scope, vector_label.topic_epoch).expect("context")
                ),
                vector["scope_key_context_hex"].as_str().expect("hex")
            );
            let vector_topic_key =
                derive_topic_key(&vector_scope_key, &vector_scope, vector_label.topic_epoch)
                    .expect("topic key");
            assert_eq!(
                hex::encode(vector_topic_key),
                vector["topic_key_hex"].as_str().expect("hex")
            );
            assert_eq!(
                hex::encode(vector_label.encode_canonical().expect("encode")),
                vector["label_hex"].as_str().expect("hex")
            );
            assert_eq!(
                hex::encode(derive_topic_digest(&vector_topic_key, &vector_label).expect("digest")),
                vector["topic_digest_hex"].as_str().expect("hex")
            );
            assert_eq!(
                derive_topic_name(&vector_scope_key, &vector_label).expect("name"),
                vector["topic_name"].as_str().expect("topic name")
            );
        }

        let label = header_label();
        let first = derive_topic_name(&scope_key(), &label).expect("topic");
        let second = derive_topic_name(&scope_key(), &label).expect("topic");
        assert_eq!(first, second, "derivation is a pure function of its inputs");
        assert!(first.starts_with(TOPIC_NAME_PREFIX));
        assert_eq!(
            first.len(),
            TOPIC_NAME_PREFIX.len() + 52,
            "32 bytes of base32 without padding is 52 characters"
        );
        assert!(
            first
                .trim_start_matches(TOPIC_NAME_PREFIX)
                .chars()
                .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit()),
            "the name is lowercase unpadded base32"
        );

        let mut names = std::collections::BTreeSet::new();
        names.insert(first.clone());
        for lane in TopicLane::ALL {
            names.insert(
                derive_topic_name(&scope_key(), &TopicLabel { lane, ..label }).expect("topic"),
            );
        }
        assert_eq!(names.len(), 3, "the three lanes never share a topic name");

        let other_class = derive_topic_name(
            &scope_key(),
            &TopicLabel {
                object_class: ObjectClass::Segment,
                ..label
            },
        )
        .expect("topic");
        assert_ne!(first, other_class);

        let other_scope = derive_topic_name(
            &scope_key(),
            &TopicLabel {
                scope: petal_scope(),
                ..label
            },
        )
        .expect("topic");
        assert_ne!(first, other_scope);

        let other_epoch = derive_topic_name(
            &scope_key(),
            &TopicLabel {
                topic_epoch: 8,
                ..label
            },
        )
        .expect("topic");
        assert_ne!(first, other_epoch, "an epoch bump rotates the topic label");

        let other_key = derive_topic_name(&[0x5d; 32], &label).expect("topic");
        assert_ne!(
            first, other_key,
            "the label is a MAC under the scope key, not an unkeyed content address"
        );
    }

    #[test]
    fn a_topic_name_never_discloses_a_raw_identifier() {
        let label = TopicLabel {
            lane: TopicLane::Payload,
            object_class: ObjectClass::Operation,
            scope: petal_scope(),
            topic_epoch: 7,
        };
        let name = derive_topic_name(&scope_key(), &label).expect("topic");

        for raw in [identifier(0x11).0, identifier(0x22).0, scope_key()] {
            let base32 = BASE32_NOPAD.encode(&raw).to_lowercase();
            let hexadecimal = raw
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            assert!(!name.contains(&base32));
            assert!(!name.contains(&hexadecimal));
        }
        assert_ne!(
            name,
            topic_name(blake3::hash(&label.encode_canonical().expect("encode")).as_bytes()),
            "an unkeyed BLAKE3 of the label would be enumerable"
        );
    }

    #[test]
    fn the_topic_key_is_bound_to_its_scope_and_epoch() {
        let key_at_seven = derive_topic_key(&scope_key(), &verse_scope(), 7).expect("key");
        let key_at_eight = derive_topic_key(&scope_key(), &verse_scope(), 8).expect("key");
        let petal_key = derive_topic_key(&scope_key(), &petal_scope(), 7).expect("key");

        assert_ne!(key_at_seven, key_at_eight);
        assert_ne!(key_at_seven, petal_key);
        assert_eq!(
            key_at_seven,
            derive_topic_key(&scope_key(), &verse_scope(), 7).expect("key")
        );
        assert_ne!(
            key_at_seven,
            scope_key(),
            "the topic key is separated from the payload-sealing key"
        );

        let context = scope_key_context(&verse_scope(), 7).expect("context");
        assert_eq!(
            context,
            scope_key_context(&verse_scope(), 7).expect("context")
        );
        assert_ne!(context, scope_key_context(&verse_scope(), 8).expect("ctx"));
    }

    /// The SPEC-3 §6 derivation, supplied to the SPEC-6 §7 subscription gate as the real thing.
    ///
    /// The gate hands over a fully built normative label, so this is `derive_topic_name` and
    /// nothing else — there is no second lane taxonomy to translate through.
    struct Spec3Derivation {
        scope_key: [u8; 32],
    }

    impl BlindedTopicDerivation for Spec3Derivation {
        fn blinded_topic(&self, label: &TopicLabel) -> Result<String, SegmentError> {
            Ok(derive_topic_name(&self.scope_key, label)?)
        }
    }

    /// A view that authorizes one peer at one epoch and nobody else, ever.
    struct OnePeerAtOneEpoch {
        authorized: PeerIdentity,
        epoch: u64,
    }

    impl RelayAuthorizationView for OnePeerAtOneEpoch {
        fn current_scope_epoch(&self, _lane: &LaneKey) -> Option<u64> {
            Some(self.epoch)
        }

        fn current_key_id(&self, _lane: &LaneKey) -> Option<Identifier32> {
            Some(identifier(0x41))
        }

        fn has_seed_capability(&self, peer: &PeerIdentity, _: &LaneKey, epoch: u64) -> bool {
            *peer == self.authorized && epoch == self.epoch
        }

        fn has_fetch_capability(&self, peer: &PeerIdentity, _: &LaneKey, epoch: u64) -> bool {
            *peer == self.authorized && epoch == self.epoch
        }

        fn may_wrap_scope_key_for_peer(&self, _: &PeerIdentity, _: &LaneKey, _: u64) -> bool {
            false
        }
    }

    #[test]
    fn private_topic_requires_capability_and_rotates_on_epoch_bump() {
        let authorized = PeerIdentity([0x07; 32]);
        let stranger = PeerIdentity([0x08; 32]);
        let lane_scope = LaneKey::Payload(PayloadTopicScope {
            verse_id: identifier(0x11),
            petal_id: identifier(0x22),
            scope_epoch: 7,
            key_id: identifier(0x41),
        });
        let derivation = Spec3Derivation {
            scope_key: scope_key(),
        };
        let at_seven = OnePeerAtOneEpoch {
            authorized,
            epoch: 7,
        };

        let subscribe = |view: &OnePeerAtOneEpoch,
                         peer: &PeerIdentity,
                         epoch: u64|
         -> Result<BlindedTopic, SegmentError> {
            authorize_lane_subscription(
                view,
                peer,
                &lane_scope,
                TopicLane::Payload,
                ObjectClass::Segment,
                epoch,
                &derivation,
            )
        };

        let subscribed = subscribe(&at_seven, &authorized, 7)
            .expect("an authorized peer at the current epoch subscribes");

        assert_eq!(
            subscribe(&at_seven, &stranger, 7),
            Err(SegmentError::Unauthorized),
            "an unauthorized subscribe never reaches a derivable topic label"
        );

        let at_eight = OnePeerAtOneEpoch {
            authorized,
            epoch: 8,
        };
        assert_eq!(
            subscribe(&at_eight, &authorized, 7),
            Err(SegmentError::StaleScopeEpoch {
                requested: 7,
                current: 8,
            }),
            "after a bump the old epoch stops being subscribable"
        );

        let rotated = subscribe(&at_eight, &authorized, 8)
            .expect("the same peer resubscribes at the bumped epoch");
        assert_ne!(
            subscribed, rotated,
            "an epoch bump rotates the blinded topic label"
        );

        let payload_scope =
            Scope::new(identifier(0x11), Some(identifier(0x22)), None).expect("payload scope");
        let before = TopicLabel {
            lane: TopicLane::Payload,
            object_class: ObjectClass::Segment,
            scope: payload_scope,
            topic_epoch: 7,
        };
        let after = TopicLabel {
            topic_epoch: 8,
            ..before
        };
        assert_ne!(
            derive_topic_name(&scope_key(), &before).expect("name"),
            derive_topic_name(&scope_key(), &after).expect("name"),
            "the derived topic name itself rotates, not only the gate's answer"
        );
    }

    #[test]
    fn the_two_topic_domains_are_distinct_and_nul_terminated() {
        assert_eq!(TOPIC_KEY_DOMAIN, b"fe-topic-key-v1\0");
        assert_eq!(TOPIC_DOMAIN, b"fe-topic-v1\0");
        assert_ne!(TOPIC_KEY_DOMAIN, TOPIC_DOMAIN);
        assert!(!TOPIC_KEY_DOMAIN.starts_with(TOPIC_DOMAIN));
        assert!(!TOPIC_DOMAIN.starts_with(TOPIC_KEY_DOMAIN));
        assert_eq!(TOPIC_KEY_DOMAIN.last(), Some(&0u8));
        assert_eq!(TOPIC_DOMAIN.last(), Some(&0u8));
    }
}
