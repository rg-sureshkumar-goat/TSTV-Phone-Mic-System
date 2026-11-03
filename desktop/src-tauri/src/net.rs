//! Network side: the WebSocket control server, the UDP audio socket, round-trip pings and
//! Bonjour advertising. Runs on its own tokio runtime thread.

use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;

use crate::engine::{Engine, MAX_SLOTS, ToAudio};
use crate::protocol::{
    self, AUDIO_PORT, CONTROL_PORT, DEFAULT_BITRATE, Datagram, DesktopMessage, InputInfo, KIND_PING, MAX_DATAGRAM,
    PROTOCOL_VERSION, PhoneMessage, SERVICE_TYPE,
};

/// Latest `status` from a phone.
#[derive(Debug, Clone, Default)]
pub struct PhoneStatus {
    pub talent_muted: bool,
    pub input: Option<InputInfo>,
    pub capture_latency_ms: Option<f64>,
    pub battery: Option<f64>,
    pub interrupted: bool,
}

pub enum Outgoing {
    Msg(DesktopMessage),
    Close,
}

/// A connected phone.
pub struct Session {
    conn_id: u64,
    pub device_id: String,
    pub name: String,
    pub model: String,
    pub token: u32,
    pub udp_addr: Option<SocketAddr>,
    pub status: PhoneStatus,
    pub rtt_ms: Option<f64>,
    /// Packet arrival jitter (RFC 3550 style), ms.
    pub jitter_ms: f64,
    pub last_audio: Option<Instant>,
    last_arrival: Option<(Instant, u32)>,
    tx: mpsc::UnboundedSender<Outgoing>,
}

impl Session {
    pub fn send(&self, msg: DesktopMessage) {
        let _ = self.tx.send(Outgoing::Msg(msg));
    }
}

pub struct Net {
    pub sessions: Mutex<[Option<Session>; MAX_SLOTS]>,
    /// How many slots accept phones (Milestone 1: one).
    pub active_slots: AtomicUsize,
    pub desktop_name: String,
    pub listen_error: Mutex<Option<String>>,
    engine: Arc<Engine>,
    next_conn: AtomicU64,
    epoch: Instant,
}

impl Net {
    pub fn start(engine: Arc<Engine>, desktop_name: String, active_slots: usize) -> Arc<Net> {
        let net = Arc::new(Net {
            sessions: Mutex::new(std::array::from_fn(|_| None)),
            active_slots: AtomicUsize::new(active_slots),
            desktop_name,
            listen_error: Mutex::new(None),
            engine,
            next_conn: AtomicU64::new(1),
            epoch: Instant::now(),
        });
        let thread_net = net.clone();
        std::thread::Builder::new()
            .name("network".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                    .expect("tokio runtime");
                rt.block_on(run(thread_net));
            })
            .expect("spawn network thread");
        net
    }

    /// Tells the phone in `slot` (if any) about the operator's mute.
    pub fn notify_operator_mute(&self, slot: usize, muted: bool) {
        if let Some(s) = &self.sessions.lock().unwrap()[slot] {
            s.send(DesktopMessage::Operator { operator_muted: muted });
        }
    }

    fn now_ns(&self) -> u64 {
        self.epoch.elapsed().as_nanos() as u64
    }

    /// Gives a new phone a slot. Returns (slot index, token).
    fn assign(&self, conn_id: u64, device_id: &str, name: &str, model: &str, tx: mpsc::UnboundedSender<Outgoing>) -> Option<(usize, u32)> {
        let mut sessions = self.sessions.lock().unwrap();
        let active = self.active_slots.load(Relaxed).min(MAX_SLOTS);
        let slot = sessions[..active]
            .iter()
            .position(|s| s.as_ref().is_some_and(|s| s.device_id == device_id))
            .or_else(|| sessions[..active].iter().position(|s| s.is_none()))?;
        if let Some(old) = sessions[slot].take() {
            log::info!("Slot {}: {} reconnected; closing old connection", slot + 1, old.name);
            let _ = old.tx.send(Outgoing::Close);
        }
        let token = loop {
            let t: u32 = rand::random();
            if t != 0 {
                break t;
            }
        };
        sessions[slot] = Some(Session {
            conn_id,
            device_id: device_id.to_string(),
            name: name.to_string(),
            model: model.to_string(),
            token,
            udp_addr: None,
            status: PhoneStatus::default(),
            rtt_ms: None,
            jitter_ms: 0.0,
            last_audio: None,
            last_arrival: None,
            tx,
        });
        self.engine.send(ToAudio::Reset { slot: slot as u8 });
        Some((slot, token))
    }

    fn release(&self, slot: usize, conn_id: u64) {
        let mut sessions = self.sessions.lock().unwrap();
        if sessions[slot].as_ref().is_some_and(|s| s.conn_id == conn_id) {
            let s = sessions[slot].take().unwrap();
            log::info!("Slot {}: {} disconnected", slot + 1, s.name);
            self.engine.send(ToAudio::Reset { slot: slot as u8 });
        }
    }
}

async fn run(net: Arc<Net>) {
    let udp = match UdpSocket::bind(("0.0.0.0", AUDIO_PORT)).await {
        Ok(s) => Arc::new(s),
        Err(e) => return listen_failed(&net, format!("Audio port {AUDIO_PORT} is busy ({e}). Is another copy running?")),
    };
    let listener = match TcpListener::bind(("0.0.0.0", CONTROL_PORT)).await {
        Ok(l) => l,
        Err(e) => return listen_failed(&net, format!("Control port {CONTROL_PORT} is busy ({e}). Is another copy running?")),
    };
    log::info!("Listening: control tcp/{CONTROL_PORT}, audio udp/{AUDIO_PORT}");
    let _mdns = advertise(&net.desktop_name);

    tokio::spawn(udp_loop(net.clone(), udp.clone()));
    tokio::spawn(ping_loop(net.clone(), udp));
    loop {
        match listener.accept().await {
            Ok((stream, peer)) => {
                let conn_id = net.next_conn.fetch_add(1, Relaxed);
                tokio::spawn(handle_phone(net.clone(), stream, peer, conn_id));
            }
            Err(e) => {
                log::warn!("accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

fn listen_failed(net: &Net, msg: String) {
    log::error!("{msg}");
    *net.listen_error.lock().unwrap() = Some(msg);
}

async fn handle_phone(net: Arc<Net>, stream: TcpStream, peer: SocketAddr, conn_id: u64) {
    let _ = stream.set_nodelay(true);
    let ws = match tokio_tungstenite::accept_async(stream).await {
        Ok(ws) => ws,
        Err(e) => {
            log::debug!("{peer}: not a WebSocket ({e})");
            return;
        }
    };
    let (mut sink, mut source) = ws.split();
    let send = |msg: &DesktopMessage| Message::Text(serde_json::to_string(msg).unwrap().into());

    let hello = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(Ok(msg)) = source.next().await {
            if let Message::Text(text) = msg {
                return serde_json::from_str::<PhoneMessage>(&text).ok();
            }
        }
        None
    })
    .await;
    let Ok(Some(PhoneMessage::Hello { protocol, device_id, name, platform, model, app_version })) = hello else {
        let reject = DesktopMessage::Reject { reason: "bad_hello".into(), message: "Expected a hello message.".into() };
        let _ = sink.send(send(&reject)).await;
        return;
    };
    if protocol != PROTOCOL_VERSION {
        let reject = DesktopMessage::Reject {
            reason: "protocol".into(),
            message: format!("This desktop app speaks protocol {PROTOCOL_VERSION}; the phone app speaks {protocol}. Update both apps."),
        };
        let _ = sink.send(send(&reject)).await;
        return;
    }

    let (tx, mut rx) = mpsc::unbounded_channel();
    let Some((slot, token)) = net.assign(conn_id, &device_id, &name, &model, tx) else {
        log::info!("{peer}: {name} rejected, all slots in use");
        let reject = DesktopMessage::Reject { reason: "full".into(), message: "All phone slots are in use.".into() };
        let _ = sink.send(send(&reject)).await;
        return;
    };
    log::info!("Slot {}: {name} connected from {peer} ({platform} {model}, app {app_version})", slot + 1);

    let engine_slot = &net.engine.shared.slots[slot];
    let welcome = DesktopMessage::Welcome {
        protocol: PROTOCOL_VERSION,
        slot: slot as u32 + 1,
        channel: engine_slot.channel.load(Relaxed),
        token,
        udp_port: AUDIO_PORT,
        bitrate: DEFAULT_BITRATE,
        desktop_name: net.desktop_name.clone(),
    };
    let operator = DesktopMessage::Operator { operator_muted: engine_slot.operator_muted.load(Relaxed) };
    if sink.send(send(&welcome)).await.is_err() || sink.send(send(&operator)).await.is_err() {
        net.release(slot, conn_id);
        return;
    }

    loop {
        tokio::select! {
            msg = source.next() => match msg {
                Some(Ok(Message::Text(text))) => {
                    if let Ok(PhoneMessage::Status { talent_muted, input, capture_latency_ms, battery, interrupted, .. }) =
                        serde_json::from_str::<PhoneMessage>(&text)
                    {
                        let mut sessions = net.sessions.lock().unwrap();
                        if let Some(s) = sessions[slot].as_mut().filter(|s| s.conn_id == conn_id) {
                            if s.status.interrupted != interrupted {
                                log::warn!("Slot {}: {} mic {}", slot + 1, s.name, if interrupted { "interrupted" } else { "resumed" });
                            }
                            s.status = PhoneStatus { talent_muted, input, capture_latency_ms, battery, interrupted };
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
            out = rx.recv() => match out {
                Some(Outgoing::Msg(msg)) => {
                    if sink.send(send(&msg)).await.is_err() {
                        break;
                    }
                }
                Some(Outgoing::Close) | None => {
                    let _ = sink.send(Message::Close(None)).await;
                    break;
                }
            },
        }
    }
    net.release(slot, conn_id);
}

async fn udp_loop(net: Arc<Net>, sock: Arc<UdpSocket>) {
    let mut buf = [0u8; 2048];
    loop {
        let (n, from) = match sock.recv_from(&mut buf).await {
            Ok(r) => r,
            Err(e) => {
                // Windows reports ICMP "port unreachable" this way; just carry on.
                log::debug!("udp recv: {e}");
                continue;
            }
        };
        if n > MAX_DATAGRAM {
            continue;
        }
        match protocol::parse(&buf[..n]) {
            Some(Datagram::Audio(pkt)) => {
                let now = Instant::now();
                let slot = {
                    let mut sessions = net.sessions.lock().unwrap();
                    let Some((slot, s)) =
                        sessions.iter_mut().enumerate().find_map(|(i, s)| s.as_mut().filter(|s| s.token == pkt.token).map(|s| (i, s)))
                    else {
                        continue;
                    };
                    s.udp_addr = Some(from);
                    s.last_audio = Some(now);
                    if let Some((prev_at, prev_seq)) = s.last_arrival {
                        let frames = pkt.seq.wrapping_sub(prev_seq) as i32;
                        if frames > 0 {
                            let d = now.duration_since(prev_at).as_secs_f64() * 1000.0 - frames as f64 * 10.0;
                            s.jitter_ms += (d.abs() - s.jitter_ms) / 16.0;
                        }
                    }
                    if s.last_arrival.is_none_or(|(_, prev)| pkt.seq.wrapping_sub(prev) as i32 > 0) {
                        s.last_arrival = Some((now, pkt.seq));
                    }
                    slot
                };
                let mut data = [0u8; MAX_DATAGRAM];
                data[..n].copy_from_slice(&buf[..n]);
                net.engine.send(ToAudio::Packet { slot: slot as u8, len: n as u16, data });
            }
            Some(Datagram::Pong { token, stamp }) => {
                let rtt = net.now_ns().saturating_sub(stamp) as f64 / 1e6;
                let mut sessions = net.sessions.lock().unwrap();
                if let Some(s) = sessions.iter_mut().flatten().find(|s| s.token == token) {
                    s.rtt_ms = Some(match s.rtt_ms {
                        Some(prev) => prev + (rtt - prev) * 0.25,
                        None => rtt,
                    });
                }
            }
            _ => {}
        }
    }
}

async fn ping_loop(net: Arc<Net>, sock: Arc<UdpSocket>) {
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    loop {
        tick.tick().await;
        let targets: Vec<(SocketAddr, u32)> = net
            .sessions
            .lock()
            .unwrap()
            .iter()
            .flatten()
            .filter_map(|s| s.udp_addr.map(|a| (a, s.token)))
            .collect();
        for (addr, token) in targets {
            let _ = sock.send_to(&protocol::write_ping(KIND_PING, token, net.now_ns()), addr).await;
        }
    }
}

/// IPv4 addresses phones could use to reach this computer.
pub fn local_ipv4s() -> Vec<IpAddr> {
    let mut ips: Vec<IpAddr> = local_ip_address::list_afinet_netifas()
        .unwrap_or_default()
        .into_iter()
        .map(|(_, ip)| ip)
        .filter(|ip| matches!(ip, IpAddr::V4(v4) if !v4.is_loopback() && !v4.is_link_local()))
        .collect();
    ips.sort();
    ips.dedup();
    ips
}

/// The computer's friendly name, e.g. "Studio Mac".
pub fn computer_name() -> String {
    #[cfg(target_os = "macos")]
    if let Ok(out) = std::process::Command::new("scutil").args(["--get", "ComputerName"]).output() {
        let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !name.is_empty() {
            return name;
        }
    }
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Desktop".into())
}

fn advertise(desktop_name: &str) -> Option<mdns_sd::ServiceDaemon> {
    let ips = local_ipv4s();
    if ips.is_empty() {
        log::warn!("No network address found; phones will need the IP address typed in");
        return None;
    }
    let host: String = computer_name()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect();
    let result = (|| {
        let daemon = mdns_sd::ServiceDaemon::new()?;
        let info = mdns_sd::ServiceInfo::new(
            SERVICE_TYPE,
            desktop_name,
            &format!("{host}-tstv.local."),
            &ips[..],
            CONTROL_PORT,
            &[("proto", "1")][..],
        )?;
        daemon.register(info)?;
        Ok::<_, mdns_sd::Error>(daemon)
    })();
    match result {
        Ok(daemon) => {
            log::info!("Advertising \"{desktop_name}\" on {ips:?}");
            Some(daemon)
        }
        Err(e) => {
            log::warn!("Bonjour advertising failed ({e}); phones will need the IP address typed in");
            None
        }
    }
}
