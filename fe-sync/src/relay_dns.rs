//! Connect-time judgment of advertised relay host names (DNS rebinding guard); see `AGENTS.md` §relay-trust "connect-time judgment".

use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::pin::Pin;
use std::sync::{Arc, RwLock};

use fe_runtime::relay_advertisement::{check_resolved_relay_address, AdvertisedRelayPolicy, RelayReach};
use iroh::dns::{BoxIter, DnsError, DnsResolver, Resolver, TxtRecordData, DNS_TIMEOUT};

/// Most judged host names kept; current hosts always fit, withdrawn ones are dropped first.
pub const JUDGED_RELAY_HOST_CAP: usize = 1024;

/// Address rule applied to every resolution of an advertised relay's host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RelayHostRule {
    /// DialOnly: every address passes the process address policy (`dial_only_relay_allowed`).
    DialOnly,
    /// In the relay map (Pinned / HomeEligible): every address is public (`resolve_public_relay`).
    PublicOnly,
}

impl RelayHostRule {
    fn admits(self, address: IpAddr, policy: AdvertisedRelayPolicy) -> bool {
        match check_resolved_relay_address(address, policy) {
            Err(_) => false,
            Ok(reach) => self == Self::DialOnly || reach == RelayReach::Public,
        }
    }
}

/// Lower-case host without a trailing dot, the form both the sync loop and iroh's lookups are compared in.
pub fn normalize_host(host: &str) -> String {
    host.trim_end_matches('.').to_ascii_lowercase()
}

/// Judged host names and their rules: written by the sync loop, read by the endpoint's resolver on every lookup.
#[derive(Debug, Clone, Default)]
pub struct RelayHostJudge {
    rules: Arc<RwLock<JudgedHosts>>,
}

/// Each judged host's rule and the last update that named it current.
#[derive(Debug, Default)]
struct JudgedHosts {
    hosts: HashMap<String, (RelayHostRule, u64)>,
    generation: u64,
}

impl RelayHostJudge {
    /// Adopts this pass's hosts; withdrawn hosts stay judged (an open relay connection may redial
    /// them) and leave one at a time, the longest withdrawn first, only when the cap needs room.
    /// A current host is never evicted, and every host in the relay map is current: the sync
    /// loop names a planned removal current until it has left the map. `exempt` (configured and
    /// default relay hosts) are never judged.
    pub fn update(&self, current: HashMap<String, RelayHostRule>, exempt: &BTreeSet<String>) {
        let mut judged = self.rules.write().unwrap_or_else(|poisoned| poisoned.into_inner());
        judged.generation += 1;
        let generation = judged.generation;
        judged.hosts.retain(|host, _| !exempt.contains(host));
        for (host, rule) in current {
            if !exempt.contains(&host) {
                judged.hosts.insert(host, (rule, generation));
            }
        }
        let over = judged.hosts.len().saturating_sub(JUDGED_RELAY_HOST_CAP);
        if over > 0 {
            let mut withdrawn: Vec<(u64, String)> = judged
                .hosts
                .iter()
                .filter(|(_, (_, seen))| *seen < generation)
                .map(|(host, (_, seen))| (*seen, host.clone()))
                .collect();
            withdrawn.sort();
            for (_, host) in withdrawn.into_iter().take(over) {
                judged.hosts.remove(&host);
            }
        }
    }

    /// The rule for `host`, if it is an advertised relay host.
    pub fn rule(&self, host: &str) -> Option<RelayHostRule> {
        let judged = self.rules.read().unwrap_or_else(|poisoned| poisoned.into_inner());
        judged.hosts.get(&normalize_host(host)).map(|(rule, _)| *rule)
    }

    /// Hosts currently judged.
    pub fn len(&self) -> usize {
        self.rules.read().unwrap_or_else(|poisoned| poisoned.into_inner()).hosts.len()
    }

    /// True when no host is judged.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Decorator over the system resolver: a judged host's address family resolves to nothing when any address fails its rule.
#[derive(Debug, Clone)]
struct JudgingResolver {
    inner: DnsResolver,
    judge: RelayHostJudge,
}

type Lookup<T> = Pin<Box<dyn Future<Output = Result<BoxIter<T>, DnsError>> + Send + 'static>>;

/// The addresses a judged host may be dialled at: all of them, or none when one fails the rule.
fn judged<T: Copy>(host: &str, rule: Option<RelayHostRule>, addresses: Vec<T>, as_ip: impl Fn(T) -> IpAddr) -> Vec<T> {
    let Some(rule) = rule else {
        return addresses;
    };
    let policy = AdvertisedRelayPolicy::process();
    let refused = addresses.iter().copied().map(&as_ip).find(|address| !rule.admits(*address, policy));
    match refused {
        None => addresses,
        Some(refused) => {
            tracing::warn!(relay_host = %host, address = %refused, ?rule, "advertised relay host resolved to a refused address at connect time; not dialled");
            Vec::new()
        }
    }
}

impl Resolver for JudgingResolver {
    fn lookup_ipv4(&self, host: String) -> Lookup<Ipv4Addr> {
        let (inner, judge) = (self.inner.clone(), self.judge.clone());
        Box::pin(async move {
            let addresses: Vec<Ipv4Addr> = inner
                .lookup_ipv4(host.clone(), DNS_TIMEOUT)
                .await?
                .filter_map(|address| match address {
                    IpAddr::V4(address) => Some(address),
                    IpAddr::V6(_) => None,
                })
                .collect();
            let kept = judged(&host, judge.rule(&host), addresses, IpAddr::V4);
            Ok::<_, DnsError>(Box::new(kept.into_iter()) as BoxIter<Ipv4Addr>)
        })
    }

    fn lookup_ipv6(&self, host: String) -> Lookup<Ipv6Addr> {
        let (inner, judge) = (self.inner.clone(), self.judge.clone());
        Box::pin(async move {
            let addresses: Vec<Ipv6Addr> = inner
                .lookup_ipv6(host.clone(), DNS_TIMEOUT)
                .await?
                .filter_map(|address| match address {
                    IpAddr::V6(address) => Some(address),
                    IpAddr::V4(_) => None,
                })
                .collect();
            let kept = judged(&host, judge.rule(&host), addresses, IpAddr::V6);
            Ok::<_, DnsError>(Box::new(kept.into_iter()) as BoxIter<Ipv6Addr>)
        })
    }

    fn lookup_txt(&self, host: String) -> Lookup<TxtRecordData> {
        let inner = self.inner.clone();
        Box::pin(async move {
            let records: Vec<TxtRecordData> = inner.lookup_txt(host, DNS_TIMEOUT).await?.collect();
            Ok::<_, DnsError>(Box::new(records.into_iter()) as BoxIter<TxtRecordData>)
        })
    }

    fn clear_cache(&self) {
        self.inner.clear_cache();
    }

    fn reset(&self) -> Box<dyn Resolver> {
        self.inner.reset();
        Box::new(self.clone())
    }
}

/// The endpoint's resolver: `inner` answers, `judge` decides which answers an advertised relay host may use.
pub fn judging_resolver(inner: DnsResolver, judge: RelayHostJudge) -> DnsResolver {
    DnsResolver::custom(JudgingResolver { inner, judge })
}
