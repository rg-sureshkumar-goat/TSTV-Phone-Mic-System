//! Wire format shared with the phone apps. The spec is shared/PROTOCOL.md.

use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const CONTROL_PORT: u16 = 47800;
pub const AUDIO_PORT: u16 = 47801;
pub const SERVICE_TYPE: &str = "_tstvmic._tcp.local.";

pub const SAMPLE_RATE: u32 = 48_000;
/// 10 ms at 48 kHz.
pub const FRAME_SAMPLES: usize = 480;
pub const DEFAULT_BITRATE: u32 = 128_000;

const MAGIC: [u8; 2] = *b"TV";
const VERSION: u8 = 1;
pub const KIND_AUDIO: u8 = 1;
pub const KIND_PING: u8 = 2;
pub const KIND_PONG: u8 = 3;
pub const CODEC_OPUS: u8 = 1;
pub const FLAG_TALENT_MUTED: u8 = 1;

/// Largest datagram we accept.
pub const MAX_DATAGRAM: usize = 1500;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AudioPacket<'a> {
    pub token: u32,
    pub seq: u32,
    pub flags: u8,
    pub codec: u8,
    pub primary: &'a [u8],
    pub redundant: &'a [u8],
}

impl AudioPacket<'_> {
    pub fn talent_muted(&self) -> bool {
        self.flags & FLAG_TALENT_MUTED != 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Datagram<'a> {
    Audio(AudioPacket<'a>),
    Ping { token: u32, stamp: u64 },
    Pong { token: u32, stamp: u64 },
}

pub fn parse(buf: &[u8]) -> Option<Datagram<'_>> {
    if buf.len() < 8 || buf[0..2] != MAGIC || buf[2] != VERSION {
        return None;
    }
    let token = u32::from_be_bytes(buf[4..8].try_into().unwrap());
    match buf[3] {
        KIND_AUDIO => {
            if buf.len() < 16 {
                return None;
            }
            let seq = u32::from_be_bytes(buf[8..12].try_into().unwrap());
            let flags = buf[12];
            let codec = buf[13];
            let n = u16::from_be_bytes([buf[14], buf[15]]) as usize;
            let primary = buf.get(16..16 + n)?;
            let rest = &buf[16 + n..];
            let redundant = if rest.len() >= 2 {
                let m = u16::from_be_bytes([rest[0], rest[1]]) as usize;
                rest.get(2..2 + m)?
            } else {
                &[]
            };
            Some(Datagram::Audio(AudioPacket { token, seq, flags, codec, primary, redundant }))
        }
        KIND_PING | KIND_PONG => {
            let stamp = u64::from_be_bytes(buf.get(8..16)?.try_into().unwrap());
            Some(if buf[3] == KIND_PING { Datagram::Ping { token, stamp } } else { Datagram::Pong { token, stamp } })
        }
        _ => None,
    }
}

fn write_header(out: &mut Vec<u8>, kind: u8, token: u32) {
    out.extend_from_slice(&MAGIC);
    out.push(VERSION);
    out.push(kind);
    out.extend_from_slice(&token.to_be_bytes());
}

pub fn write_audio(out: &mut Vec<u8>, token: u32, seq: u32, flags: u8, primary: &[u8], redundant: &[u8]) {
    out.clear();
    write_header(out, KIND_AUDIO, token);
    out.extend_from_slice(&seq.to_be_bytes());
    out.push(flags);
    out.push(CODEC_OPUS);
    out.extend_from_slice(&(primary.len() as u16).to_be_bytes());
    out.extend_from_slice(primary);
    out.extend_from_slice(&(redundant.len() as u16).to_be_bytes());
    out.extend_from_slice(redundant);
}

pub fn write_ping(kind: u8, token: u32, stamp: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(16);
    write_header(&mut out, kind, token);
    out.extend_from_slice(&stamp.to_be_bytes());
    out
}

// ---- Control channel (JSON over WebSocket) ----

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InputInfo {
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum PhoneMessage {
    Hello {
        protocol: u32,
        device_id: String,
        name: String,
        #[serde(default)]
        platform: String,
        #[serde(default)]
        model: String,
        #[serde(default)]
        app_version: String,
    },
    Status {
        #[serde(default)]
        talent_muted: bool,
        #[serde(default)]
        input: Option<InputInfo>,
        #[serde(default)]
        capture_latency_ms: Option<f64>,
        #[serde(default)]
        input_sample_rate: Option<f64>,
        #[serde(default)]
        battery: Option<f64>,
        #[serde(default)]
        interrupted: bool,
    },
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum DesktopMessage {
    Welcome {
        protocol: u32,
        slot: u32,
        channel: u32,
        token: u32,
        udp_port: u16,
        bitrate: u32,
        desktop_name: String,
    },
    Reject {
        reason: String,
        message: String,
    },
    Operator {
        operator_muted: bool,
    },
    Stats {
        latency_ms: f64,
        loss_pct: f64,
        channel: u32,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_round_trip() {
        let mut buf = Vec::new();
        write_audio(&mut buf, 0xDEADBEEF, 42, FLAG_TALENT_MUTED, &[1, 2, 3], &[9, 8]);
        let Some(Datagram::Audio(p)) = parse(&buf) else { panic!("not audio") };
        assert_eq!(p.token, 0xDEADBEEF);
        assert_eq!(p.seq, 42);
        assert!(p.talent_muted());
        assert_eq!(p.codec, CODEC_OPUS);
        assert_eq!(p.primary, &[1, 2, 3]);
        assert_eq!(p.redundant, &[9, 8]);
    }

    #[test]
    fn rejects_garbage_and_truncation() {
        assert_eq!(parse(b"hello world, not a packet"), None);
        let mut buf = Vec::new();
        write_audio(&mut buf, 1, 1, 0, &[1, 2, 3, 4], &[]);
        assert_eq!(parse(&buf[..buf.len() - 4]), None);
    }

    #[test]
    fn ping_round_trip() {
        let buf = write_ping(KIND_PONG, 7, 123456789);
        assert_eq!(parse(&buf), Some(Datagram::Pong { token: 7, stamp: 123456789 }));
    }

    #[test]
    fn json_messages() {
        let hello: PhoneMessage = serde_json::from_str(
            r#"{"type":"hello","protocol":1,"deviceId":"abc","name":"Alex","platform":"ios","extra":true}"#,
        )
        .unwrap();
        assert!(matches!(hello, PhoneMessage::Hello { ref device_id, .. } if device_id == "abc"));
        let other: PhoneMessage = serde_json::from_str(r#"{"type":"somethingNew"}"#).unwrap();
        assert_eq!(other, PhoneMessage::Unknown);

        let json = serde_json::to_string(&DesktopMessage::Operator { operator_muted: true }).unwrap();
        assert_eq!(json, r#"{"type":"operator","operatorMuted":true}"#);
    }
}
