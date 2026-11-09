//! MQTT client sessions: one per declared `mqtt` ingress (subscribe) or egress (publish) endpoint.

use std::time::Duration;

use fe_sdk::manifest_v2::{AuthScheme, AuthSlot, MqttEgress, MqttIngress, Qos};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, watch};

use super::connect::{self, Stream, CONNECT_TIMEOUT};
use super::mqtt::{decode, encode, topic_matches, Decoded, Packet};
use super::{backoff_ms, host_rate, host_size, until_stopped, EndpointState, IngressFrame, Net, ReplyTo, Transport, HOST_RATE_CEILING_HZ};
use crate::bus::rate::TokenBucket;
use crate::bus::{InstanceKey, InstanceSpec};

const KEEP_ALIVE_S: u16 = 30;
const RECONNECT_MIN_MS: u64 = 1_000;
const RECONNECT_MAX_MS: u64 = 60_000;
/// Fixed header, topic length prefix, the longest topic and a packet id, on top of the payload cap.
const PACKET_OVERHEAD: usize = 5 + 2 + 65_535 + 2;
/// A publisher only ever reads acknowledgements and pings.
const PUBLISHER_MAX_PACKET: usize = 1_024;
const READ_CHUNK: usize = 16 * 1024;

/// One queued `egress.mqtt-publish`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MqttPublish {
    pub(crate) topic: String,
    pub(crate) payload: Vec<u8>,
    pub(crate) qos: Qos,
    pub(crate) retain: bool,
}

enum Role {
    Subscribe { topics: Vec<String>, qos: u8, max_payload: usize, rate_hz: f64 },
    Publish,
}

/// An `mqtt` endpoint as the session needs it.
pub(super) struct MqttConfig {
    endpoint: String,
    broker: String,
    client_id: String,
    auth: Option<AuthSlot>,
    role: Role,
}

impl MqttConfig {
    /// A subscriber for an ingress endpoint, host ceilings applied (net/AGENTS.md §host-ceilings).
    pub(super) fn ingress(spec: &InstanceSpec, mqtt: &MqttIngress) -> Self {
        Self {
            endpoint: mqtt.id.clone(),
            broker: mqtt.broker.clone(),
            client_id: client_id(spec, &mqtt.id),
            auth: mqtt.auth.clone(),
            role: Role::Subscribe {
                topics: mqtt.topics.clone(),
                qos: qos_level(mqtt.qos),
                max_payload: host_size(mqtt.max_payload_bytes),
                rate_hz: host_rate(mqtt.rate_hz, HOST_RATE_CEILING_HZ),
            },
        }
    }

    /// A publisher for an egress endpoint.
    pub(super) fn egress(spec: &InstanceSpec, mqtt: &MqttEgress) -> Self {
        Self { endpoint: mqtt.id.clone(), broker: mqtt.broker.clone(), client_id: client_id(spec, &mqtt.id), auth: mqtt.auth.clone(), role: Role::Publish }
    }
}

/// Stable per installation and endpoint, 22 characters (inside MQTT 3.1.1's guaranteed 23).
fn client_id(spec: &InstanceSpec, endpoint: &str) -> String {
    let digest = blake3::hash(format!("{}\n{endpoint}", spec.installation_id).as_bytes());
    format!("fe{}", &digest.to_hex().as_str()[..20])
}

fn qos_level(qos: Option<Qos>) -> u8 {
    match qos {
        Some(Qos::AtLeastOnce) => 1,
        _ => 0,
    }
}

enum Ended {
    Stopped,
    Dropped(String),
}

/// Connect, serve, reconnect with jittered backoff until the instance stops.
pub(super) async fn run(
    net: Net,
    key: InstanceKey,
    generation: u64,
    config: MqttConfig,
    mut publishes: Option<mpsc::Receiver<MqttPublish>>,
    mut stop: watch::Receiver<bool>,
) {
    let mut attempt = 0u32;
    loop {
        if *stop.borrow() {
            return;
        }
        // Re-read on every connect, so an operator filling the slot needs no restart; slots are this package's only.
        match net.credential(&key.package_id, config.auth.as_ref()) {
            Err(state) => net.set_endpoint_state(&key, generation, &config.endpoint, state),
            Ok(Some((scheme, _, _))) if scheme != AuthScheme::MqttPassword => {
                net.set_endpoint_state(&key, generation, &config.endpoint, EndpointState::CredentialMissing);
            }
            Ok(credential) => {
                let (username, password) = match credential {
                    Some((_, credential, _)) => (credential.username, Some(credential.secret.into_bytes())),
                    None => (None, None),
                };
                let session = Session { net: &net, key: &key, generation, config: &config };
                match session.serve(username, password, publishes.as_mut(), &mut stop, &mut attempt).await {
                    Ended::Stopped => return,
                    Ended::Dropped(reason) => net.set_endpoint_state(&key, generation, &config.endpoint, EndpointState::Down(reason)),
                }
            }
        }
        attempt = attempt.saturating_add(1);
        let wait = backoff_ms(attempt, RECONNECT_MIN_MS, RECONNECT_MAX_MS);
        if until_stopped(&mut stop, tokio::time::sleep(Duration::from_millis(wait))).await.is_none() {
            return;
        }
    }
}

struct Session<'a> {
    net: &'a Net,
    key: &'a InstanceKey,
    generation: u64,
    config: &'a MqttConfig,
}

impl Session<'_> {
    async fn serve(
        &self,
        username: Option<String>,
        password: Option<Vec<u8>>,
        mut publishes: Option<&mut mpsc::Receiver<MqttPublish>>,
        stop: &mut watch::Receiver<bool>,
        attempt: &mut u32,
    ) -> Ended {
        let target = match connect::target(&self.config.broker) {
            Ok(target) => target,
            Err(error) => return Ended::Dropped(error),
        };
        let mut stream = match until_stopped(stop, connect::connect(&target)).await {
            None => return Ended::Stopped,
            Some(Ok(stream)) => stream,
            Some(Err(error)) => return Ended::Dropped(error),
        };
        let max_packet = match &self.config.role {
            Role::Subscribe { max_payload, .. } => max_payload.saturating_add(PACKET_OVERHEAD),
            Role::Publish => PUBLISHER_MAX_PACKET,
        };
        let mut reader = Reader::new(max_packet);
        let hello = Packet::Connect { client_id: self.config.client_id.clone(), keep_alive_s: KEEP_ALIVE_S, username, password };
        if let Err(error) = write(&mut stream, &hello).await {
            return Ended::Dropped(error);
        }
        let connack = match until_stopped(stop, tokio::time::timeout(CONNECT_TIMEOUT, reader.next(&mut stream))).await {
            None => return Ended::Stopped,
            Some(read) => read,
        };
        match connack {
            Ok(Ok(Packet::ConnAck { return_code: 0, .. })) => {}
            Ok(Ok(Packet::ConnAck { return_code, .. })) => return Ended::Dropped(format!("broker refused the connection (code {return_code})")),
            Ok(Ok(_)) => return Ended::Dropped("expected CONNACK".into()),
            Ok(Err(error)) => return Ended::Dropped(error),
            Err(_) => return Ended::Dropped("CONNACK timed out".into()),
        }
        let mut bucket = None;
        if let Role::Subscribe { topics, qos, rate_hz, .. } = &self.config.role {
            let subscribe = Packet::Subscribe { packet_id: 1, filters: topics.iter().map(|topic| (topic.clone(), *qos)).collect() };
            if let Err(error) = write(&mut stream, &subscribe).await {
                return Ended::Dropped(error);
            }
            let suback = match until_stopped(stop, tokio::time::timeout(CONNECT_TIMEOUT, reader.next(&mut stream))).await {
                None => return Ended::Stopped,
                Some(read) => read,
            };
            match suback {
                Ok(Ok(Packet::SubAck { return_codes, .. })) if return_codes.iter().all(|code| *code != 0x80) => {}
                Ok(Ok(Packet::SubAck { .. })) => return Ended::Dropped("the broker refused a topic filter".into()),
                Ok(Ok(_)) => return Ended::Dropped("expected SUBACK".into()),
                Ok(Err(error)) => return Ended::Dropped(error),
                Err(_) => return Ended::Dropped("SUBACK timed out".into()),
            }
            bucket = Some(TokenBucket::new(*rate_hz, self.net.now()));
        }
        self.net.set_endpoint_state(self.key, self.generation, &self.config.endpoint, EndpointState::Up);
        *attempt = 0;
        let period = Duration::from_secs(u64::from(KEEP_ALIVE_S / 2));
        let mut keep_alive = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        let mut unanswered_pings = 0u8;
        let mut next_packet_id: u16 = 2;
        loop {
            tokio::select! {
                _ = stop.changed() => {
                    let _ = write(&mut stream, &Packet::Disconnect).await;
                    return Ended::Stopped;
                }
                packet = reader.next(&mut stream) => {
                    let packet = match packet {
                        Ok(packet) => packet,
                        Err(error) => return Ended::Dropped(error),
                    };
                    self.flush_oversized(&mut reader);
                    match packet {
                        Packet::Publish { topic, packet_id, payload, .. } => {
                            if let Some(packet_id) = packet_id {
                                if let Err(error) = write(&mut stream, &Packet::PubAck { packet_id }).await {
                                    return Ended::Dropped(error);
                                }
                            }
                            if let Some(bucket) = bucket.as_mut() {
                                self.deliver(topic, payload, bucket);
                            }
                        }
                        Packet::PingResp => unanswered_pings = 0,
                        _ => {}
                    }
                }
                publish = next_publish(&mut publishes) => {
                    let Some(publish) = publish else { return Ended::Stopped };
                    let qos = qos_level(Some(publish.qos));
                    let packet_id = (qos == 1).then_some(next_packet_id);
                    if qos == 1 {
                        next_packet_id = if next_packet_id == u16::MAX { 2 } else { next_packet_id + 1 };
                    }
                    let packet = Packet::Publish { topic: publish.topic, packet_id, qos, retain: publish.retain, dup: false, payload: publish.payload };
                    if let Err(error) = write(&mut stream, &packet).await {
                        return Ended::Dropped(error);
                    }
                }
                _ = keep_alive.tick() => {
                    if unanswered_pings >= 2 {
                        return Ended::Dropped("keep-alive timed out".into());
                    }
                    unanswered_pings += 1;
                    if let Err(error) = write(&mut stream, &Packet::PingReq).await {
                        return Ended::Dropped(error);
                    }
                }
            }
        }
    }

    /// A message on a subscribed topic: size and rate checked, then queued for `on-ingress` (replies are ignored).
    fn deliver(&self, topic: String, payload: Vec<u8>, bucket: &mut TokenBucket) {
        let Role::Subscribe { topics, max_payload, .. } = &self.config.role else { return };
        if !topics.iter().any(|filter| topic_matches(filter, &topic)) {
            return;
        }
        if payload.len() > *max_payload {
            self.net.count(self.key, self.generation, |counters| counters.ingress_dropped_size += 1);
            return;
        }
        if !bucket.try_take(self.net.now()) {
            self.net.count(self.key, self.generation, |counters| counters.ingress_dropped_rate += 1);
            return;
        }
        let frame = IngressFrame {
            endpoint: self.config.endpoint.clone(),
            transport: Transport::Mqtt,
            connection: None,
            caller_did: None,
            method: None,
            path: None,
            query: Vec::new(),
            headers: Vec::new(),
            topic: Some(topic),
            content_type: None,
            body: payload,
            received_at_ms: self.net.now(),
        };
        self.net.deliver_frame(self.key, self.generation, frame, ReplyTo::Nowhere);
    }

    fn flush_oversized(&self, reader: &mut Reader) {
        let skipped = reader.take_oversized();
        if skipped > 0 {
            self.net.count(self.key, self.generation, |counters| counters.ingress_dropped_size += skipped);
        }
    }
}

/// The publish queue for publishers; never ready for subscribers.
async fn next_publish(publishes: &mut Option<&mut mpsc::Receiver<MqttPublish>>) -> Option<MqttPublish> {
    match publishes {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

async fn write(stream: &mut Stream, packet: &Packet) -> Result<(), String> {
    let bytes = encode(packet).map_err(|error| error.to_string())?;
    tokio::time::timeout(CONNECT_TIMEOUT, stream.write_all(&bytes))
        .await
        .map_err(|_| "write timed out".to_string())?
        .map_err(|error| error.to_string())
}

/// Buffers bytes until a whole packet decodes; oversized packets are skipped as they stream past, never buffered,
/// so the buffer stays under `max_packet + READ_CHUNK` whatever the broker announces.
pub(super) struct Reader {
    buffer: Vec<u8>,
    chunk: Vec<u8>,
    skip: usize,
    max_packet: usize,
    oversized: u64,
}

impl Reader {
    /// A reader that refuses (skips) any packet larger than `max_packet` bytes.
    pub(super) fn new(max_packet: usize) -> Self {
        Self { buffer: Vec::new(), chunk: vec![0; READ_CHUNK], skip: 0, max_packet, oversized: 0 }
    }

    /// Oversized packets skipped since the last call.
    pub(super) fn take_oversized(&mut self) -> u64 {
        std::mem::take(&mut self.oversized)
    }

    /// Bytes currently held (never more than `max_packet + READ_CHUNK`).
    #[cfg(test)]
    pub(super) fn buffered(&self) -> usize {
        self.buffer.len()
    }

    /// Cancel-safe: the only await is the read, and its bytes are appended before the next await.
    pub(super) async fn next(&mut self, stream: &mut Stream) -> Result<Packet, String> {
        loop {
            match decode(&self.buffer, self.max_packet).map_err(|error| error.to_string())? {
                Decoded::Packet(packet, used) => {
                    self.buffer.drain(..used);
                    return Ok(packet);
                }
                Decoded::Oversized { total } => {
                    self.oversized += 1;
                    if self.buffer.len() >= total {
                        self.buffer.drain(..total);
                    } else {
                        self.skip = total - self.buffer.len();
                        self.buffer.clear();
                    }
                    continue;
                }
                Decoded::Incomplete => {}
            }
            let read = stream.read(&mut self.chunk).await.map_err(|error| error.to_string())?;
            if read == 0 {
                return Err("closed by the broker".into());
            }
            let mut fresh = &self.chunk[..read];
            if self.skip > 0 {
                let skipped = self.skip.min(fresh.len());
                self.skip -= skipped;
                fresh = &fresh[skipped..];
            }
            self.buffer.extend_from_slice(fresh);
        }
    }
}
