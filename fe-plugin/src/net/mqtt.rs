//! Minimal MQTT 3.1.1 codec (QoS 0/1, the subset a host-side client needs) and topic-filter matching.
//! Spec: <https://docs.oasis-open.org/mqtt/mqtt/v3.1.1/os/mqtt-v3.1.1-os.html>; rationale in net/AGENTS.md §mqtt.

/// The largest remaining length MQTT can encode.
pub const MAX_REMAINING_LENGTH: usize = 268_435_455;

/// The packets the host sends or understands; `Debug` redacts the CONNECT password and prints only payload sizes.
#[derive(Clone, PartialEq, Eq)]
pub enum Packet {
    Connect { client_id: String, keep_alive_s: u16, username: Option<String>, password: Option<Vec<u8>> },
    ConnAck { session_present: bool, return_code: u8 },
    Publish { topic: String, packet_id: Option<u16>, qos: u8, retain: bool, dup: bool, payload: Vec<u8> },
    PubAck { packet_id: u16 },
    Subscribe { packet_id: u16, filters: Vec<(String, u8)> },
    SubAck { packet_id: u16, return_codes: Vec<u8> },
    PingReq,
    PingResp,
    Disconnect,
}

impl std::fmt::Debug for Packet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Connect { client_id, keep_alive_s, username, password } => f
                .debug_struct("Connect")
                .field("client_id", client_id)
                .field("keep_alive_s", keep_alive_s)
                .field("username", username)
                .field("password", &password.as_ref().map(|_| "<redacted>"))
                .finish(),
            Self::ConnAck { session_present, return_code } => {
                f.debug_struct("ConnAck").field("session_present", session_present).field("return_code", return_code).finish()
            }
            Self::Publish { topic, packet_id, qos, retain, dup, payload } => f
                .debug_struct("Publish")
                .field("topic", topic)
                .field("packet_id", packet_id)
                .field("qos", qos)
                .field("retain", retain)
                .field("dup", dup)
                .field("payload_bytes", &payload.len())
                .finish(),
            Self::PubAck { packet_id } => f.debug_struct("PubAck").field("packet_id", packet_id).finish(),
            Self::Subscribe { packet_id, filters } => {
                f.debug_struct("Subscribe").field("packet_id", packet_id).field("filters", filters).finish()
            }
            Self::SubAck { packet_id, return_codes } => {
                f.debug_struct("SubAck").field("packet_id", packet_id).field("return_codes", return_codes).finish()
            }
            Self::PingReq => f.write_str("PingReq"),
            Self::PingResp => f.write_str("PingResp"),
            Self::Disconnect => f.write_str("Disconnect"),
        }
    }
}

/// What the front of a read buffer holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decoded {
    /// A whole packet and the bytes it used.
    Packet(Packet, usize),
    /// A packet whose total size exceeds the cap; skip `total` bytes without buffering them.
    Oversized { total: usize },
    /// Read more.
    Incomplete,
}

/// A packet the codec cannot accept; the session drops the connection.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MqttError {
    #[error("malformed remaining length")]
    Length,
    #[error("malformed packet: {0}")]
    Malformed(&'static str),
    #[error("unsupported packet type {0}")]
    Unsupported(u8),
}

/// Encode one packet.
pub fn encode(packet: &Packet) -> Result<Vec<u8>, MqttError> {
    let mut body = Vec::new();
    let header = match packet {
        Packet::Connect { client_id, keep_alive_s, username, password } => {
            if password.is_some() && username.is_none() {
                return Err(MqttError::Malformed("a password needs a username"));
            }
            put_bytes(&mut body, b"MQTT")?;
            body.push(4);
            let mut flags = 0x02u8; // clean session
            if username.is_some() {
                flags |= 0x80;
            }
            if password.is_some() {
                flags |= 0x40;
            }
            body.push(flags);
            body.extend_from_slice(&keep_alive_s.to_be_bytes());
            put_bytes(&mut body, client_id.as_bytes())?;
            if let Some(username) = username {
                put_bytes(&mut body, username.as_bytes())?;
            }
            if let Some(password) = password {
                put_bytes(&mut body, password)?;
            }
            0x10
        }
        Packet::ConnAck { session_present, return_code } => {
            body.push(u8::from(*session_present));
            body.push(*return_code);
            0x20
        }
        Packet::Publish { topic, packet_id, qos, retain, dup, payload } => {
            put_bytes(&mut body, topic.as_bytes())?;
            match (*qos, *packet_id) {
                (0, None) => {}
                (1, Some(id)) if id != 0 => body.extend_from_slice(&id.to_be_bytes()),
                _ => return Err(MqttError::Malformed("QoS 0 carries no packet id, QoS 1 a non-zero one, QoS 2 is unsupported")),
            }
            body.extend_from_slice(payload);
            0x30 | (u8::from(*dup) << 3) | (*qos << 1) | u8::from(*retain)
        }
        Packet::PubAck { packet_id } => {
            body.extend_from_slice(&packet_id.to_be_bytes());
            0x40
        }
        Packet::Subscribe { packet_id, filters } => {
            if filters.is_empty() {
                return Err(MqttError::Malformed("SUBSCRIBE needs a filter"));
            }
            body.extend_from_slice(&packet_id.to_be_bytes());
            for (filter, qos) in filters {
                put_bytes(&mut body, filter.as_bytes())?;
                body.push(*qos);
            }
            0x82
        }
        Packet::SubAck { packet_id, return_codes } => {
            body.extend_from_slice(&packet_id.to_be_bytes());
            body.extend_from_slice(return_codes);
            0x90
        }
        Packet::PingReq => 0xC0,
        Packet::PingResp => 0xD0,
        Packet::Disconnect => 0xE0,
    };
    if body.len() > MAX_REMAINING_LENGTH {
        return Err(MqttError::Length);
    }
    let mut out = Vec::with_capacity(body.len() + 5);
    out.push(header);
    put_length(&mut out, body.len());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Decode the packet at the front of `buffer`; packets larger than `max_packet` are reported, never buffered.
pub fn decode(buffer: &[u8], max_packet: usize) -> Result<Decoded, MqttError> {
    if buffer.len() < 2 {
        return Ok(Decoded::Incomplete);
    }
    let Some((remaining, length_bytes)) = read_length(&buffer[1..])? else {
        return Ok(Decoded::Incomplete);
    };
    let total = 1 + length_bytes + remaining;
    if total > max_packet {
        return Ok(Decoded::Oversized { total });
    }
    if buffer.len() < total {
        return Ok(Decoded::Incomplete);
    }
    let packet = parse(buffer[0], &buffer[1 + length_bytes..total])?;
    Ok(Decoded::Packet(packet, total))
}

/// MQTT topic-filter matching: `+` is one level, a trailing `#` any number (including none);
/// wildcards at the first level never match `$`-topics.
pub fn topic_matches(filter: &str, topic: &str) -> bool {
    if topic.starts_with('$') && (filter.starts_with('+') || filter.starts_with('#')) {
        return false;
    }
    let mut filter_levels = filter.split('/');
    let mut topic_levels = topic.split('/');
    loop {
        match (filter_levels.next(), topic_levels.next()) {
            (Some("#"), _) => return true,
            (Some("+"), Some(_)) => {}
            (Some(expected), Some(actual)) if expected == actual => {}
            (None, None) => return true,
            _ => return false,
        }
    }
}

/// A publishable topic: non-empty, at most 65 535 bytes, no wildcard and no NUL.
pub fn valid_topic_name(topic: &str) -> bool {
    !topic.is_empty() && topic.len() <= usize::from(u16::MAX) && !topic.contains(['+', '#', '\0'])
}

/// A subscribable filter: `#` only as the whole last level, `+` only as a whole level.
pub fn valid_topic_filter(filter: &str) -> bool {
    if filter.is_empty() || filter.len() > usize::from(u16::MAX) || filter.contains('\0') {
        return false;
    }
    let levels: Vec<&str> = filter.split('/').collect();
    levels.iter().enumerate().all(|(index, level)| match *level {
        "#" => index + 1 == levels.len(),
        "+" => true,
        other => !other.contains(['+', '#']),
    })
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), MqttError> {
    let length = u16::try_from(bytes.len()).map_err(|_| MqttError::Malformed("a field is over 65535 bytes"))?;
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

fn put_length(out: &mut Vec<u8>, mut length: usize) {
    loop {
        let mut byte = (length % 128) as u8;
        length /= 128;
        if length > 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if length == 0 {
            break;
        }
    }
}

/// The remaining length and how many bytes encoded it; `None` while incomplete.
fn read_length(bytes: &[u8]) -> Result<Option<(usize, usize)>, MqttError> {
    let mut value = 0usize;
    for (index, byte) in bytes.iter().take(4).enumerate() {
        value += usize::from(byte & 0x7f) << (7 * index);
        if byte & 0x80 == 0 {
            return Ok(Some((value, index + 1)));
        }
    }
    if bytes.len() >= 4 {
        Err(MqttError::Length)
    } else {
        Ok(None)
    }
}

struct Cursor<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Cursor<'a> {
    fn u8(&mut self) -> Result<u8, MqttError> {
        let byte = *self.bytes.get(self.at).ok_or(MqttError::Malformed("truncated"))?;
        self.at += 1;
        Ok(byte)
    }

    fn u16(&mut self) -> Result<u16, MqttError> {
        let high = self.u8()?;
        let low = self.u8()?;
        Ok(u16::from_be_bytes([high, low]))
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], MqttError> {
        let end = self.at.checked_add(length).filter(|end| *end <= self.bytes.len()).ok_or(MqttError::Malformed("truncated"))?;
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Ok(slice)
    }

    fn string(&mut self) -> Result<String, MqttError> {
        let length = usize::from(self.u16()?);
        let raw = self.take(length)?;
        String::from_utf8(raw.to_vec()).map_err(|_| MqttError::Malformed("a string is not UTF-8"))
    }

    fn binary(&mut self) -> Result<Vec<u8>, MqttError> {
        let length = usize::from(self.u16()?);
        Ok(self.take(length)?.to_vec())
    }

    fn rest(&mut self) -> &'a [u8] {
        let slice = &self.bytes[self.at..];
        self.at = self.bytes.len();
        slice
    }

    fn done(&self) -> bool {
        self.at == self.bytes.len()
    }
}

fn parse(header: u8, body: &[u8]) -> Result<Packet, MqttError> {
    let mut cursor = Cursor { bytes: body, at: 0 };
    let flags = header & 0x0f;
    let packet = match header >> 4 {
        1 => {
            let name = cursor.string()?;
            let level = cursor.u8()?;
            if name != "MQTT" || level != 4 {
                return Err(MqttError::Malformed("not MQTT 3.1.1"));
            }
            let connect_flags = cursor.u8()?;
            if connect_flags & 0x04 != 0 {
                return Err(MqttError::Malformed("will messages are not supported"));
            }
            let keep_alive_s = cursor.u16()?;
            let client_id = cursor.string()?;
            let username = if connect_flags & 0x80 != 0 { Some(cursor.string()?) } else { None };
            let password = if connect_flags & 0x40 != 0 { Some(cursor.binary()?) } else { None };
            Packet::Connect { client_id, keep_alive_s, username, password }
        }
        2 => {
            let session_present = (cursor.u8()? & 0x01) == 1;
            Packet::ConnAck { session_present, return_code: cursor.u8()? }
        }
        3 => {
            let qos = (flags >> 1) & 0x03;
            if qos > 1 {
                return Err(MqttError::Malformed("QoS 2 is not supported"));
            }
            let topic = cursor.string()?;
            let packet_id = if qos == 1 { Some(cursor.u16()?) } else { None };
            Packet::Publish { topic, packet_id, qos, retain: flags & 0x01 != 0, dup: flags & 0x08 != 0, payload: cursor.rest().to_vec() }
        }
        4 => Packet::PubAck { packet_id: cursor.u16()? },
        8 => {
            if flags != 0x02 {
                return Err(MqttError::Malformed("SUBSCRIBE flags must be 0010"));
            }
            let packet_id = cursor.u16()?;
            let mut filters = Vec::new();
            while !cursor.done() {
                let filter = cursor.string()?;
                filters.push((filter, cursor.u8()?));
            }
            if filters.is_empty() {
                return Err(MqttError::Malformed("SUBSCRIBE needs a filter"));
            }
            Packet::Subscribe { packet_id, filters }
        }
        9 => {
            let packet_id = cursor.u16()?;
            Packet::SubAck { packet_id, return_codes: cursor.rest().to_vec() }
        }
        12 => Packet::PingReq,
        13 => Packet::PingResp,
        14 => Packet::Disconnect,
        other => return Err(MqttError::Unsupported(other)),
    };
    if !cursor.done() {
        return Err(MqttError::Malformed("trailing bytes"));
    }
    Ok(packet)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every packet round-trips; oversized and partial buffers are reported, not parsed; filters match per spec.
    #[test]
    fn codec_round_trips_and_topic_filters_match() {
        let packets = [
            Packet::Connect { client_id: "fe0123".into(), keep_alive_s: 30, username: Some("site-a".into()), password: Some(b"pw".to_vec()) },
            Packet::Connect { client_id: "fe0123".into(), keep_alive_s: 30, username: None, password: None },
            Packet::ConnAck { session_present: false, return_code: 0 },
            Packet::Publish { topic: "site-a/d1/telemetry".into(), packet_id: Some(7), qos: 1, retain: false, dup: false, payload: vec![1; 300] },
            Packet::Publish { topic: "a".into(), packet_id: None, qos: 0, retain: true, dup: false, payload: Vec::new() },
            Packet::PubAck { packet_id: 7 },
            Packet::Subscribe { packet_id: 1, filters: vec![("site-a/+/telemetry".into(), 1), ("b/#".into(), 0)] },
            Packet::SubAck { packet_id: 1, return_codes: vec![1, 0x80] },
            Packet::PingReq,
            Packet::PingResp,
            Packet::Disconnect,
        ];
        for packet in packets {
            let bytes = encode(&packet).unwrap();
            assert_eq!(decode(&bytes, 1 << 20).unwrap(), Decoded::Packet(packet.clone(), bytes.len()), "{packet:?}");
            assert_eq!(decode(&bytes[..bytes.len() - 1], 1 << 20).unwrap(), Decoded::Incomplete, "{packet:?} partial");
        }
        let big = encode(&Packet::Publish { topic: "t".into(), packet_id: None, qos: 0, retain: false, dup: false, payload: vec![0; 200] }).unwrap();
        assert_eq!(decode(&big[..4], 64).unwrap(), Decoded::Oversized { total: big.len() }, "known from the header alone");
        assert!(encode(&Packet::Publish { topic: "t".into(), packet_id: Some(1), qos: 2, retain: false, dup: false, payload: vec![] }).is_err());
        assert_eq!(decode(&[0x30, 0xff, 0xff, 0xff, 0xff, 0x01], 1 << 20), Err(MqttError::Length));

        for (filter, topic, expected) in [
            ("site-a/+/telemetry", "site-a/d1/telemetry", true),
            ("site-a/+/telemetry", "site-a/d1/x/telemetry", false),
            ("sport/#", "sport", true),
            ("sport/#", "sport/tennis/player1", true),
            ("#", "$SYS/broker", false),
            ("+/broker", "$SYS/broker", false),
            ("$SYS/#", "$SYS/broker", true),
            ("a/+", "a/", true),
            ("a/b", "a/b/c", false),
        ] {
            assert_eq!(topic_matches(filter, topic), expected, "{filter} vs {topic}");
        }
        assert!(valid_topic_filter("a/+/b/#") && !valid_topic_filter("a/#/b") && !valid_topic_filter("a+/b"));
        assert!(valid_topic_name("site-a/d1/command") && !valid_topic_name("site-a/+/command") && !valid_topic_name(""));
    }
}
