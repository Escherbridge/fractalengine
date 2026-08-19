//! Blinded discovery lanes and the capability gate on subscribing (SPEC-6 §7).
//!
//! No hashing and no lane taxonomy of its own. §7.1 says the derivation, the topic epoch and the
//! lane labels are normative in SPEC-3 §6 and MUST NOT be reimplemented, so this module names
//! nothing: it builds the normative `capability::topic::TopicLabel`, states the authorization
//! that must precede any use of one, and takes the keyed MAC as a trait. See
//! `segment/AGENTS.md` §discovery-lanes for how the four §7 traffic kinds map onto the three
//! §6.1 lanes.

use crate::capability::topic::{TopicLabel, TopicLane};
use crate::capability::verbs::ObjectClass;

use super::hashseq::LaneKey;
use super::relay_policy::{PeerIdentity, RelayAuthorizationView};
use super::SegmentError;

/// An opaque blinded topic name; never a raw scope identifier and never a text URL.
///
/// Holds the SPEC-3 §6.1 `topic_name`, `"fe-topic-v1/" || lowercase-base32(topic_digest)`, which
/// is exactly what `capability::topic::derive_topic_name` produces.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlindedTopic(String);

impl BlindedTopic {
    /// The derived topic name.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The SPEC-3 §6.1 keyed derivation, supplied by the capability layer.
///
/// Implementations MUST be `capability::topic::derive_topic_name` under the lane's current scope
/// key and nothing else. The label arrives fully built, so an implementation has no lane, epoch
/// or scope of its own to get wrong; deriving a topic from raw identifiers or with a different
/// MAC is a spec violation, not an implementation choice.
pub trait BlindedTopicDerivation {
    /// `derive_topic_name(scope_key_for(label.scope), label)`.
    fn blinded_topic(&self, label: &TopicLabel) -> Result<String, SegmentError>;
}

/// Validates the capability and epoch before a peer subscribes, announces, or requests (§7.3).
///
/// Returning the topic only after both checks is what makes the gate real: there is no path to
/// a subscribable label that skips the persistent authorization view. The label this builds is
/// the normative five-key §6.1 map, so `lane` is one of the three §6.1 lanes and `object_class`
/// is the single §3.2 class bit key 2 requires — a caller cannot name a fourth lane, and cannot
/// omit the class the label commits to.
pub fn authorize_lane_subscription(
    view: &impl RelayAuthorizationView,
    peer: &PeerIdentity,
    scope: &LaneKey,
    lane: TopicLane,
    object_class: ObjectClass,
    requested_epoch: u64,
    derivation: &impl BlindedTopicDerivation,
) -> Result<BlindedTopic, SegmentError> {
    let current = view
        .current_scope_epoch(scope)
        .ok_or(SegmentError::UnknownLane)?;
    if requested_epoch != current {
        return Err(SegmentError::StaleScopeEpoch {
            requested: requested_epoch,
            current,
        });
    }
    if !view.has_fetch_capability(peer, scope, current) {
        return Err(SegmentError::Unauthorized);
    }
    let label = TopicLabel {
        lane,
        object_class,
        scope: scope.topic_scope(),
        topic_epoch: current,
    };
    Ok(BlindedTopic(derivation.blinded_topic(&label)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    use crate::capability::topic::derive_topic_name;
    use crate::envelope::Identifier32;
    use crate::segment::payload_shard::PayloadTopicScope;
    use crate::segment::test_fixtures::identifier;

    /// The real SPEC-3 §6 derivation; nothing here reimplements the MAC.
    struct KeyedTopics {
        scope_key: [u8; 32],
    }

    impl BlindedTopicDerivation for KeyedTopics {
        fn blinded_topic(&self, label: &TopicLabel) -> Result<String, SegmentError> {
            Ok(derive_topic_name(&self.scope_key, label)?)
        }
    }

    struct AlwaysAuthorized {
        epoch: u64,
    }

    impl RelayAuthorizationView for AlwaysAuthorized {
        fn current_scope_epoch(&self, _lane: &LaneKey) -> Option<u64> {
            Some(self.epoch)
        }

        fn current_key_id(&self, _lane: &LaneKey) -> Option<Identifier32> {
            Some(identifier(0x41))
        }

        fn has_seed_capability(&self, _: &PeerIdentity, _: &LaneKey, _: u64) -> bool {
            true
        }

        fn has_fetch_capability(&self, _: &PeerIdentity, _: &LaneKey, _: u64) -> bool {
            true
        }

        fn may_wrap_scope_key_for_peer(&self, _: &PeerIdentity, _: &LaneKey, _: u64) -> bool {
            true
        }
    }

    struct NeverAuthorized;

    impl RelayAuthorizationView for NeverAuthorized {
        fn current_scope_epoch(&self, _lane: &LaneKey) -> Option<u64> {
            Some(7)
        }

        fn current_key_id(&self, _lane: &LaneKey) -> Option<Identifier32> {
            Some(identifier(0x41))
        }

        fn has_seed_capability(&self, _: &PeerIdentity, _: &LaneKey, _: u64) -> bool {
            false
        }

        fn has_fetch_capability(&self, _: &PeerIdentity, _: &LaneKey, _: u64) -> bool {
            false
        }

        fn may_wrap_scope_key_for_peer(&self, _: &PeerIdentity, _: &LaneKey, _: u64) -> bool {
            false
        }
    }

    #[test]
    fn private_discovery_uses_blinded_lane_separation() {
        let verse = identifier(0x11);
        let petal = identifier(0x21);
        let payload_scope = LaneKey::Payload(PayloadTopicScope {
            verse_id: verse,
            petal_id: petal,
            scope_epoch: 7,
            key_id: identifier(0x41),
        });
        let header_scope = LaneKey::Header { verse_id: verse };
        let peer = PeerIdentity([1; 32]);
        let view = AlwaysAuthorized { epoch: 7 };
        let derivation = KeyedTopics {
            scope_key: [0x5a; 32],
        };

        // The four §7 traffic kinds resolve onto the three §6.1 lanes and stay separated: the
        // manifest rides the verse-wide header lane as a Segment, not a fourth lane.
        let traffic = [
            (&header_scope, TopicLane::Header, ObjectClass::Operation),
            (&payload_scope, TopicLane::Payload, ObjectClass::Shard),
            (&header_scope, TopicLane::Header, ObjectClass::Segment),
            (
                &payload_scope,
                TopicLane::Availability,
                ObjectClass::Segment,
            ),
        ];
        let labels: Vec<BlindedTopic> = traffic
            .iter()
            .map(|&(scope, lane, class)| {
                authorize_lane_subscription(&view, &peer, scope, lane, class, 7, &derivation)
                    .expect("authorized")
            })
            .collect();
        let distinct: BTreeSet<&BlindedTopic> = labels.iter().collect();
        assert_eq!(distinct.len(), traffic.len());

        for label in &labels {
            assert!(label.as_str().starts_with("fe-topic-v1/"));
            for raw in [verse, petal, identifier(0x41)] {
                let hexadecimal = raw
                    .as_bytes()
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>();
                assert!(!label.as_str().contains(&hexadecimal));
            }
        }

        assert_eq!(
            authorize_lane_subscription(
                &NeverAuthorized,
                &peer,
                &header_scope,
                TopicLane::Header,
                ObjectClass::Operation,
                7,
                &derivation,
            ),
            Err(SegmentError::Unauthorized)
        );
    }

    #[test]
    fn a_lane_key_blinds_the_scope_spec3_section_6_1_rule_4_requires() {
        let verse = identifier(0x11);
        let petal = identifier(0x21);
        let header = LaneKey::Header { verse_id: verse };
        let payload = LaneKey::Payload(PayloadTopicScope {
            verse_id: verse,
            petal_id: petal,
            scope_epoch: 7,
            key_id: identifier(0x41),
        });

        assert_eq!(header.topic_scope().verse_id(), verse);
        assert_eq!(header.topic_scope().petal_id(), None);
        assert_eq!(payload.topic_scope().petal_id(), Some(petal));
        assert_eq!(payload.topic_scope().resource_id(), None);
    }
}
