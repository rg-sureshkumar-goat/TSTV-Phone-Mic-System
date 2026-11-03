//! Per-phone playout: jitter buffer, Opus decoding, loss concealment and clock-drift
//! correction. Runs on the audio thread, so nothing here allocates after `new()`.
//!
//! Timeline: frames are played strictly in sequence order, one every 10 ms of phone audio.
//! A frame that hasn't arrived when it's due is concealed and its packet, if it turns up
//! later, is dropped. That keeps the delay constant, which matters more for lip-sync than
//! saving a late packet.
//!
//! "Depth" is how much audio sits between the playhead and the newest frame received.
//! It is the delay this buffer adds. A small speed adjustment (at most 0.5 %, inaudible)
//! keeps the average depth at the operator's target, absorbing the difference between
//! the phone's and the mixer's sample clocks.

use crate::opus::Decoder;
use crate::protocol::{AudioPacket, FRAME_SAMPLES};

/// Frames held for reordering (1.28 s).
const RING: usize = 128;
const MAX_FRAME_BYTES: usize = 1276;
/// Lost frames in a row that are concealed (faded) before going silent.
const MAX_CONCEALED: u32 = 5;
/// No packets for this long (48 kHz samples) → stop and wait for the phone again.
const STALL_SAMPLES: f64 = 0.5 * 48_000.0;
/// This many late packets in a row means the phone's timeline jumped back; start over.
const LATE_RESYNC: u32 = 20;
/// Speed change per sample of depth error: 10 ms too deep → play 0.1 % faster.
const DRIFT_GAIN: f64 = 0.001 / FRAME_SAMPLES as f64;
const MAX_DRIFT: f64 = 0.005;
/// Averaging time for the depth estimate (48 kHz samples).
const DEPTH_TAU: f64 = 2.0 * 48_000.0;
/// If the average depth exceeds the target by more than this (or by the target itself,
/// whichever is larger), skip frames to get back to the target at once.
const MIN_CATCH_UP: f64 = 30.0 * 48.0;

#[derive(Clone, Copy)]
struct Slot {
    seq: u32,
    present: bool,
    muted: bool,
    from_redundancy: bool,
    len: u16,
    data: [u8; MAX_FRAME_BYTES],
}

const EMPTY_SLOT: Slot =
    Slot { seq: 0, present: false, muted: false, from_redundancy: false, len: 0, data: [0; MAX_FRAME_BYTES] };

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// No phone audio yet.
    Idle,
    /// Collecting audio until the target delay is reached.
    Buffering,
    Playing,
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Counters {
    /// Frames played (including concealed ones).
    pub played: u64,
    /// Frames that never arrived in time and were concealed.
    pub concealed: u64,
    /// Packets that arrived after their frame was due.
    pub late: u64,
    /// Lost frames rebuilt from the copy in the next packet.
    pub recovered: u64,
    /// Times the buffer had to start over.
    pub resyncs: u64,
}

pub struct Playout {
    decoder: Decoder,
    ring: Box<[Slot]>,
    state: State,
    next_seq: u32,
    newest_seq: u32,
    frame: [f32; FRAME_SAMPLES],
    frame_pos: usize,
    hist: [f32; 4],
    frac: f64,
    depth_avg: f64,
    last_packet_at: f64,
    missing_run: u32,
    late_run: u32,
    talent_muted: bool,
    pub counters: Counters,
}

impl Playout {
    pub fn new() -> Self {
        Self {
            decoder: Decoder::new().expect("Opus decoder"),
            ring: vec![EMPTY_SLOT; RING].into_boxed_slice(),
            state: State::Idle,
            next_seq: 0,
            newest_seq: 0,
            frame: [0.0; FRAME_SAMPLES],
            frame_pos: FRAME_SAMPLES,
            hist: [0.0; 4],
            frac: 0.0,
            depth_avg: 0.0,
            last_packet_at: 0.0,
            missing_run: 0,
            late_run: 0,
            talent_muted: false,
            counters: Counters::default(),
        }
    }

    /// Forget the current phone (disconnect or new session).
    pub fn reset(&mut self) {
        self.state = State::Idle;
        self.talent_muted = false;
        self.clear();
        self.decoder.reset();
    }

    pub fn state(&self) -> State {
        self.state
    }

    pub fn talent_muted(&self) -> bool {
        self.talent_muted
    }

    /// Current buffered audio in 48 kHz samples.
    pub fn depth(&self) -> f64 {
        let frames = (self.newest_seq.wrapping_add(1).wrapping_sub(self.next_seq) as i32).max(0);
        (FRAME_SAMPLES - self.frame_pos) as f64 + frames as f64 * FRAME_SAMPLES as f64
    }

    /// Averaged depth in 48 kHz samples (what the latency readout uses).
    pub fn depth_avg(&self) -> f64 {
        if self.state == State::Playing { self.depth_avg } else { self.depth() }
    }

    /// Accepts one packet from the phone. `now` is the playout clock in 48 kHz samples.
    pub fn push(&mut self, pkt: &AudioPacket, now: f64) {
        self.talent_muted = pkt.talent_muted();
        self.last_packet_at = now;
        if self.state == State::Idle {
            self.start_at(pkt.seq);
        }

        let ahead = pkt.seq.wrapping_sub(self.next_seq) as i32;
        if ahead < 0 {
            self.counters.late += 1;
            self.late_run += 1;
            if self.late_run < LATE_RESYNC {
                return;
            }
            self.counters.resyncs += 1;
            self.start_at(pkt.seq);
        } else if ahead as usize >= RING {
            self.counters.resyncs += 1;
            self.start_at(pkt.seq);
        }
        self.late_run = 0;

        self.store(pkt.seq, pkt.primary, pkt.talent_muted(), false);
        if !pkt.redundant.is_empty() {
            let prev = pkt.seq.wrapping_sub(1);
            if prev.wrapping_sub(self.next_seq) as i32 >= 0 {
                self.store(prev, pkt.redundant, false, true);
            }
        }
        if pkt.seq.wrapping_sub(self.newest_seq) as i32 > 0 {
            self.newest_seq = pkt.seq;
        }
    }

    /// Fills `out` at the output device's rate. `step` is 48 kHz samples per output sample
    /// (48000 / device rate), `target` the wanted delay in 48 kHz samples.
    pub fn render(&mut self, out: &mut [f32], step: f64, target: f64, now: f64) {
        if self.state != State::Idle && now - self.last_packet_at > STALL_SAMPLES {
            self.state = State::Idle;
            self.clear();
        }
        match self.state {
            State::Idle => {
                out.fill(0.0);
                return;
            }
            State::Buffering => {
                let mut depth = self.depth();
                if depth < target {
                    out.fill(0.0);
                    return;
                }
                // Start with exactly the target delay.
                while depth - FRAME_SAMPLES as f64 >= target {
                    self.skip_frame();
                    depth -= FRAME_SAMPLES as f64;
                }
                self.state = State::Playing;
                self.depth_avg = depth;
                self.hist = [0.0; 4];
                self.frac = 0.0;
            }
            State::Playing => {}
        }

        if self.depth_avg - target > target.max(MIN_CATCH_UP) {
            let mut depth = self.depth();
            while depth - FRAME_SAMPLES as f64 >= target {
                self.skip_frame();
                depth -= FRAME_SAMPLES as f64;
            }
            self.depth_avg = depth;
        }

        let speed = 1.0 + ((self.depth_avg - target) * DRIFT_GAIN).clamp(-MAX_DRIFT, MAX_DRIFT);
        let s = step * speed;
        for o in out.iter_mut() {
            self.frac += s;
            while self.frac >= 1.0 {
                self.frac -= 1.0;
                let x = self.pull();
                self.hist = [self.hist[1], self.hist[2], self.hist[3], x];
            }
            *o = hermite(self.hist, self.frac as f32);
        }

        let alpha = (out.len() as f64 * step / DEPTH_TAU).min(1.0);
        self.depth_avg += alpha * (self.depth() - self.depth_avg);
    }

    fn start_at(&mut self, seq: u32) {
        self.clear();
        self.next_seq = seq;
        self.newest_seq = seq.wrapping_sub(1);
        self.state = State::Buffering;
    }

    fn clear(&mut self) {
        for slot in self.ring.iter_mut() {
            slot.present = false;
        }
        self.frame_pos = FRAME_SAMPLES;
        self.missing_run = 0;
        self.late_run = 0;
    }

    fn store(&mut self, seq: u32, data: &[u8], muted: bool, from_redundancy: bool) {
        let slot = &mut self.ring[seq as usize % RING];
        if from_redundancy && slot.present && slot.seq == seq {
            return;
        }
        if data.len() > MAX_FRAME_BYTES {
            return;
        }
        slot.seq = seq;
        slot.present = true;
        slot.muted = muted || data.is_empty();
        slot.from_redundancy = from_redundancy;
        slot.len = data.len() as u16;
        slot.data[..data.len()].copy_from_slice(data);
    }

    fn skip_frame(&mut self) {
        let idx = self.next_seq as usize % RING;
        self.ring[idx].present = false;
        self.next_seq = self.next_seq.wrapping_add(1);
    }

    fn pull(&mut self) -> f32 {
        if self.frame_pos >= FRAME_SAMPLES {
            self.decode_next();
        }
        let s = self.frame[self.frame_pos];
        self.frame_pos += 1;
        s
    }

    fn decode_next(&mut self) {
        let idx = self.next_seq as usize % RING;
        let slot = &self.ring[idx];
        if slot.present && slot.seq == self.next_seq {
            if slot.muted {
                self.frame.fill(0.0);
            } else {
                let len = slot.len as usize;
                self.decoder.decode(Some(&self.ring[idx].data[..len]), &mut self.frame);
            }
            if self.ring[idx].from_redundancy {
                self.counters.recovered += 1;
            }
            self.ring[idx].present = false;
            self.missing_run = 0;
        } else {
            self.missing_run += 1;
            self.counters.concealed += 1;
            if self.missing_run <= MAX_CONCEALED {
                self.decoder.decode(None, &mut self.frame);
                // Fade out across consecutive concealed frames.
                let steps = (MAX_CONCEALED + 1) as f32;
                let from = 1.0 - (self.missing_run - 1) as f32 / steps;
                let to = 1.0 - self.missing_run as f32 / steps;
                for (i, s) in self.frame.iter_mut().enumerate() {
                    *s *= from + (to - from) * i as f32 / FRAME_SAMPLES as f32;
                }
            } else {
                self.frame.fill(0.0);
            }
        }
        self.counters.played += 1;
        self.next_seq = self.next_seq.wrapping_add(1);
        self.frame_pos = 0;
    }
}

impl Default for Playout {
    fn default() -> Self {
        Self::new()
    }
}

/// 4-point cubic Hermite interpolation between h[1] and h[2] at fraction t.
fn hermite(h: [f32; 4], t: f32) -> f32 {
    let c = (h[2] - h[0]) * 0.5;
    let v = h[1] - h[2];
    let w = c + v;
    let a = w + v + (h[3] - h[1]) * 0.5;
    let b = w + a;
    (((a * t) - b) * t + c) * t + h[1]
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::opus::Encoder;
    use crate::protocol::{self, Datagram, SAMPLE_RATE};

    /// Generates protocol-correct packets carrying a sine wave, like a phone would.
    pub(crate) struct FakePhone {
        enc: Encoder,
        pub seq: u32,
        phase: f64,
        freq: f64,
        prev: Vec<u8>,
        pub muted: bool,
    }

    impl FakePhone {
        pub fn new(freq: f64, first_seq: u32) -> Self {
            Self { enc: Encoder::new(128_000).unwrap(), seq: first_seq, phase: 0.0, freq, prev: Vec::new(), muted: false }
        }

        /// The next 10 ms packet, as raw datagram bytes.
        pub fn next_packet(&mut self) -> Vec<u8> {
            let mut pcm = [0f32; FRAME_SAMPLES];
            for s in pcm.iter_mut() {
                *s = (0.5 * (self.phase * std::f64::consts::TAU).sin()) as f32;
                self.phase = (self.phase + self.freq / SAMPLE_RATE as f64).fract();
            }
            let mut buf = [0u8; 1500];
            let primary = if self.muted {
                Vec::new()
            } else {
                let n = self.enc.encode(&pcm, &mut buf).unwrap();
                buf[..n].to_vec()
            };
            let mut out = Vec::new();
            let flags = if self.muted { protocol::FLAG_TALENT_MUTED } else { 0 };
            protocol::write_audio(&mut out, 1, self.seq, flags, &primary, &self.prev);
            self.prev = primary;
            self.seq = self.seq.wrapping_add(1);
            out
        }
    }

    fn push_raw(p: &mut Playout, raw: &[u8], now: f64) {
        let Some(Datagram::Audio(pkt)) = protocol::parse(raw) else { panic!("bad packet") };
        p.push(&pkt, now);
    }

    /// Deterministic pseudo-random numbers in [0, 1).
    struct Lcg(u64);
    impl Lcg {
        fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    struct SimResult {
        playout: Playout,
        /// (time in seconds, depth_avg in ms) every callback
        depths: Vec<(f64, f64)>,
        rms_last_second: f64,
    }

    /// Runs a phone → network → playout simulation.
    /// `clock_ratio` > 1 means the phone's clock runs fast. `jitter_ms` is uniform extra delay.
    /// `drop` decides per sequence number whether the packet is lost.
    fn simulate(
        seconds: f64,
        clock_ratio: f64,
        jitter_ms: f64,
        target_ms: f64,
        mut drop: impl FnMut(u32, &mut Lcg) -> bool,
    ) -> SimResult {
        let device_rate = 48_000.0;
        let block = 128usize;
        let mut rng = Lcg(42);
        let mut phone = FakePhone::new(1000.0, 1_000_000);
        let mut playout = Playout::new();

        // Pre-generate packets with arrival times (seconds).
        let mut arrivals: Vec<(f64, Vec<u8>)> = Vec::new();
        let frames = (seconds * 100.0 * clock_ratio) as usize + 10;
        for i in 0..frames {
            let seq = phone.seq;
            let raw = phone.next_packet();
            let sent = i as f64 * 0.010 / clock_ratio;
            let arrive = sent + 0.003 + rng.next() * jitter_ms / 1000.0;
            if !drop(seq, &mut rng) {
                arrivals.push((arrive, raw));
            }
        }
        arrivals.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

        let mut out = vec![0f32; block];
        let mut depths = Vec::new();
        let mut next = 0;
        let mut t = 0.0;
        let mut clock = 0.0;
        let mut energy = 0.0;
        let mut energy_n = 0;
        while t < seconds {
            while next < arrivals.len() && arrivals[next].0 <= t {
                push_raw(&mut playout, &arrivals[next].1, clock);
                next += 1;
            }
            playout.render(&mut out, 48_000.0 / device_rate, target_ms * 48.0, clock);
            depths.push((t, playout.depth_avg() / 48.0));
            if t > seconds - 1.0 {
                energy += out.iter().map(|s| (s * s) as f64).sum::<f64>();
                energy_n += out.len();
            }
            t += block as f64 / device_rate;
            clock += block as f64;
        }
        SimResult { playout, depths, rms_last_second: (energy / energy_n.max(1) as f64).sqrt() }
    }

    fn depth_range_after(r: &SimResult, from: f64) -> (f64, f64) {
        r.depths
            .iter()
            .filter(|(t, _)| *t >= from)
            .fold((f64::MAX, f64::MIN), |(lo, hi), (_, d)| (lo.min(*d), hi.max(*d)))
    }

    #[test]
    fn steady_stream_plays_at_target_delay() {
        let r = simulate(5.0, 1.0, 5.0, 40.0, |_, _| false);
        assert_eq!(r.playout.state(), State::Playing);
        assert_eq!(r.playout.counters.concealed, 0, "{:?}", r.playout.counters);
        let (lo, hi) = depth_range_after(&r, 2.0);
        assert!(lo > 34.0 && hi < 50.0, "depth {lo:.1}..{hi:.1} ms");
        // 0.5 amplitude sine → RMS ≈ 0.354
        assert!((r.rms_last_second - 0.354).abs() < 0.04, "rms {}", r.rms_last_second);
    }

    #[test]
    fn corrects_clock_drift_both_ways() {
        for ratio in [1.0003, 0.9997] {
            // Uncorrected, 300 ppm over 60 s would move the delay by 18 ms.
            let r = simulate(60.0, ratio, 4.0, 40.0, |_, _| false);
            let (lo, hi) = depth_range_after(&r, 10.0);
            assert!(lo > 33.0 && hi < 47.0, "ratio {ratio}: depth {lo:.1}..{hi:.1} ms");
            assert_eq!(r.playout.counters.concealed, 0, "ratio {ratio}: {:?}", r.playout.counters);
            assert_eq!(r.playout.counters.resyncs, 0);
        }
    }

    #[test]
    fn single_lost_packets_are_rebuilt_from_redundancy() {
        // Drop every 37th packet: never two in a row.
        let r = simulate(5.0, 1.0, 2.0, 30.0, |seq, _| seq % 37 == 0);
        assert!(r.playout.counters.recovered >= 10, "{:?}", r.playout.counters);
        assert_eq!(r.playout.counters.concealed, 0, "{:?}", r.playout.counters);
    }

    #[test]
    fn bursts_of_loss_are_concealed_not_fatal() {
        // Lose 4 packets in a row every second.
        let r = simulate(5.0, 1.0, 2.0, 30.0, |seq, _| seq % 100 < 4);
        assert!(r.playout.counters.concealed > 0);
        assert_eq!(r.playout.state(), State::Playing);
        assert!((r.rms_last_second - 0.354).abs() < 0.08, "rms {}", r.rms_last_second);
    }

    #[test]
    fn stall_goes_idle_then_recovers() {
        let mut phone = FakePhone::new(440.0, 7);
        let mut p = Playout::new();
        let mut out = vec![0f32; 480];
        let mut clock = 0.0;
        let target = 20.0 * 48.0;
        for _ in 0..100 {
            push_raw(&mut p, &phone.next_packet(), clock);
            p.render(&mut out, 1.0, target, clock);
            clock += 480.0;
        }
        assert_eq!(p.state(), State::Playing);
        // Phone goes quiet for 1 s.
        for _ in 0..100 {
            p.render(&mut out, 1.0, target, clock);
            clock += 480.0;
        }
        assert_eq!(p.state(), State::Idle);
        assert!(out.iter().all(|s| *s == 0.0));
        // It comes back with a jump in sequence numbers.
        phone.seq = phone.seq.wrapping_add(5000);
        for _ in 0..50 {
            push_raw(&mut p, &phone.next_packet(), clock);
            p.render(&mut out, 1.0, target, clock);
            clock += 480.0;
        }
        assert_eq!(p.state(), State::Playing);
        assert!(out.iter().any(|s| s.abs() > 0.1));
    }

    #[test]
    fn timeline_jumping_back_resyncs() {
        let mut phone = FakePhone::new(440.0, 500);
        let mut p = Playout::new();
        let mut out = vec![0f32; 480];
        let mut clock = 0.0;
        let target = 20.0 * 48.0;
        for _ in 0..100 {
            push_raw(&mut p, &phone.next_packet(), clock);
            p.render(&mut out, 1.0, target, clock);
            clock += 480.0;
        }
        // The phone restarts its counter 300 frames back (e.g. app restarted audio).
        phone.seq -= 300;
        for _ in 0..60 {
            push_raw(&mut p, &phone.next_packet(), clock);
            p.render(&mut out, 1.0, target, clock);
            clock += 480.0;
        }
        assert_eq!(p.counters.resyncs, 1);
        assert_eq!(p.state(), State::Playing);
        assert!(out.iter().any(|s| s.abs() > 0.1));
    }

    #[test]
    fn talent_mute_plays_silence() {
        let mut phone = FakePhone::new(440.0, 0);
        let mut p = Playout::new();
        let mut out = vec![0f32; 480];
        let mut clock = 0.0;
        phone.muted = true;
        for _ in 0..50 {
            push_raw(&mut p, &phone.next_packet(), clock);
            p.render(&mut out, 1.0, 960.0, clock);
            clock += 480.0;
        }
        assert!(p.talent_muted());
        assert_eq!(p.state(), State::Playing);
        assert!(out.iter().all(|s| s.abs() < 1e-6));
        assert_eq!(p.counters.concealed, 0);
    }

    #[test]
    fn hermite_passes_through_samples() {
        let h = [1.0, 2.0, 3.0, 4.0];
        assert_eq!(hermite(h, 0.0), 2.0);
        assert!((hermite(h, 0.5) - 2.5).abs() < 1e-6);
    }
}
