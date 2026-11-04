//! Audio output. Mixes every phone slot (or its test tone) into its chosen channel of the
//! output device. The audio callback never blocks or allocates; the UI and network talk to
//! it through atomics and a lock-free packet queue.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, SampleFormat, StreamConfig, SupportedBufferSize};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::playout::{Playout, State};
use crate::protocol::{self, Datagram, MAX_DATAGRAM, SAMPLE_RATE};

pub const MAX_SLOTS: usize = 5;
const TONE_HZ: f64 = 1000.0;
/// −18 dBFS, the usual line-up level.
const TONE_LEVEL: f32 = 0.125_89;
/// Largest number of frames processed in one go (bigger callbacks are split).
const MAX_BLOCK: usize = 4096;
/// Requested device buffer (frames). Small = low latency.
const DEVICE_BUFFER: u32 = 128;
/// Gain/mute changes glide over about this long, to avoid clicks.
const GAIN_GLIDE_S: f64 = 0.005;

/// f32 stored in an AtomicU32.
#[derive(Default)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    pub fn new(v: f32) -> Self {
        Self(AtomicU32::new(v.to_bits()))
    }
    pub fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Relaxed))
    }
    pub fn store(&self, v: f32) {
        self.0.store(v.to_bits(), Relaxed)
    }
    pub fn swap(&self, v: f32) -> f32 {
        f32::from_bits(self.0.swap(v.to_bits(), Relaxed))
    }
    /// Raise to `v` if larger. Only for non-negative values (their bit patterns sort like the floats).
    pub fn fetch_max(&self, v: f32) {
        self.0.fetch_max(v.to_bits(), Relaxed);
    }
}

/// One phone slot's controls and telemetry, shared between the UI, network and audio threads.
#[derive(Default)]
pub struct SlotShared {
    // Controls (UI → audio)
    /// 1-based output channel; 0 = none.
    pub channel: AtomicU32,
    pub gain_db: AtomicF32,
    pub operator_muted: AtomicBool,
    pub tone: AtomicBool,
    // Telemetry (audio → UI)
    /// Largest post-gain sample since the UI last read it.
    pub peak: AtomicF32,
    pub depth_ms: AtomicF32,
    /// 0 idle, 1 buffering, 2 playing.
    pub state: AtomicU8,
    pub talent_muted: AtomicBool,
    pub played: AtomicU64,
    pub concealed: AtomicU64,
    pub late: AtomicU64,
    pub recovered: AtomicU64,
    pub resyncs: AtomicU64,
}

#[derive(Default)]
pub struct EngineShared {
    pub slots: [SlotShared; MAX_SLOTS],
    pub buffer_ms: AtomicF32,
    pub output_latency_ms: AtomicF32,
    pub running: AtomicBool,
    pub device_channels: AtomicU32,
    pub device_rate: AtomicU32,
    stream_failed: AtomicBool,
    pub device_name: Mutex<String>,
    pub error: Mutex<Option<String>>,
}

/// Messages from the network thread to the audio thread.
pub enum ToAudio {
    Packet { slot: u8, len: u16, data: [u8; MAX_DATAGRAM] },
    /// A phone connected or left this slot: forget its audio.
    Reset { slot: u8 },
}

/// The audio thread's state. Lives behind a mutex only so it survives switching devices;
/// while a stream runs, the audio callback is the only thing that locks it.
pub struct AudioCore {
    rx: Consumer<ToAudio>,
    playouts: Vec<Playout>,
    gains: [f32; MAX_SLOTS],
    tone_phase: [f64; MAX_SLOTS],
    /// Playout clock in 48 kHz samples.
    clock: f64,
    scratch: Vec<f32>,
    shared: Arc<EngineShared>,
}

impl AudioCore {
    pub fn new(rx: Consumer<ToAudio>, shared: Arc<EngineShared>) -> Self {
        Self {
            rx,
            playouts: (0..MAX_SLOTS).map(|_| Playout::new()).collect(),
            gains: [0.0; MAX_SLOTS],
            tone_phase: [0.0; MAX_SLOTS],
            clock: 0.0,
            scratch: vec![0.0; MAX_BLOCK],
            shared,
        }
    }

    /// Fills one interleaved output buffer with `channels` channels at `device_rate`.
    pub fn render(&mut self, out: &mut [f32], channels: usize, device_rate: f64) {
        while let Ok(msg) = self.rx.pop() {
            match msg {
                ToAudio::Packet { slot, len, data } => {
                    if let Some(Datagram::Audio(pkt)) = protocol::parse(&data[..len as usize]) {
                        self.playouts[slot as usize].push(&pkt, self.clock);
                    }
                }
                ToAudio::Reset { slot } => self.playouts[slot as usize].reset(),
            }
        }
        for block in out.chunks_mut(MAX_BLOCK * channels) {
            self.render_block(block, channels, device_rate);
        }
        for (playout, sh) in self.playouts.iter().zip(&self.shared.slots) {
            sh.depth_ms.store((playout.depth_avg() / 48.0) as f32);
            sh.state.store(
                match playout.state() {
                    State::Idle => 0,
                    State::Buffering => 1,
                    State::Playing => 2,
                },
                Relaxed,
            );
            sh.talent_muted.store(playout.talent_muted(), Relaxed);
            let c = playout.counters;
            sh.played.store(c.played, Relaxed);
            sh.concealed.store(c.concealed, Relaxed);
            sh.late.store(c.late, Relaxed);
            sh.recovered.store(c.recovered, Relaxed);
            sh.resyncs.store(c.resyncs, Relaxed);
        }
    }

    fn render_block(&mut self, out: &mut [f32], channels: usize, device_rate: f64) {
        out.fill(0.0);
        let frames = out.len() / channels;
        let step = SAMPLE_RATE as f64 / device_rate;
        let target = self.shared.buffer_ms.load() as f64 * 48.0;
        let glide = (1.0 - (-1.0 / (GAIN_GLIDE_S * device_rate)).exp()) as f32;

        for s in 0..MAX_SLOTS {
            let sh = &self.shared.slots[s];
            let mono = &mut self.scratch[..frames];
            self.playouts[s].render(mono, step, target, self.clock);

            let tone = sh.tone.load(Relaxed);
            if tone {
                let inc = TONE_HZ / device_rate;
                let mut phase = self.tone_phase[s];
                for x in mono.iter_mut() {
                    *x = TONE_LEVEL * (phase * std::f64::consts::TAU).sin() as f32;
                    phase = (phase + inc).fract();
                }
                self.tone_phase[s] = phase;
            }

            // The tone ignores the gain fader so it always arrives at line-up level.
            let gain = if tone { 1.0 } else { db_to_gain(sh.gain_db.load()) };
            let wanted = if sh.operator_muted.load(Relaxed) { 0.0 } else { gain };
            let ch = sh.channel.load(Relaxed) as usize;
            let mut g = self.gains[s];
            let mut peak = 0f32;
            for (i, &x) in mono.iter().enumerate() {
                peak = peak.max((x * gain).abs());
                g += (wanted - g) * glide;
                if (1..=channels).contains(&ch) {
                    out[i * channels + ch - 1] += x * g;
                }
            }
            self.gains[s] = g;
            sh.peak.fetch_max(peak);
        }
        self.clock += frames as f64 * step;
    }
}

/// Fader position in dB to linear gain. −60 dB and below is off (−∞).
pub fn db_to_gain(db: f32) -> f32 {
    if db <= -60.0 { 0.0 } else { 10f32.powf(db / 20.0) }
}

#[derive(Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub name: String,
    pub channels: u16,
}

pub fn list_output_devices() -> Vec<DeviceInfo> {
    let host = cpal::default_host();
    let Ok(devices) = host.output_devices() else { return Vec::new() };
    devices
        .filter_map(|d| {
            let channels = d.supported_output_configs().ok()?.map(|c| c.channels()).max()?;
            Some(DeviceInfo { name: d.to_string(), channels })
        })
        .collect()
}

enum Command {
    Open(Option<String>),
}

/// Handle to the audio engine thread.
pub struct Engine {
    pub shared: Arc<EngineShared>,
    pub packets: Arc<Mutex<Producer<ToAudio>>>,
    commands: mpsc::Sender<Command>,
}

impl Engine {
    /// Starts the engine. `device` is the output device the operator picked, or `None` to
    /// use a StudioLive if one is connected, otherwise the system default output.
    pub fn start(device: Option<String>) -> Self {
        let (producer, consumer) = RingBuffer::new(1024);
        let shared = Arc::new(EngineShared::default());
        let core = Arc::new(Mutex::new(AudioCore::new(consumer, shared.clone())));
        let (tx, rx) = mpsc::channel();
        let thread_shared = shared.clone();
        std::thread::Builder::new()
            .name("audio-engine".into())
            .spawn(move || engine_thread(core, thread_shared, rx, device))
            .expect("spawn audio engine thread");
        Self { shared, packets: Arc::new(Mutex::new(producer)), commands: tx }
    }

    pub fn open_device(&self, device: Option<String>) {
        let _ = self.commands.send(Command::Open(device));
    }

    /// Queues an instruction for the audio thread (drops it if the queue is full).
    pub fn send(&self, msg: ToAudio) {
        let _ = self.packets.lock().unwrap().push(msg);
    }
}

/// Keeps an output stream open, reopening it if the device disappears or is changed.
fn engine_thread(
    core: Arc<Mutex<AudioCore>>,
    shared: Arc<EngineShared>,
    commands: mpsc::Receiver<Command>,
    mut wanted: Option<String>,
) {
    let mut stream: Option<cpal::Stream> = None;
    let mut retry_at = Instant::now();
    let mut last_error = String::new();
    loop {
        match commands.recv_timeout(Duration::from_millis(500)) {
            Ok(Command::Open(device)) => {
                wanted = device;
                stream = None;
                shared.running.store(false, Relaxed);
                retry_at = Instant::now();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        if shared.stream_failed.swap(false, Relaxed) {
            log::error!("Output stream failed; reopening");
            stream = None;
            shared.running.store(false, Relaxed);
            retry_at = Instant::now() + Duration::from_secs(1);
        }
        if stream.is_none() && Instant::now() >= retry_at {
            match open_stream(wanted.as_deref(), &core, &shared) {
                Ok(s) => {
                    stream = Some(s);
                    last_error.clear();
                    *shared.error.lock().unwrap() = None;
                }
                Err(e) => {
                    if e != last_error {
                        log::warn!("Can't open output: {e}");
                        last_error = e.clone();
                    }
                    *shared.error.lock().unwrap() = Some(e);
                    retry_at = Instant::now() + Duration::from_secs(2);
                }
            }
        }
    }
}

fn open_stream(wanted: Option<&str>, core: &Arc<Mutex<AudioCore>>, shared: &Arc<EngineShared>) -> Result<cpal::Stream, String> {
    let host = cpal::default_host();
    let devices: Vec<cpal::Device> = host.output_devices().map_err(|e| e.to_string())?.collect();
    let device = match wanted {
        // A device the operator picked: wait for it rather than play somewhere else.
        Some(name) => devices.into_iter().find(|d| d.to_string() == name).ok_or_else(|| format!("\"{name}\" is not connected"))?,
        None => match devices.into_iter().find(|d| d.to_string().contains("StudioLive")) {
            Some(d) => d,
            None => host.default_output_device().ok_or("No audio output device found")?,
        },
    };
    let name = device.to_string();

    let range = device
        .supported_output_configs()
        .map_err(|e| e.to_string())?
        .filter(|c| c.sample_format() == SampleFormat::F32)
        .max_by_key(|c| (c.channels(), c.min_sample_rate() <= SAMPLE_RATE && SAMPLE_RATE <= c.max_sample_rate()))
        .ok_or_else(|| format!("{name} has no usable output format"))?;
    let rate = if range.min_sample_rate() <= SAMPLE_RATE && SAMPLE_RATE <= range.max_sample_rate() {
        SAMPLE_RATE
    } else {
        device.default_output_config().map_err(|e| e.to_string())?.sample_rate()
    };
    let channels = range.channels();
    let buffer_size = match *range.buffer_size() {
        SupportedBufferSize::Range { min, max } if (min..=max).contains(&DEVICE_BUFFER) => BufferSize::Fixed(DEVICE_BUFFER),
        _ => BufferSize::Default,
    };
    let config = StreamConfig { channels, sample_rate: rate, buffer_size };

    let cb_core = core.clone();
    let cb_shared = shared.clone();
    let err_shared = shared.clone();
    let ch = channels as usize;
    let stream = device
        .build_output_stream::<f32, _, _>(
            config,
            move |data, info| {
                let ts = info.timestamp();
                let latency = ts.playback.saturating_duration_since(ts.callback);
                cb_shared.output_latency_ms.store(latency.as_secs_f32() * 1000.0);
                match cb_core.try_lock() {
                    Ok(mut core) => core.render(data, ch, rate as f64),
                    Err(_) => data.fill(0.0),
                }
            },
            move |err| {
                log::error!("Output stream error: {err}");
                err_shared.stream_failed.store(true, Relaxed);
            },
            None,
        )
        .map_err(|e| e.to_string())?;
    stream.play().map_err(|e| e.to_string())?;

    log::info!("Output open: {name}, {channels} ch, {rate} Hz, buffer {buffer_size:?}");
    *shared.device_name.lock().unwrap() = name;
    shared.device_channels.store(channels as u32, Relaxed);
    shared.device_rate.store(rate, Relaxed);
    shared.running.store(true, Relaxed);
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::playout::tests::FakePhone;

    fn core() -> (AudioCore, Producer<ToAudio>, Arc<EngineShared>) {
        let (producer, consumer) = RingBuffer::new(1024);
        let shared = Arc::new(EngineShared::default());
        shared.buffer_ms.store(30.0);
        for (i, s) in shared.slots.iter().enumerate() {
            s.channel.store(17 + i as u32, Relaxed);
        }
        (AudioCore::new(consumer, shared.clone()), producer, shared)
    }

    fn rms_per_channel(out: &[f32], channels: usize) -> Vec<f32> {
        let frames = out.len() / channels;
        (0..channels)
            .map(|c| ((0..frames).map(|i| out[i * channels + c].powi(2)).sum::<f32>() / frames as f32).sqrt())
            .collect()
    }

    #[test]
    fn test_tone_lands_only_on_its_channel() {
        let (mut core, _tx, shared) = core();
        shared.slots[0].tone.store(true, Relaxed);
        let mut out = vec![0f32; 64 * 512];
        for _ in 0..10 {
            core.render(&mut out, 64, 48_000.0);
        }
        let rms = rms_per_channel(&out, 64);
        // Channel 17 is index 16. A sine at −18 dBFS peak has RMS ≈ 0.089.
        assert!((rms[16] - 0.089).abs() < 0.005, "ch17 rms {}", rms[16]);
        for (i, r) in rms.iter().enumerate() {
            if i != 16 {
                assert_eq!(*r, 0.0, "channel {} not silent", i + 1);
            }
        }
        let peak = shared.slots[0].peak.load();
        assert!((peak - TONE_LEVEL).abs() < 0.01, "peak {peak}");
    }

    #[test]
    fn phone_audio_follows_channel_gain_and_mute() {
        let (mut core, mut tx, shared) = core();
        shared.slots[0].channel.store(21, Relaxed);
        let mut phone = FakePhone::new(1000.0, 0);
        let mut out = vec![0f32; 64 * 480];
        fn run(core: &mut AudioCore, tx: &mut Producer<ToAudio>, phone: &mut FakePhone, out: &mut [f32], frames: usize) {
            for _ in 0..frames {
                let raw = phone.next_packet();
                let mut data = [0u8; MAX_DATAGRAM];
                data[..raw.len()].copy_from_slice(&raw);
                tx.push(ToAudio::Packet { slot: 0, len: raw.len() as u16, data }).ok().unwrap();
                core.render(out, 64, 48_000.0);
            }
        }
        run(&mut core, &mut tx, &mut phone, &mut out, 100);
        let rms = rms_per_channel(&out, 64);
        assert!((rms[20] - 0.354).abs() < 0.04, "ch21 rms {}", rms[20]);
        assert!(rms.iter().enumerate().all(|(i, r)| i == 20 || *r == 0.0));
        assert_eq!(shared.slots[0].state.load(Relaxed), 2);

        shared.slots[0].gain_db.store(-6.0);
        run(&mut core, &mut tx, &mut phone, &mut out, 20);
        let rms = rms_per_channel(&out, 64);
        assert!((rms[20] - 0.354 * 0.501).abs() < 0.03, "-6 dB rms {}", rms[20]);

        shared.slots[0].operator_muted.store(true, Relaxed);
        run(&mut core, &mut tx, &mut phone, &mut out, 20);
        assert!(rms_per_channel(&out, 64)[20] < 1e-4);
        // The meter keeps showing the phone while muted.
        assert!(shared.slots[0].peak.load() > 0.1);
    }

    #[test]
    fn five_phones_each_land_on_their_own_channel_in_real_time() {
        let (mut core, mut tx, _shared) = core();
        let mut phones: Vec<FakePhone> =
            (0..MAX_SLOTS).map(|i| FakePhone::new(300.0 + 200.0 * i as f64, i as u32 * 1000)).collect();
        let mut out = vec![0f32; 64 * 480];
        let start = Instant::now();
        let blocks = 200;
        for _ in 0..blocks {
            for (slot, phone) in phones.iter_mut().enumerate() {
                let raw = phone.next_packet();
                let mut data = [0u8; MAX_DATAGRAM];
                data[..raw.len()].copy_from_slice(&raw);
                tx.push(ToAudio::Packet { slot: slot as u8, len: raw.len() as u16, data }).ok().unwrap();
            }
            core.render(&mut out, 64, 48_000.0);
        }
        let elapsed = start.elapsed();
        let rms = rms_per_channel(&out, 64);
        for (i, r) in rms.iter().enumerate() {
            if (16..21).contains(&i) {
                assert!((r - 0.354).abs() < 0.05, "channel {} rms {r}", i + 1);
            } else {
                assert_eq!(*r, 0.0, "channel {} not silent", i + 1);
            }
        }
        // 200 blocks are 2 s of audio; this time also includes encoding all five phones.
        assert!(elapsed < Duration::from_millis(1000), "took {elapsed:?} for 2 s of audio");
    }

    #[test]
    fn channel_beyond_device_is_ignored() {
        let (mut core, _tx, shared) = core();
        shared.slots[0].tone.store(true, Relaxed);
        let mut out = vec![0f32; 2 * 256];
        core.render(&mut out, 2, 48_000.0);
        assert!(out.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn fader_law() {
        assert_eq!(db_to_gain(-60.0), 0.0);
        assert_eq!(db_to_gain(0.0), 1.0);
        assert!((db_to_gain(12.0) - 3.98).abs() < 0.01);
    }
}
