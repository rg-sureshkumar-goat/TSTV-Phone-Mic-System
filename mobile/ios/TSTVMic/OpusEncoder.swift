import COpus
import COpusShim
import Foundation

/// Mono 48 kHz Opus encoder in low-delay mode (10 ms frames). Not thread-safe:
/// use it from the capture thread only.
final class OpusEncoder {
    private let encoder: OpaquePointer
    private var output = [UInt8](repeating: 0, count: 1500)

    init?(bitrate: Int) {
        var error: Int32 = 0
        guard let enc = opus_encoder_create(Int32(Wire.sampleRate), 1, OPUS_APPLICATION_RESTRICTED_LOWDELAY, &error),
              error == OPUS_OK
        else { return nil }
        encoder = enc
        copus_encoder_set_int(enc, OPUS_SET_BITRATE_REQUEST, Int32(bitrate))
        copus_encoder_set_int(enc, OPUS_SET_COMPLEXITY_REQUEST, 9)
    }

    deinit {
        opus_encoder_destroy(encoder)
    }

    /// The encoder's built-in delay in milliseconds.
    var lookaheadMs: Double {
        var samples: Int32 = 0
        copus_encoder_get_int(encoder, OPUS_GET_LOOKAHEAD_REQUEST, &samples)
        return Double(samples) / Wire.sampleRate * 1000
    }

    /// Encodes exactly `Wire.frameSamples` samples.
    func encode(_ pcm: UnsafePointer<Float>) -> Data? {
        let n = output.withUnsafeMutableBufferPointer { out in
            opus_encode_float(encoder, pcm, Int32(Wire.frameSamples), out.baseAddress!, Int32(out.count))
        }
        guard n > 0 else { return nil }
        return Data(output[0..<Int(n)])
    }
}
