//! Minimal bindings to the vendored libopus, compiled by build.rs.

use std::os::raw::{c_int, c_uchar};

use crate::protocol::{FRAME_SAMPLES, SAMPLE_RATE};

#[repr(C)]
struct RawDecoder {
    _private: [u8; 0],
}

#[repr(C)]
struct RawEncoder {
    _private: [u8; 0],
}

const OPUS_OK: c_int = 0;
const OPUS_APPLICATION_RESTRICTED_LOWDELAY: c_int = 2051;
const OPUS_SET_BITRATE_REQUEST: c_int = 4002;
const OPUS_SET_COMPLEXITY_REQUEST: c_int = 4010;
const OPUS_GET_LOOKAHEAD_REQUEST: c_int = 4027;
const OPUS_RESET_STATE: c_int = 4028;

unsafe extern "C" {
    fn opus_decoder_create(fs: i32, channels: c_int, error: *mut c_int) -> *mut RawDecoder;
    fn opus_decoder_destroy(st: *mut RawDecoder);
    fn opus_decoder_ctl(st: *mut RawDecoder, request: c_int, ...) -> c_int;
    fn opus_decode_float(
        st: *mut RawDecoder,
        data: *const c_uchar,
        len: i32,
        pcm: *mut f32,
        frame_size: c_int,
        decode_fec: c_int,
    ) -> c_int;

    fn opus_encoder_create(fs: i32, channels: c_int, application: c_int, error: *mut c_int) -> *mut RawEncoder;
    fn opus_encoder_destroy(st: *mut RawEncoder);
    fn opus_encode_float(
        st: *mut RawEncoder,
        pcm: *const f32,
        frame_size: c_int,
        data: *mut c_uchar,
        max_data_bytes: i32,
    ) -> i32;
    fn opus_encoder_ctl(st: *mut RawEncoder, request: c_int, ...) -> c_int;
}

/// Mono 48 kHz Opus decoder.
pub struct Decoder {
    raw: *mut RawDecoder,
}

// The decoder is only ever used from one thread at a time.
unsafe impl Send for Decoder {}

impl Decoder {
    pub fn new() -> Result<Self, i32> {
        let mut err = 0;
        let raw = unsafe { opus_decoder_create(SAMPLE_RATE as i32, 1, &mut err) };
        if raw.is_null() || err != OPUS_OK {
            return Err(err);
        }
        Ok(Self { raw })
    }

    /// Forgets all decoder history (new phone session). Does not allocate.
    pub fn reset(&mut self) {
        unsafe { opus_decoder_ctl(self.raw, OPUS_RESET_STATE) };
    }

    /// Decodes one 10 ms frame into `out`. `None` asks Opus to conceal a lost frame.
    /// Returns false (and leaves silence) if the packet is corrupt.
    pub fn decode(&mut self, packet: Option<&[u8]>, out: &mut [f32; FRAME_SAMPLES]) -> bool {
        let (ptr, len) = match packet {
            Some(p) => (p.as_ptr(), p.len() as i32),
            None => (std::ptr::null(), 0),
        };
        let n = unsafe { opus_decode_float(self.raw, ptr, len, out.as_mut_ptr(), FRAME_SAMPLES as c_int, 0) };
        if n != FRAME_SAMPLES as c_int {
            out.fill(0.0);
            return false;
        }
        true
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe { opus_decoder_destroy(self.raw) }
    }
}

/// Mono 48 kHz low-delay Opus encoder. The phones do the real encoding; this is used by
/// the fake-phone test tool and the unit tests.
pub struct Encoder {
    raw: *mut RawEncoder,
}

unsafe impl Send for Encoder {}

impl Encoder {
    pub fn new(bitrate: i32) -> Result<Self, i32> {
        let mut err = 0;
        let raw = unsafe { opus_encoder_create(SAMPLE_RATE as i32, 1, OPUS_APPLICATION_RESTRICTED_LOWDELAY, &mut err) };
        if raw.is_null() || err != OPUS_OK {
            return Err(err);
        }
        unsafe {
            opus_encoder_ctl(raw, OPUS_SET_BITRATE_REQUEST, bitrate);
            opus_encoder_ctl(raw, OPUS_SET_COMPLEXITY_REQUEST, 9 as c_int);
        }
        Ok(Self { raw })
    }

    /// Algorithmic delay of the encoder, in samples.
    pub fn lookahead(&mut self) -> i32 {
        let mut value: i32 = 0;
        unsafe { opus_encoder_ctl(self.raw, OPUS_GET_LOOKAHEAD_REQUEST, &mut value as *mut i32) };
        value
    }

    /// Encodes one 10 ms frame. Returns the Opus packet length written into `out`.
    pub fn encode(&mut self, pcm: &[f32; FRAME_SAMPLES], out: &mut [u8]) -> Result<usize, i32> {
        let n = unsafe {
            opus_encode_float(self.raw, pcm.as_ptr(), FRAME_SAMPLES as c_int, out.as_mut_ptr(), out.len() as i32)
        };
        if n < 0 { Err(n) } else { Ok(n as usize) }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { opus_encoder_destroy(self.raw) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_survives_encode_decode() {
        let mut enc = Encoder::new(128_000).unwrap();
        let mut dec = Decoder::new().unwrap();
        let mut packet = [0u8; 1500];
        let mut out = [0f32; FRAME_SAMPLES];
        let mut energy = 0.0;
        for frame in 0..20 {
            let mut pcm = [0f32; FRAME_SAMPLES];
            for (i, s) in pcm.iter_mut().enumerate() {
                let t = (frame * FRAME_SAMPLES + i) as f32 / SAMPLE_RATE as f32;
                *s = 0.5 * (2.0 * std::f32::consts::PI * 1000.0 * t).sin();
            }
            let n = enc.encode(&pcm, &mut packet).unwrap();
            assert!(n > 10 && n < 400, "unexpected packet size {n}");
            assert!(dec.decode(Some(&packet[..n]), &mut out));
            if frame > 5 {
                energy += out.iter().map(|s| s * s).sum::<f32>();
            }
        }
        let rms = (energy / (14.0 * FRAME_SAMPLES as f32)).sqrt();
        assert!((rms - 0.354).abs() < 0.05, "rms {rms}");
        assert!(enc.lookahead() > 0);
    }
}
