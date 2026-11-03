//! TSTV Phone Mics — desktop app. Receives phone audio over the local network and plays
//! each phone out of its own channel of the audio interface (StudioLive USB channels 17–21).

pub mod engine;
pub mod net;
pub mod opus;
pub mod playout;
pub mod protocol;
pub mod settings;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use engine::{Engine, MAX_SLOTS};
use net::Net;
use protocol::DesktopMessage;
use settings::{MAX_BUFFER_MS, MIN_BUFFER_MS, Settings, SlotSettings};

/// Milestone 1 accepts one phone. Milestone 2 raises this to MAX_SLOTS.
const ACTIVE_SLOTS: usize = 1;
/// Used in the latency estimate until the phone reports its own figure.
const DEFAULT_CAPTURE_LATENCY_MS: f64 = 20.0;

struct AppState {
    engine: Arc<Engine>,
    net: Arc<Net>,
    settings: Mutex<Settings>,
    settings_path: PathBuf,
    log_path: PathBuf,
}

impl AppState {
    fn update_settings(&self, change: impl FnOnce(&mut Settings)) {
        let mut s = self.settings.lock().unwrap();
        change(&mut s);
        s.save(&self.settings_path);
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Setup {
    version: &'static str,
    desktop_name: String,
    ips: Vec<String>,
    control_port: u16,
    active_slots: usize,
    devices: Vec<engine::DeviceInfo>,
    /// `None` = automatic (StudioLive if connected).
    selected_device: Option<String>,
    buffer_ms: f32,
    min_buffer_ms: f32,
    max_buffer_ms: f32,
    slots: Vec<SlotSettings>,
    log_path: String,
}

#[tauri::command]
fn get_setup(state: State<'_, Arc<AppState>>) -> Setup {
    log::debug!("UI requested setup");
    let s = state.settings.lock().unwrap().clone();
    Setup {
        version: env!("CARGO_PKG_VERSION"),
        desktop_name: state.net.desktop_name.clone(),
        ips: net::local_ipv4s().iter().map(|ip| ip.to_string()).collect(),
        control_port: protocol::CONTROL_PORT,
        active_slots: ACTIVE_SLOTS,
        devices: engine::list_output_devices(),
        selected_device: s.device,
        buffer_ms: s.buffer_ms,
        min_buffer_ms: MIN_BUFFER_MS,
        max_buffer_ms: MAX_BUFFER_MS,
        slots: s.slots,
        log_path: state.log_path.display().to_string(),
    }
}

#[tauri::command]
fn set_device(state: State<'_, Arc<AppState>>, device: Option<String>) {
    log::info!("Operator chose output device: {}", device.as_deref().unwrap_or("automatic"));
    state.engine.open_device(device.clone());
    state.update_settings(|s| s.device = device);
}

#[tauri::command]
fn set_buffer_ms(state: State<'_, Arc<AppState>>, ms: f32) {
    let ms = ms.clamp(MIN_BUFFER_MS, MAX_BUFFER_MS);
    state.engine.shared.buffer_ms.store(ms);
    state.update_settings(|s| s.buffer_ms = ms);
}

#[tauri::command]
fn set_channel(state: State<'_, Arc<AppState>>, slot: usize, channel: u32) {
    if slot >= MAX_SLOTS {
        return;
    }
    log::info!("Slot {} → output channel {channel}", slot + 1);
    state.engine.shared.slots[slot].channel.store(channel, Relaxed);
    state.update_settings(|s| s.slots[slot].channel = channel);
}

#[tauri::command]
fn set_gain(state: State<'_, Arc<AppState>>, slot: usize, db: f32) {
    if slot >= MAX_SLOTS {
        return;
    }
    let db = db.clamp(-60.0, 12.0);
    state.engine.shared.slots[slot].gain_db.store(db);
    state.update_settings(|s| s.slots[slot].gain_db = db);
}

#[tauri::command]
fn set_mute(state: State<'_, Arc<AppState>>, slot: usize, muted: bool) {
    if slot >= MAX_SLOTS {
        return;
    }
    log::info!("Slot {} operator {}", slot + 1, if muted { "muted" } else { "unmuted" });
    state.engine.shared.slots[slot].operator_muted.store(muted, Relaxed);
    state.net.notify_operator_mute(slot, muted);
}

#[tauri::command]
fn mute_all(state: State<'_, Arc<AppState>>, muted: bool) {
    log::info!("Operator {} all phones", if muted { "muted" } else { "unmuted" });
    for slot in 0..MAX_SLOTS {
        state.engine.shared.slots[slot].operator_muted.store(muted, Relaxed);
        state.net.notify_operator_mute(slot, muted);
    }
}

#[tauri::command]
fn set_tone(state: State<'_, Arc<AppState>>, slot: usize, on: bool) {
    if slot >= MAX_SLOTS {
        return;
    }
    log::info!("Slot {} test tone {}", slot + 1, if on { "on" } else { "off" });
    state.engine.shared.slots[slot].tone.store(on, Relaxed);
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct EngineView {
    running: bool,
    device: String,
    channels: u32,
    sample_rate: u32,
    output_latency_ms: f32,
    buffer_ms: f32,
    error: Option<String>,
}

#[derive(Serialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
struct SlotView {
    slot: usize,
    active: bool,
    connected: bool,
    /// "empty" | "waiting" (connected, no audio yet) | "buffering" | "live" | "no-audio"
    state: &'static str,
    name: String,
    model: String,
    mic: String,
    mic_kind: String,
    talent_muted: bool,
    operator_muted: bool,
    interrupted: bool,
    battery: Option<f64>,
    tone: bool,
    gain_db: f32,
    channel: u32,
    level_db: f32,
    latency_ms: Option<f64>,
    buffer_ms: f32,
    loss_pct: f64,
    jitter_ms: f64,
    rtt_ms: Option<f64>,
    late: u64,
    recovered: u64,
    concealed: u64,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Tick {
    engine: EngineView,
    network_error: Option<String>,
    slots: Vec<SlotView>,
}

/// Every 50 ms: sends meters/status to the UI. Every 500 ms: sends stats to the phones.
fn monitor(app: AppHandle, state: Arc<AppState>) {
    let mut history: Vec<VecDeque<(Instant, u64, u64)>> = vec![VecDeque::new(); MAX_SLOTS];
    let mut last_stats = Instant::now();
    let mut last_log = Instant::now();
    let mut logged_concealed = [0u64; MAX_SLOTS];
    loop {
        std::thread::sleep(Duration::from_millis(50));
        let shared = &state.engine.shared;
        let now = Instant::now();
        let output_latency = shared.output_latency_ms.load() as f64;
        let running = shared.running.load(Relaxed);
        let engine = EngineView {
            running,
            device: shared.device_name.lock().unwrap().clone(),
            channels: shared.device_channels.load(Relaxed),
            sample_rate: shared.device_rate.load(Relaxed),
            output_latency_ms: shared.output_latency_ms.load(),
            buffer_ms: shared.buffer_ms.load(),
            error: shared.error.lock().unwrap().clone(),
        };

        let send_stats = now.duration_since(last_stats) >= Duration::from_millis(500);
        let write_log = now.duration_since(last_log) >= Duration::from_secs(10);
        let sessions = state.net.sessions.lock().unwrap();
        let mut slots = Vec::with_capacity(MAX_SLOTS);
        for i in 0..MAX_SLOTS {
            let sh = &shared.slots[i];
            let peak = sh.peak.swap(0.0);
            let played = sh.played.load(Relaxed);
            let concealed = sh.concealed.load(Relaxed);
            let h = &mut history[i];
            h.push_back((now, played, concealed));
            while h.len() > 2 && now.duration_since(h[0].0) > Duration::from_secs(3) {
                h.pop_front();
            }
            let (_, p0, c0) = h[0];
            let loss_pct = if played > p0 { (concealed - c0) as f64 * 100.0 / (played - p0) as f64 } else { 0.0 };
            let depth_ms = sh.depth_ms.load();
            let audio_state = sh.state.load(Relaxed);

            let mut v = SlotView {
                slot: i,
                active: i < state.net.active_slots.load(Relaxed),
                tone: sh.tone.load(Relaxed),
                gain_db: sh.gain_db.load(),
                channel: sh.channel.load(Relaxed),
                operator_muted: sh.operator_muted.load(Relaxed),
                level_db: if peak > 0.0 { 20.0 * peak.log10() } else { -120.0 },
                state: "empty",
                ..Default::default()
            };
            if let Some(s) = &sessions[i] {
                let fresh = s.last_audio.is_some_and(|t| now.duration_since(t) < Duration::from_millis(500));
                v.connected = true;
                v.state = match (fresh, audio_state) {
                    (false, _) if s.last_audio.is_none() => "waiting",
                    (false, _) => "no-audio",
                    (true, 2) => "live",
                    _ => "buffering",
                };
                v.name = s.name.clone();
                v.model = s.model.clone();
                if let Some(input) = &s.status.input {
                    v.mic = input.name.clone();
                    v.mic_kind = input.kind.clone();
                }
                v.talent_muted = s.status.talent_muted || sh.talent_muted.load(Relaxed);
                v.interrupted = s.status.interrupted;
                v.battery = s.status.battery;
                v.rtt_ms = s.rtt_ms;
                v.jitter_ms = s.jitter_ms;
                v.buffer_ms = depth_ms;
                v.loss_pct = loss_pct;
                v.late = sh.late.load(Relaxed);
                v.recovered = sh.recovered.load(Relaxed);
                v.concealed = concealed;
                if running && audio_state == 2 {
                    let capture = s.status.capture_latency_ms.unwrap_or(DEFAULT_CAPTURE_LATENCY_MS);
                    v.latency_ms = Some(capture + s.rtt_ms.unwrap_or(0.0) / 2.0 + depth_ms as f64 + output_latency);
                }
                if send_stats {
                    s.send(DesktopMessage::Stats { latency_ms: v.latency_ms.unwrap_or(0.0), loss_pct, channel: v.channel });
                }
                if write_log {
                    let new_loss = concealed - logged_concealed[i];
                    log::info!(
                        "Slot {}: {} {} | latency {} | loss {:.1}% ({} frames since last log), late {}, recovered {}, jitter {:.1} ms, rtt {} | mic {}",
                        i + 1,
                        v.name,
                        v.state,
                        v.latency_ms.map_or("-".into(), |l| format!("{l:.0} ms")),
                        loss_pct,
                        new_loss,
                        v.late,
                        v.recovered,
                        v.jitter_ms,
                        v.rtt_ms.map_or("-".into(), |r| format!("{r:.1} ms")),
                        v.mic,
                    );
                    logged_concealed[i] = concealed;
                }
            }
            slots.push(v);
        }
        drop(sessions);
        if send_stats {
            last_stats = now;
        }
        if write_log {
            last_log = now;
        }
        let network_error = state.net.listen_error.lock().unwrap().clone();
        let _ = app.emit("tick", Tick { engine, network_error, slots });
    }
}

fn init_logging(log_dir: &std::path::Path) -> PathBuf {
    use simplelog::*;
    let _ = std::fs::create_dir_all(log_dir);
    let path = log_dir.join("tstv-phone-mics.log");
    // Keep one previous log once the current one passes 5 MB.
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > 5_000_000) {
        let _ = std::fs::rename(&path, log_dir.join("tstv-phone-mics.old.log"));
    }
    // TSTV_LOG=debug for more detail.
    let level = if std::env::var("TSTV_LOG").as_deref() == Ok("debug") { LevelFilter::Debug } else { LevelFilter::Info };
    let config = ConfigBuilder::new().set_time_format_rfc3339().set_time_offset_to_local().unwrap_or_else(|b| b).build();
    let mut loggers: Vec<Box<dyn SharedLogger>> =
        vec![TermLogger::new(level, config.clone(), TerminalMode::Mixed, ColorChoice::Auto)];
    if let Ok(file) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        loggers.push(WriteLogger::new(level, config, file));
    }
    let _ = CombinedLogger::init(loggers);
    std::panic::set_hook(Box::new(|info| log::error!("PANIC: {info}")));
    path
}

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let log_path = init_logging(&app.path().app_log_dir()?);
            log::info!("TSTV Phone Mics {} starting", env!("CARGO_PKG_VERSION"));

            let config_dir = app.path().app_config_dir()?;
            std::fs::create_dir_all(&config_dir)?;
            let settings_path = config_dir.join("settings.json");
            let settings = Settings::load(&settings_path);

            let engine = Arc::new(Engine::start(settings.device.clone()));
            engine.shared.buffer_ms.store(settings.buffer_ms);
            for (slot, s) in engine.shared.slots.iter().zip(&settings.slots) {
                slot.channel.store(s.channel, Relaxed);
                slot.gain_db.store(s.gain_db);
            }
            let desktop_name = format!("TSTV Phone Mics ({})", net::computer_name());
            let net = Net::start(engine.clone(), desktop_name, ACTIVE_SLOTS);

            let state = Arc::new(AppState { engine, net, settings: Mutex::new(settings), settings_path, log_path });
            app.manage(state.clone());
            let handle = app.handle().clone();
            std::thread::Builder::new().name("monitor".into()).spawn(move || monitor(handle, state))?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_setup,
            set_device,
            set_buffer_ms,
            set_channel,
            set_gain,
            set_mute,
            mute_all,
            set_tone
        ])
        .run(tauri::generate_context!())
        .expect("error while running TSTV Phone Mics");
}
