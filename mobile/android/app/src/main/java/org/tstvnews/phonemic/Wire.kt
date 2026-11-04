package org.tstvnews.phonemic

import java.net.URI
import java.net.URLDecoder
import java.nio.ByteBuffer
import org.json.JSONObject

/** Wire format shared with the desktop app. The spec is shared/PROTOCOL.md. */
object Wire {
    const val PROTOCOL_VERSION = 1
    const val CONTROL_PORT = 47800
    const val SERVICE_TYPE = "_tstvmic._tcp."
    const val SAMPLE_RATE = 48_000
    /** 10 ms at 48 kHz. */
    const val FRAME_SAMPLES = 480

    private const val VERSION: Byte = 1
    private const val KIND_AUDIO: Byte = 1
    private const val KIND_PING: Byte = 2
    private const val KIND_PONG: Byte = 3
    private const val CODEC_OPUS: Byte = 1
    private const val FLAG_TALENT_MUTED: Byte = 1

    fun audioPacket(token: Int, seq: Int, muted: Boolean, primary: ByteArray, redundant: ByteArray): ByteArray {
        // ByteBuffer is big-endian by default, as the protocol requires.
        val b = ByteBuffer.allocate(18 + primary.size + redundant.size)
        b.put('T'.code.toByte()).put('V'.code.toByte()).put(VERSION).put(KIND_AUDIO)
        b.putInt(token).putInt(seq)
        b.put(if (muted) FLAG_TALENT_MUTED else 0).put(CODEC_OPUS)
        b.putShort(primary.size.toShort()).put(primary)
        b.putShort(redundant.size.toShort()).put(redundant)
        return b.array()
    }

    /** If this datagram is a ping from the desktop, returns the matching pong. */
    fun pongFor(data: ByteArray, length: Int): ByteArray? {
        if (length < 16 || data[0] != 'T'.code.toByte() || data[1] != 'V'.code.toByte() ||
            data[2] != VERSION || data[3] != KIND_PING
        ) return null
        return data.copyOf(16).also { it[3] = KIND_PONG }
    }

    fun hello(deviceId: String, name: String, model: String, appVersion: String, code: String?): String =
        JSONObject()
            .put("type", "hello").put("protocol", PROTOCOL_VERSION)
            .put("deviceId", deviceId).put("name", name)
            .put("platform", "android").put("model", model).put("appVersion", appVersion)
            .apply { if (!code.isNullOrEmpty()) put("code", code) }
            .toString()

    fun status(
        talentMuted: Boolean,
        mic: Mic?,
        captureLatencyMs: Double,
        battery: Double?,
        interrupted: Boolean,
    ): String = JSONObject()
        .put("type", "status").put("talentMuted", talentMuted)
        .apply { if (mic != null) put("input", JSONObject().put("name", mic.name).put("kind", mic.kind)) }
        .put("captureLatencyMs", captureLatencyMs).put("inputSampleRate", SAMPLE_RATE)
        .put("battery", battery ?: JSONObject.NULL).put("interrupted", interrupted)
        .toString()
}

/** A microphone the talent can pick. `kind` is a protocol value: builtIn, bluetooth, wired, other. */
data class Mic(val id: Int, val name: String, val kind: String, val type: Int)

sealed class DesktopMessage {
    data class Welcome(val slot: Int, val channel: Int, val token: Int, val udpPort: Int, val bitrate: Int, val desktopName: String) : DesktopMessage()
    /** `reason` is "code" when the join code is missing or wrong. */
    data class Reject(val reason: String, val message: String) : DesktopMessage()
    data class OperatorMuted(val muted: Boolean) : DesktopMessage()
    data class Stats(val latencyMs: Double, val lossPct: Double, val channel: Int) : DesktopMessage()

    companion object {
        fun parse(text: String): DesktopMessage? {
            val j = try { JSONObject(text) } catch (_: Exception) { return null }
            return when (j.optString("type")) {
                "welcome" -> Welcome(
                    j.optInt("slot"), j.optInt("channel"),
                    // The token is an unsigned 32-bit number; keep its bits.
                    j.optLong("token").toInt(),
                    j.optInt("udpPort"), j.optInt("bitrate", 128_000), j.optString("desktopName"),
                )
                "reject" -> Reject(j.optString("reason"), j.optString("message", "The desktop app refused the connection."))
                "operator" -> OperatorMuted(j.optBoolean("operatorMuted"))
                "stats" -> Stats(j.optDouble("latencyMs", 0.0), j.optDouble("lossPct", 0.0), j.optInt("channel"))
                else -> null
            }
        }
    }
}

/** The link in the desktop's QR code: tstvmic://join?host=…&host=…&name=…[&code=…] */
data class JoinLink(val hosts: List<String>, val name: String, val code: String?) {
    companion object {
        fun parse(link: String): JoinLink? {
            val uri = try { URI(link) } catch (_: Exception) { return null }
            if (uri.scheme != "tstvmic" || uri.host != "join") return null
            val items = (uri.rawQuery ?: "").split('&').mapNotNull { item ->
                val at = item.indexOf('=')
                if (at < 0) null else item.substring(0, at) to URLDecoder.decode(item.substring(at + 1), "UTF-8")
            }
            val hosts = items.filter { it.first == "host" }.map { it.second }
            if (hosts.isEmpty()) return null
            return JoinLink(hosts, items.firstOrNull { it.first == "name" }?.second ?: "Desktop", items.firstOrNull { it.first == "code" }?.second)
        }
    }
}
