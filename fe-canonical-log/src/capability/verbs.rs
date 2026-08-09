//! Capability verb and object-class bitsets (SPEC-3 §3.1, §3.2).

use super::CapabilityError;

/// Bit mask covering exactly the six §3.1 verb bits.
pub const VERB_BIT_MASK: u8 = 0x3f;

/// Bit mask covering exactly the six §3.2 object-class bits.
pub const OBJECT_CLASS_BIT_MASK: u8 = 0x3f;

/// One §3.1 capability verb.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Verb {
    /// Create an admissible immutable artifact or operation intent.
    Append,
    /// Request or receive the encrypted or header artifact; never plaintext access.
    Fetch,
    /// Resolve the scope key and authenticate or decrypt an artifact.
    Decrypt,
    /// Apply a verified artifact to a local projection or derived catalog.
    Materialize,
    /// Send or receive an ephemeral, lossy preview message.
    Preview,
    /// Retain and serve an immutable artifact to an authorized requester.
    Seed,
}

impl Verb {
    /// Every §3.1 verb in bit order.
    pub const ALL: [Verb; 6] = [
        Verb::Append,
        Verb::Fetch,
        Verb::Decrypt,
        Verb::Materialize,
        Verb::Preview,
        Verb::Seed,
    ];

    /// The §3.1 bit value for this verb.
    pub const fn bit(self) -> u8 {
        match self {
            Verb::Append => 0x01,
            Verb::Fetch => 0x02,
            Verb::Decrypt => 0x04,
            Verb::Materialize => 0x08,
            Verb::Preview => 0x10,
            Verb::Seed => 0x20,
        }
    }

    /// The verb a single §3.1 bit names, or `None` for any other value.
    pub const fn from_bit(bit: u8) -> Option<Self> {
        match bit {
            0x01 => Some(Verb::Append),
            0x02 => Some(Verb::Fetch),
            0x04 => Some(Verb::Decrypt),
            0x08 => Some(Verb::Materialize),
            0x10 => Some(Verb::Preview),
            0x20 => Some(Verb::Seed),
            _ => None,
        }
    }
}

/// One §3.2 object class.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ObjectClass {
    /// One operation header and its referenced payload artifact.
    Operation,
    /// An immutable, scope-affine segment or segment manifest.
    Segment,
    /// A signed replay-verifiable checkpoint claim and its snapshot artifact.
    Checkpoint,
    /// A Hexon data shard or sub-artifact.
    Shard,
    /// A terrain tile or tile-derived data artifact.
    Tile,
    /// A scene asset or asset-derived immutable artifact.
    Asset,
}

impl ObjectClass {
    /// Every §3.2 class in bit order.
    pub const ALL: [ObjectClass; 6] = [
        ObjectClass::Operation,
        ObjectClass::Segment,
        ObjectClass::Checkpoint,
        ObjectClass::Shard,
        ObjectClass::Tile,
        ObjectClass::Asset,
    ];

    /// The §3.2 bit value for this class.
    pub const fn bit(self) -> u8 {
        match self {
            ObjectClass::Operation => 0x01,
            ObjectClass::Segment => 0x02,
            ObjectClass::Checkpoint => 0x04,
            ObjectClass::Shard => 0x08,
            ObjectClass::Tile => 0x10,
            ObjectClass::Asset => 0x20,
        }
    }

    /// The class a single §3.2 bit names, or `None` for any other value.
    pub const fn from_bit(bit: u8) -> Option<Self> {
        match bit {
            0x01 => Some(ObjectClass::Operation),
            0x02 => Some(ObjectClass::Segment),
            0x04 => Some(ObjectClass::Checkpoint),
            0x08 => Some(ObjectClass::Shard),
            0x10 => Some(ObjectClass::Tile),
            0x20 => Some(ObjectClass::Asset),
            _ => None,
        }
    }
}

/// A non-empty subset of the §3.1 verb bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VerbSet(u8);

impl VerbSet {
    /// Builds a verb set, rejecting an unknown bit and the empty set (§2.2 key 8).
    pub fn from_bits(bits: u8) -> Result<Self, CapabilityError> {
        let unknown = bits & !VERB_BIT_MASK;
        if unknown != 0 {
            return Err(CapabilityError::UnknownVerbBits { bits: unknown });
        }
        if bits == 0 {
            return Err(CapabilityError::EmptyVerbSet);
        }
        Ok(Self(bits))
    }

    /// Builds a verb set from named verbs, rejecting the empty set.
    pub fn from_verbs(verbs: impl IntoIterator<Item = Verb>) -> Result<Self, CapabilityError> {
        let bits = verbs.into_iter().fold(0u8, |bits, verb| bits | verb.bit());
        Self::from_bits(bits)
    }

    /// The raw §3.1 bitset.
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Reports whether this set grants `verb`.
    pub const fn contains(self, verb: Verb) -> bool {
        self.0 & verb.bit() != 0
    }

    /// Reports whether every verb in this set is also in `other`.
    pub const fn is_subset_of(self, other: VerbSet) -> bool {
        self.0 & !other.0 == 0
    }

    /// The granted verbs in bit order.
    pub fn iter(self) -> impl Iterator<Item = Verb> {
        Verb::ALL
            .into_iter()
            .filter(move |verb| self.contains(*verb))
    }
}

/// A non-empty subset of the §3.2 object-class bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjectClassSet(u8);

impl ObjectClassSet {
    /// Builds a class set, rejecting an unknown bit and the empty set (§2.2 key 9).
    pub fn from_bits(bits: u8) -> Result<Self, CapabilityError> {
        let unknown = bits & !OBJECT_CLASS_BIT_MASK;
        if unknown != 0 {
            return Err(CapabilityError::UnknownObjectClassBits { bits: unknown });
        }
        if bits == 0 {
            return Err(CapabilityError::EmptyObjectClassSet);
        }
        Ok(Self(bits))
    }

    /// Builds a class set from named classes, rejecting the empty set.
    pub fn from_classes(
        classes: impl IntoIterator<Item = ObjectClass>,
    ) -> Result<Self, CapabilityError> {
        let bits = classes
            .into_iter()
            .fold(0u8, |bits, class| bits | class.bit());
        Self::from_bits(bits)
    }

    /// The raw §3.2 bitset.
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Reports whether this set grants `class`.
    pub const fn contains(self, class: ObjectClass) -> bool {
        self.0 & class.bit() != 0
    }

    /// Reports whether every class in this set is also in `other`.
    pub const fn is_subset_of(self, other: ObjectClassSet) -> bool {
        self.0 & !other.0 == 0
    }

    /// The granted classes in bit order.
    pub fn iter(self) -> impl Iterator<Item = ObjectClass> {
        ObjectClass::ALL
            .into_iter()
            .filter(move |class| self.contains(*class))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verb_bits_match_the_section_three_one_table() {
        assert_eq!(Verb::Append.bit(), 0x01);
        assert_eq!(Verb::Fetch.bit(), 0x02);
        assert_eq!(Verb::Decrypt.bit(), 0x04);
        assert_eq!(Verb::Materialize.bit(), 0x08);
        assert_eq!(Verb::Preview.bit(), 0x10);
        assert_eq!(Verb::Seed.bit(), 0x20);
        for verb in Verb::ALL {
            assert_eq!(Verb::from_bit(verb.bit()), Some(verb));
        }
        assert_eq!(Verb::from_bit(0x40), None);
        assert_eq!(Verb::from_bit(0x03), None);
    }

    #[test]
    fn object_class_bits_match_the_section_three_two_table() {
        assert_eq!(ObjectClass::Operation.bit(), 0x01);
        assert_eq!(ObjectClass::Segment.bit(), 0x02);
        assert_eq!(ObjectClass::Checkpoint.bit(), 0x04);
        assert_eq!(ObjectClass::Shard.bit(), 0x08);
        assert_eq!(ObjectClass::Tile.bit(), 0x10);
        assert_eq!(ObjectClass::Asset.bit(), 0x20);
        for class in ObjectClass::ALL {
            assert_eq!(ObjectClass::from_bit(class.bit()), Some(class));
        }
        assert_eq!(ObjectClass::from_bit(0x40), None);
    }

    #[test]
    fn a_bitset_rejects_an_unknown_bit_and_the_empty_set() {
        assert_eq!(VerbSet::from_bits(0), Err(CapabilityError::EmptyVerbSet));
        assert_eq!(
            VerbSet::from_bits(0x41),
            Err(CapabilityError::UnknownVerbBits { bits: 0x40 })
        );
        assert_eq!(
            ObjectClassSet::from_bits(0),
            Err(CapabilityError::EmptyObjectClassSet)
        );
        assert_eq!(
            ObjectClassSet::from_bits(0x80),
            Err(CapabilityError::UnknownObjectClassBits { bits: 0x80 })
        );
        assert_eq!(
            VerbSet::from_verbs(Vec::new()),
            Err(CapabilityError::EmptyVerbSet)
        );
        assert_eq!(
            ObjectClassSet::from_classes(Vec::new()),
            Err(CapabilityError::EmptyObjectClassSet)
        );
    }

    #[test]
    fn subset_and_containment_track_the_bits() {
        let broad = VerbSet::from_verbs([Verb::Append, Verb::Fetch, Verb::Seed]).expect("verbs");
        let narrow = VerbSet::from_verbs([Verb::Fetch]).expect("verbs");
        let sideways = VerbSet::from_verbs([Verb::Decrypt]).expect("verbs");

        assert!(narrow.is_subset_of(broad));
        assert!(broad.is_subset_of(broad));
        assert!(!broad.is_subset_of(narrow));
        assert!(!sideways.is_subset_of(broad));
        assert!(broad.contains(Verb::Seed));
        assert!(!broad.contains(Verb::Decrypt));
        assert_eq!(
            broad.iter().collect::<Vec<Verb>>(),
            vec![Verb::Append, Verb::Fetch, Verb::Seed]
        );

        let classes = ObjectClassSet::from_classes([ObjectClass::Operation, ObjectClass::Tile])
            .expect("classes");
        let single = ObjectClassSet::from_classes([ObjectClass::Tile]).expect("classes");
        assert!(single.is_subset_of(classes));
        assert!(!classes.is_subset_of(single));
        assert_eq!(
            classes.iter().collect::<Vec<ObjectClass>>(),
            vec![ObjectClass::Operation, ObjectClass::Tile]
        );
    }
}
