//! Network side: the WebSocket control server, the UDP audio socket, round-trip pings and
//! Bonjour advertising. Runs on its own tokio runtime thread.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
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
    PROTOCOL_VERSION, PhoneMessage,
};
#[cfg(not(target_os = "macos"))]
use crate::protocol::SERVICE_TYPE;
use crate::settings::SlotLock;

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
    /// The name the phone gave, for when the operator clears theirs.
    phone_name: String,
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

/// The phone that last left a slot.
pub struct LeftPhone {
    pub device_id: String,
    pub name: String,
    phone_name: String,
    pub model: String,
    pub at: Instant,
}

/// Who may join and where, as set by the operator.
#[derive(Default)]
pub struct JoinRules {
    pub require_code: bool,
    pub code: String,
    pub locks: [Option<SlotLock>; MAX_SLOTS],
    pub names: HashMap<String, String>,
}

impl JoinRules {
    /// Why a phone giving `code` can't join, if it can't.
    fn code_problem(&self, code: Option<&str>) -> Option<&'static str> {
        if !self.require_code {
            return None;
        }
        match code.map(str::trim) {
            Some(given) if given == self.code => None,
            Some(given) if !given.is_empty() => Some("That join code isn't right. Check it under Connect a phone in the desktop app."),
            _ => Some("This desktop needs a join code. It's under Connect a phone in the desktop app."),
        }
    }
}

/// Picks a slot for a phone, in this order: the slot it's already in, a slot locked to it, the
/// slot it left, a free slot nobody has used, then any free slot. Locked slots stay reserved.
fn choose_slot(device_id: &str, occupied: &[Option<&str>], locks: &[Option<&str>], left: &[Option<&str>]) -> Option<usize> {
    let slots = 0..occupied.len();
    let free = |i: usize| occupied[i].is_none() && locks[i].is_none_or(|owner| owner == device_id);
    slots
        .clone()
        .find(|&i| occupied[i] == Some(device_id))
        .or_else(|| slots.clone().find(|&i| locks[i] == Some(device_id)))
        .or_else(|| slots.clone().find(|&i| left[i] == Some(device_id) && free(i)))
        .or_else(|| slots.clone().find(|&i| left[i].is_none() && free(i)))
        .or_else(|| slots.clone().find(|&i| free(i)))
}

pub struct Net {
    pub sessions: Mutex<[Option<Session>; MAX_SLOTS]>,
    pub left: Mutex<[Option<LeftPhone>; MAX_SLOTS]>,
    pub rules: Mutex<JoinRules>,
    /// How many slots accept phones.
    pub active_slots: AtomicUsize,
    pub desktop_name: String,
    pub listen_error: Mutex<Option<String>>,
    engine: Arc<Engine>,
    next_conn: AtomicU64,
    epoch: Instant,
}

impl Net {
    pub fn start(engine: Arc<Engine>, desktop_name: String, active_slots: usize, rules: JoinRules) -> Arc<Net> {
        let net = Arc::new(Net {
            sessions: Mutex::new(std::array::from_fn(|_| None)),
            left: Mutex::new(std::array::from_fn(|_| None)),
            rules: Mutex::new(rules),
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

    /// The phone in a slot, or the one that last left it: (device id, name).
    pub fn slot_phone(&self, slot: usize) -> Option<(String, String)> {
        let sessions = self.sessions.lock().unwrap();
        let left = self.left.lock().unwrap();
        match (&sessions[slot], &left[slot]) {
            (Some(s), _) => Some((s.device_id.clone(), s.name.clone())),
            (None, Some(l)) => Some((l.device_id.clone(), l.name.clone())),
            (None, None) => None,
        }
    }

    /// Gives the slot's phone the operator's name, or back its own if `name` is empty.
    pub fn rename(&self, slot: usize, name: &str) {
        let mut sessions = self.sessions.lock().unwrap();
        let mut left = self.left.lock().unwrap();
        let mut rules = self.rules.lock().unwrap();
        let name = name.trim();
        let (device_id, shown) = if let Some(s) = sessions[slot].as_mut() {
            s.name = if name.is_empty() { s.phone_name.clone() } else { name.to_string() };
            (s.device_id.clone(), s.name.clone())
        } else if let Some(l) = left[slot].as_mut() {
            l.name = if name.is_empty() { l.phone_name.clone() } else { name.to_string() };
            (l.device_id.clone(), l.name.clone())
        } else {
            return;
        };
        if name.is_empty() {
            rules.names.remove(&device_id);
        } else {
            rules.names.insert(device_id, shown.clone());
        }
        if let Some(lock) = rules.locks[slot].as_mut() {
            lock.name = shown;
        }
    }

    /// Gives a new phone a slot. Returns (slot index, token, name to show).
    fn assign(&self, conn_id: u64, device_id: &str, phone_name: &str, model: &str, tx: mpsc::UnboundedSender<Outgoing>) -> Option<(usize, u32, String)> {
        let mut sessions = self.sessions.lock().unwrap();
        let mut left = self.left.lock().unwrap();
        let rules = self.rules.lock().unwrap();
        let active = self.active_slots.load(Relaxed).min(MAX_SLOTS);
        let occupied: Vec<_> = sessions[..active].iter().map(|s| s.as_ref().map(|s| s.device_id.as_str())).collect();
        let locks: Vec<_> = rules.locks[..active].iter().map(|l| l.as_ref().map(|l| l.device_id.as_str())).collect();
        let gone: Vec<_> = left[..active].iter().map(|l| l.as_ref().map(|l| l.device_id.as_str())).collect();
        let slot = choose_slot(device_id, &occupied, &locks, &gone)?;
        let name = rules.names.get(device_id).cloned().unwrap_or_else(|| phone_name.to_string());
        left[slot] = None;
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
            name: name.clone(),
            phone_name: phone_name.to_string(),
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
        Some((slot, token, name))
    }

    fn release(&self, slot: usize, conn_id: u64) {
        let mut sessions = self.sessions.lock().unwrap();
        if sessions[slot].as_ref().is_some_and(|s| s.conn_id == conn_id) {
            let s = sessions[slot].take().unwrap();
            log::info!("Slot {}: {} disconnected", slot + 1, s.name);
            self.left.lock().unwrap()[slot] =
                Some(LeftPhone { device_id: s.device_id, name: s.name, phone_name: s.phone_name, model: s.model, at: Instant::now() });
            self.engine.send(ToAudio::Reset { slot: slot as u8 });
        }
    }
}

async fn run(net: Arc<Net>) {
    let udp = match dual_stack_udp(AUDIO_PORT) {
        Ok(s) => Arc::new(s),
        Err(e) => return listen_failed(&net, format!("Audio port {AUDIO_PORT} is busy ({e}). Is another copy running?")),
    };
    let listener = match dual_stack_tcp(CONTROL_PORT) {
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

/// A socket on `port` that accepts both IPv4 and IPv6: phone hotspots are often IPv6-only.
fn dual_stack_socket(kind: socket2::Type, port: u16) -> std::io::Result<socket2::Socket> {
    let sock = socket2::Socket::new(socket2::Domain::IPV6, kind, None)?;
    sock.set_only_v6(false)?;
    // Lets the app restart straight away. Not on UDP or Windows, where it would allow a second
    // copy to share the port instead of reporting it busy.
    #[cfg(not(windows))]
    if kind == socket2::Type::STREAM {
        sock.set_reuse_address(true)?;
    }
    sock.set_nonblocking(true)?;
    sock.bind(&SocketAddr::from((Ipv6Addr::UNSPECIFIED, port)).into())?;
    Ok(sock)
}

fn dual_stack_tcp(port: u16) -> std::io::Result<TcpListener> {
    let sock = dual_stack_socket(socket2::Type::STREAM, port)?;
    sock.listen(128)?;
    TcpListener::from_std(sock.into())
}

fn dual_stack_udp(port: u16) -> std::io::Result<UdpSocket> {
    UdpSocket::from_std(dual_stack_socket(socket2::Type::DGRAM, port)?.into())
}

fn listen_failed(net: &Net, msg: String) {
    log::error!("{msg}");
    *net.listen_error.lock().unwrap() = Some(msg);
}

async fn handle_phone(net: Arc<Net>, stream: TcpStream, peer: SocketAddr, conn_id: u64) {
    // Dual-stack sockets report IPv4 peers as ::ffff:a.b.c.d; show them normally in the log.
    let peer = SocketAddr::new(peer.ip().to_canonical(), peer.port());
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
    let Ok(Some(PhoneMessage::Hello { protocol, device_id, name, platform, model, app_version, code })) = hello else {
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

    let code_problem = net.rules.lock().unwrap().code_problem(code.as_deref());
    if let Some(message) = code_problem {
        log::info!("{peer}: {name} rejected, join code missing or wrong");
        let reject = DesktopMessage::Reject { reason: "code".into(), message: message.into() };
        let _ = sink.send(send(&reject)).await;
        return;
    }

    let (tx, mut rx) = mpsc::unbounded_channel();
    let Some((slot, token, name)) = net.assign(conn_id, &device_id, &name, &model, tx) else {
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
                            if s.status.talent_muted != talent_muted {
                                log::info!("Slot {}: {} {} on the phone", slot + 1, s.name, if talent_muted { "muted" } else { "unmuted" });
                            }
                            if s.status.input != input {
                                log::info!("Slot {}: {} mic is now {}", slot + 1, s.name, input.as_ref().map_or("unknown", |i| i.name.as_str()));
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

/// Addresses to show the operator for typing into a phone: the IPv4 ones, or on an
/// IPv6-only network (many phone hotspots) one IPv6 address.
pub fn phone_addresses() -> Vec<IpAddr> {
    let mut ips: Vec<IpAddr> = local_ip_address::list_afinet_netifas()
        .unwrap_or_default()
        .into_iter()
        .filter(|(name, ip)| reachable_by_phones(name, *ip))
        .map(|(_, ip)| ip)
        .collect();
    ips.sort();
    ips.dedup();
    if ips.iter().any(IpAddr::is_ipv4) {
        ips.retain(IpAddr::is_ipv4);
    } else {
        ips.truncate(1);
    }
    ips
}

fn reachable_by_phones(interface: &str, ip: IpAddr) -> bool {
    // VPN tunnels and Apple's device-to-device links.
    const VIRTUAL: [&str; 4] = ["utun", "awdl", "llw", "anpi"];
    if VIRTUAL.iter().any(|prefix| interface.starts_with(prefix)) {
        return false;
    }
    match ip {
        // 192.0.0.x is a stand-in macOS uses on IPv6-only networks; nothing else can reach it.
        IpAddr::V4(v4) => !v4.is_loopback() && !v4.is_link_local() && v4.octets()[..3] != [192, 0, 0],
        // Link-local addresses only work with an interface name, so they can't be typed in.
        IpAddr::V6(v6) => !v6.is_loopback() && !v6.is_unicast_link_local(),
    }
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

#[cfg(target_os = "macos")]
mod bonjour {
    use std::ffi::{CString, c_char, c_void};

    unsafe extern "C" {
        // From dns_sd.h, part of the macOS system library.
        fn DNSServiceRegister(
            sd_ref: *mut *mut c_void,
            flags: u32,
            interface_index: u32,
            name: *const c_char,
            regtype: *const c_char,
            domain: *const c_char,
            host: *const c_char,
            port_network_order: u16,
            txt_len: u16,
            txt_record: *const c_void,
            callback: *const c_void,
            context: *mut c_void,
        ) -> i32;
    }

    /// Keeps the service advertised for as long as the process lives. The system withdraws it
    /// when the process exits, however that happens.
    pub struct Registration(#[allow(dead_code)] *mut c_void);

    pub fn register(name: &str, regtype: &str, port: u16, txt: &[u8]) -> Result<Registration, i32> {
        // Bonjour names are limited to 63 bytes.
        let mut end = name.len().min(63);
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        let name = CString::new(&name[..end]).map_err(|_| -1)?;
        let regtype = CString::new(regtype).map_err(|_| -1)?;
        let mut sd_ref = std::ptr::null_mut();
        let err = unsafe {
            DNSServiceRegister(
                &mut sd_ref,
                0,
                0,
                name.as_ptr(),
                regtype.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                port.to_be(),
                txt.len() as u16,
                txt.as_ptr().cast(),
                std::ptr::null(),
                std::ptr::null_mut(),
            )
        };
        if err == 0 { Ok(Registration(sd_ref)) } else { Err(err) }
    }
}

/// On a Mac the system's own Bonjour service advertises the app: it gives the right addresses on
/// each network, follows the Mac as it roams, and removes the entry when the app quits.
#[cfg(target_os = "macos")]
fn advertise(desktop_name: &str) -> Option<bonjour::Registration> {
    match bonjour::register(desktop_name, "_tstvmic._tcp", CONTROL_PORT, b"\x07proto=1") {
        Ok(registration) => {
            log::info!("Advertising \"{desktop_name}\" on {:?}", phone_addresses());
            Some(registration)
        }
        Err(e) => {
            log::warn!("Bonjour advertising failed ({e}); phones will need the address typed in");
            None
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn advertise(desktop_name: &str) -> Option<mdns_sd::ServiceDaemon> {
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
            (),
            CONTROL_PORT,
            &[("proto", "1")][..],
        )?
        // Follow the computer's addresses: on campus Wi-Fi they change as the laptop roams.
        .enable_addr_auto();
        daemon.register(info)?;
        Ok::<_, mdns_sd::Error>(daemon)
    })();
    match result {
        Ok(daemon) => {
            log::info!("Advertising \"{desktop_name}\" on {:?}", phone_addresses());
            Some(daemon)
        }
        Err(e) => {
            log::warn!("Bonjour advertising failed ({e}); phones will need the IP address typed in");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phone_returns_to_its_slot() {
        let none = [None; 3];
        // Already in a slot (a reconnect taking over its old connection).
        assert_eq!(choose_slot("a", &[None, Some("a"), None], &none, &none), Some(1));
        // Left slot 2 and comes back, even though slots 0 and 1 are free.
        assert_eq!(choose_slot("a", &none, &none, &[None, None, Some("a")]), Some(2));
        // Locked to slot 1, while another phone has since left slot 0.
        assert_eq!(choose_slot("a", &none, &[None, Some("a"), None], &[Some("b"), None, None]), Some(1));
    }

    #[test]
    fn new_phones_avoid_slots_others_may_come_back_to() {
        let none = [None; 3];
        assert_eq!(choose_slot("new", &none, &none, &[Some("a"), None, None]), Some(1));
        assert_eq!(choose_slot("new", &[None, Some("b"), Some("c")], &none, &[Some("a"), None, None]), Some(0));
    }

    #[test]
    fn locked_slots_stay_reserved_and_full_means_full() {
        let none = [None; 3];
        assert_eq!(choose_slot("new", &none, &[Some("a"), Some("b"), None], &none), Some(2));
        assert_eq!(choose_slot("new", &none, &[Some("a"), Some("b"), Some("c")], &none), None);
        assert_eq!(choose_slot("new", &[Some("a"), Some("b"), Some("c")], &none, &none), None);
    }

    #[test]
    fn join_code_is_only_checked_when_required() {
        let mut rules = JoinRules { code: "4821".into(), ..Default::default() };
        assert_eq!(rules.code_problem(None), None);
        assert_eq!(rules.code_problem(Some("wrong")), None);
        rules.require_code = true;
        assert!(rules.code_problem(None).is_some());
        assert!(rules.code_problem(Some("")).is_some());
        assert!(rules.code_problem(Some("0000")).is_some());
        assert_eq!(rules.code_problem(Some("4821")), None);
        assert_eq!(rules.code_problem(Some(" 4821 ")), None);
    }

    #[tokio::test]
    async fn control_port_accepts_ipv4_and_ipv6() {
        let listener = dual_stack_tcp(0).unwrap();
        let port = listener.local_addr().unwrap().port();
        for addr in [format!("127.0.0.1:{port}"), format!("[::1]:{port}")] {
            let client = TcpStream::connect(&addr).await.unwrap();
            let (_, peer) = listener.accept().await.unwrap();
            assert_eq!(peer.ip().to_canonical(), client.local_addr().unwrap().ip(), "{addr}");
        }
    }

    #[tokio::test]
    async fn audio_port_receives_ipv4_and_replies_on_ipv6() {
        let server = dual_stack_udp(0).unwrap();
        let port = server.local_addr().unwrap().port();
        for (bind, target) in [("127.0.0.1:0", format!("127.0.0.1:{port}")), ("[::1]:0", format!("[::1]:{port}"))] {
            let client = UdpSocket::bind(bind).await.unwrap();
            client.send_to(b"hi", &target).await.unwrap();
            let mut buf = [0u8; 8];
            let (n, from) = server.recv_from(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"hi");
            // Replies must reach IPv4 phones too, which the socket sees as ::ffff:a.b.c.d.
            server.send_to(b"ok", from).await.unwrap();
            let n = client.recv(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], b"ok");
        }
    }

    #[test]
    fn only_addresses_phones_can_reach_are_shown() {
        let ok = |name, ip: &str| reachable_by_phones(name, ip.parse().unwrap());
        assert!(ok("en0", "10.146.113.55"));
        assert!(ok("en0", "2607:fb90:e6ab:2ba:1472:5521:4233:3f97"));
        assert!(!ok("en0", "192.0.0.2"), "macOS stand-in on IPv6-only networks");
        assert!(!ok("en0", "fe80::1cc3:72ce:ab94:9e98"));
        assert!(!ok("lo0", "127.0.0.1"));
        assert!(!ok("utun3", "100.100.1.2"));
    }
}
