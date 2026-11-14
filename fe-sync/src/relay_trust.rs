//! Relay trust: coverage evaluation, bounded selection, peer-address relay filter. See `src/AGENTS.md` section relay-trust.

use std::collections::{BTreeMap, BTreeSet};

use fe_runtime::relay_advertisement::RelayAdvertisementRow;
use iroh::{EndpointAddr, RelayUrl, TransportAddr};

/// How a relay URL may be used by this peer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdoptionMode {
    /// Dialled only when a peer address names it; never our home relay.
    DialOnly,
    /// Advertised and healthy in every participating verse; may become home.
    HomeEligible,
    /// Operator-pinned; always allowed.
    Pinned,
}

/// URLs advertised (admitted and healthy) in EVERY participating verse; no verses yields none.
pub fn evaluate_coverage(
    participating_verses: &BTreeSet<String>,
    healthy_admitted: &BTreeMap<String, BTreeSet<String>>,
) -> BTreeSet<String> {
    let mut verses = participating_verses.iter();
    let Some(first) = verses.next() else {
        return BTreeSet::new();
    };
    let mut covered = healthy_admitted.get(first).cloned().unwrap_or_default();
    for verse in verses {
        match healthy_admitted.get(verse) {
            Some(urls) => covered.retain(|url| urls.contains(url)),
            None => return BTreeSet::new(),
        }
    }
    covered
}

/// Keep at most `max_per_verse` adverts, oldest issued first; the rest are refused (deterministic).
pub fn select_bounded(
    adverts: Vec<RelayAdvertisementRow>,
    max_per_verse: usize,
) -> (Vec<RelayAdvertisementRow>, Vec<RelayAdvertisementRow>) {
    let mut ordered = adverts;
    ordered.sort_by(|left, right| {
        (left.issued_at_ms.unwrap_or(0), &left.url, &left.advertised_by).cmp(&(
            right.issued_at_ms.unwrap_or(0),
            &right.url,
            &right.advertised_by,
        ))
    });
    let refused = if ordered.len() > max_per_verse {
        ordered.split_off(max_per_verse)
    } else {
        Vec::new()
    };
    (ordered, refused)
}

/// Strip every relay URL outside the allowlist; direct IP addresses are untouched.
pub fn filter_peer_addr(addr: EndpointAddr, allowed_relays: &BTreeSet<RelayUrl>) -> EndpointAddr {
    let addrs = addr
        .addrs
        .into_iter()
        .filter(|transport| match transport {
            TransportAddr::Relay(url) => allowed_relays.contains(url),
            _ => true,
        })
        .collect();
    EndpointAddr { id: addr.id, addrs }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|item| item.to_string()).collect()
    }

    fn advert(url: &str, issued: u64) -> RelayAdvertisementRow {
        RelayAdvertisementRow {
            v: 1,
            url: url.to_string(),
            advertised_by: "did:key:manager".to_string(),
            note: None,
            verse_id: Some("verse-a".to_string()),
            expires_at_ms: None,
            issued_at_ms: Some(issued),
            dialback: None,
            sig: None,
        }
    }

    #[test]
    fn relay_trust_coverage_bound_and_filter_journey() {
        // Coverage table: (participating verses, per-verse healthy urls, expected covered).
        let table: Vec<(&[&str], Vec<(&str, &[&str])>, BTreeSet<String>)> = vec![
            (&[], vec![("a", &["r1"])], set(&[])),
            (&["a"], vec![("a", &["r1", "r2"])], set(&["r1", "r2"])),
            // Joining a verse without the advertisement breaks coverage.
            (&["a", "b"], vec![("a", &["r1"])], set(&[])),
            (&["a", "b"], vec![("a", &["r1", "r2"]), ("b", &["r2"])], set(&["r2"])),
            // Same url in two verses survives one withdrawal from a third-party view.
            (&["a", "b"], vec![("a", &["r1"]), ("b", &["r1"])], set(&["r1"])),
            (&["a", "b"], vec![("a", &["r1"]), ("b", &[])], set(&[])),
        ];
        for (verses, healthy, expected) in table {
            let participating = verses.iter().map(|verse| verse.to_string()).collect();
            let book = healthy
                .into_iter()
                .map(|(verse, urls)| (verse.to_string(), set(urls)))
                .collect();
            assert_eq!(evaluate_coverage(&participating, &book), expected);
        }

        // Bounded selection keeps the oldest issued and refuses the rest.
        let adverts = vec![advert("r3", 30), advert("r1", 10), advert("r2", 20)];
        let (kept, refused) = select_bounded(adverts, 2);
        assert_eq!(kept.iter().map(|row| row.url.as_str()).collect::<Vec<_>>(), ["r1", "r2"]);
        assert_eq!(refused.iter().map(|row| row.url.as_str()).collect::<Vec<_>>(), ["r3"]);

        // Peer-address filter strips unallowlisted relays and keeps direct addresses.
        let id = iroh::SecretKey::from_bytes(&[7u8; 32]).public();
        let allowed_url: RelayUrl = "https://allowed.example/".parse().unwrap();
        let denied_url: RelayUrl = "https://denied.example/".parse().unwrap();
        let direct: std::net::SocketAddr = "127.0.0.1:4000".parse().unwrap();
        let addr = EndpointAddr {
            id,
            addrs: [
                TransportAddr::Relay(allowed_url.clone()),
                TransportAddr::Relay(denied_url),
                TransportAddr::Ip(direct),
            ]
            .into_iter()
            .collect(),
        };
        let filtered = filter_peer_addr(addr, &[allowed_url.clone()].into_iter().collect());
        assert_eq!(filtered.addrs.len(), 2);
        assert!(filtered.addrs.contains(&TransportAddr::Relay(allowed_url)));
        assert!(filtered.addrs.contains(&TransportAddr::Ip(direct)));
    }
}
