package org.tstvnews.phonemic

/** Mono 48 kHz Opus encoder in low-delay mode (10 ms frames). Use from one thread at a time. */
class OpusEncoder(bitrate: Int) {
    private var handle = nativeCreate(bitrate)
    private val output = ByteArray(1500)

    /** The encoder's built-in delay in milliseconds. */
    val lookaheadMs: Double get() = nativeLookahead(handle) * 1000.0 / Wire.SAMPLE_RATE

    /** Encodes exactly [Wire.FRAME_SAMPLES] samples. */
    fun encode(pcm: FloatArray): ByteArray? {
        val n = nativeEncode(handle, pcm, output)
        return if (n > 0) output.copyOf(n) else null
    }

    fun close() {
        if (handle != 0L) nativeDestroy(handle)
        handle = 0
    }

    private companion object {
        init { System.loadLibrary("opusjni") }
        @JvmStatic external fun nativeCreate(bitrate: Int): Long
        @JvmStatic external fun nativeLookahead(handle: Long): Int
        @JvmStatic external fun nativeEncode(handle: Long, pcm: FloatArray, out: ByteArray): Int
        @JvmStatic external fun nativeDestroy(handle: Long)
    }
}
