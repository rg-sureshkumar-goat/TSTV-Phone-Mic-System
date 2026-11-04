package org.tstvnews.phonemic

import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.SystemClock
import android.util.Log
import java.net.ConnectException
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.net.InetSocketAddress
import java.net.NoRouteToHostException
import java.net.SocketTimeoutException
import java.net.UnknownHostException
import java.util.concurrent.Executors
import java.util.concurrent.ScheduledFuture
import java.util.concurrent.TimeUnit
import okhttp3.Dns
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.Response
import okhttp3.WebSocket
import okhttp3.WebSocketListener

/** Where to connect: a desktop found by discovery, an address typed in, or a QR code's addresses. */
sealed class DesktopTarget {
    abstract val label: String
    data class Service(val info: NsdServiceInfo) : DesktopTarget() { override val label: String get() = info.serviceName }
    data class Host(val host: String) : DesktopTarget() { override val label: String get() = host }
    data class Named(val name: String, val hosts: List<String>) : DesktopTarget() { override val label: String get() = name }
    /** An address from an earlier successful connection. */
    data class Address(val address: InetAddress) : DesktopTarget() { override val label: String get() = address.hostAddress ?: "desktop" }
}

/** Talks to the desktop app: WebSocket control channel plus UDP audio (shared/PROTOCOL.md). */
class StreamClient(private val nsd: NsdManager) {
    sealed class Event {
        data class Connected(val welcome: DesktopMessage.Welcome, val address: InetAddress) : Event()
        data class Rejected(val reason: String, val message: String) : Event()
        data class OperatorMuted(val muted: Boolean) : Event()
        data class Stats(val latencyMs: Double, val lossPct: Double, val channel: Int) : Event()
        data class Closed(val reason: String?) : Event()
    }

    /** Called on the network thread. */
    var onEvent: ((Event) -> Unit)? = null

    private val net = Executors.newSingleThreadScheduledExecutor { Thread(it, "network") }
    private var session = 0
    private var webSocket: WebSocket? = null
    private var address: InetAddress? = null
    private var welcomed = false
    private var timeout: ScheduledFuture<*>? = null

    // Audio sender state: used from the capture thread.
    private val lock = Any()
    private var audio: DatagramSocket? = null
    private var token: Int? = null
    private var encoder: OpusEncoder? = null
    private var seq = (Math.random() * Int.MAX_VALUE).toInt()
    private var previous = ByteArray(0)
    private var lastFrameAt = 0L
    private var talentMuted = false

    @Volatile var encoderDelayMs = 2.5
        private set

    fun start(target: DesktopTarget, hello: String) = net.execute {
        teardown()
        session += 1
        welcomed = false
        val mine = session
        // Some failures never report an error, so time the attempt out and let the app retry.
        timeout = net.schedule({
            if (mine == session && !welcomed) finish(mine, NOT_ANSWERING)
        }, CONNECT_TIMEOUT_S, TimeUnit.SECONDS)
        when (target) {
            is DesktopTarget.Address -> openControl(target.address, hello, mine)
            is DesktopTarget.Host -> lookUp(target.host, hello, mine)
            is DesktopTarget.Named -> lookUp(target.hosts.first(), hello, mine)
            is DesktopTarget.Service -> resolve(target.info, hello, mine)
        }
    }

    fun stop() = net.execute {
        session += 1
        teardown()
    }

    fun setTalentMuted(muted: Boolean) = synchronized(lock) { talentMuted = muted }

    fun send(json: String) = net.execute { webSocket?.send(json) }

    /** Capture thread: encodes and sends one 10 ms frame. Does nothing until connected. */
    fun sendFrame(pcm: FloatArray) {
        val packet: ByteArray
        val socket: DatagramSocket
        synchronized(lock) {
            socket = audio ?: return
            val token = token ?: return
            val encoder = encoder ?: return
            // If capture paused, skip sequence numbers for the gap so the desktop keeps the timing right.
            val now = SystemClock.elapsedRealtime()
            if (lastFrameAt != 0L && now > lastFrameAt + 100) seq += ((now - lastFrameAt) / 10).coerceAtMost(10_000).toInt() - 1
            lastFrameAt = now
            val primary = if (talentMuted) ByteArray(0) else encoder.encode(pcm) ?: ByteArray(0)
            packet = Wire.audioPacket(token, seq, talentMuted, primary, previous)
            previous = primary
            seq += 1
        }
        try {
            socket.send(DatagramPacket(packet, packet.size))
        } catch (e: Exception) {
            Log.d(TAG, "Audio send failed: $e")
        }
    }

    private fun lookUp(host: String, hello: String, mine: Int) {
        Thread({
            try {
                val found = InetAddress.getByName(host)
                net.execute { if (mine == session) openControl(found, hello, mine) }
            } catch (_: UnknownHostException) {
                net.execute { finish(mine, "\"$host\" isn't a valid address.") }
            }
        }, "lookup").start()
    }

    @Suppress("DEPRECATION")
    private fun resolve(info: NsdServiceInfo, hello: String, mine: Int) {
        nsd.resolveService(info, object : NsdManager.ResolveListener {
            override fun onResolveFailed(serviceInfo: NsdServiceInfo, errorCode: Int) {
                net.execute { finish(mine, NOT_ANSWERING) }
            }
            override fun onServiceResolved(serviceInfo: NsdServiceInfo) {
                val host = serviceInfo.host
                net.execute {
                    if (mine != session) return@execute
                    if (host == null) finish(mine, NOT_ANSWERING) else openControl(host, hello, mine)
                }
            }
        })
    }

    private fun openControl(target: InetAddress, hello: String, mine: Int) {
        address = target
        // A placeholder host name mapped straight to the address: this handles IPv6 (including
        // link-local addresses with an interface) without having to write it in a URL.
        val client = OkHttpClient.Builder()
            .dns(object : Dns { override fun lookup(hostname: String) = listOf(target) })
            .connectTimeout(5, TimeUnit.SECONDS)
            .build()
        val request = Request.Builder().url("ws://tstv-desktop:${Wire.CONTROL_PORT}/").build()
        webSocket = client.newWebSocket(request, object : WebSocketListener() {
            override fun onOpen(webSocket: WebSocket, response: Response) {
                net.execute { if (mine == session) webSocket.send(hello) }
            }
            override fun onMessage(webSocket: WebSocket, text: String) {
                val msg = DesktopMessage.parse(text) ?: return
                net.execute { if (mine == session) handle(msg, target) }
            }
            override fun onClosing(webSocket: WebSocket, code: Int, reason: String) {
                net.execute { finish(mine, "The desktop app closed the connection.") }
            }
            override fun onFailure(webSocket: WebSocket, t: Throwable, response: Response?) {
                net.execute { finish(mine, explain(t, target, welcomedOnly = mine == session && welcomed)) }
            }
        })
    }

    private fun handle(msg: DesktopMessage, target: InetAddress) {
        when (msg) {
            is DesktopMessage.Welcome -> {
                welcomed = true
                timeout?.cancel(false)
                openAudio(msg, target)
                onEvent?.invoke(Event.Connected(msg, target))
            }
            is DesktopMessage.Reject -> {
                onEvent?.invoke(Event.Rejected(msg.reason, msg.message))
                session += 1
                teardown()
            }
            is DesktopMessage.OperatorMuted -> onEvent?.invoke(Event.OperatorMuted(msg.muted))
            is DesktopMessage.Stats -> onEvent?.invoke(Event.Stats(msg.latencyMs, msg.lossPct, msg.channel))
        }
    }

    private fun openAudio(welcome: DesktopMessage.Welcome, target: InetAddress) {
        val socket = DatagramSocket()
        // DSCP EF: puts the packets in the Wi-Fi voice queue on routers that support it.
        socket.trafficClass = 0xB8
        socket.connect(InetSocketAddress(target, welcome.udpPort))
        val encoder = OpusEncoder(welcome.bitrate)
        encoderDelayMs = encoder.lookaheadMs
        synchronized(lock) {
            audio = socket
            token = welcome.token
            this.encoder = encoder
            previous = ByteArray(0)
            lastFrameAt = 0
        }
        // Answer the desktop's pings until this socket closes.
        Thread({
            val buffer = ByteArray(64)
            val packet = DatagramPacket(buffer, buffer.size)
            try {
                while (true) {
                    packet.length = buffer.size
                    socket.receive(packet)
                    Wire.pongFor(buffer, packet.length)?.let { socket.send(DatagramPacket(it, it.size)) }
                }
            } catch (_: Exception) {
                // Closed.
            }
        }, "pings").start()
    }

    /** Ends session [mine] once, reporting why. */
    private fun finish(mine: Int, reason: String?) {
        if (mine != session) return
        session += 1
        teardown()
        onEvent?.invoke(Event.Closed(reason))
    }

    private fun teardown() {
        timeout?.cancel(false)
        webSocket?.cancel()
        webSocket = null
        synchronized(lock) {
            audio?.close()
            audio = null
            token = null
            encoder?.close()
            encoder = null
        }
    }

    /** Turns a network error into something the talent or TD can act on. */
    private fun explain(error: Throwable, target: InetAddress, welcomedOnly: Boolean): String {
        if (welcomedOnly) return "The connection to the desktop was lost."
        val desktop = "the desktop at ${target.hostAddress}"
        return when (error) {
            is ConnectException -> "Nothing is answering at $desktop. Is the desktop app open, and is this still its address?"
            is SocketTimeoutException, is NoRouteToHostException ->
                "Can't reach $desktop. This network may be stopping devices from talking to each other (common on campus Wi-Fi)."
            else -> "Can't reach $desktop ($error)."
        }
    }

    private companion object {
        const val TAG = "TSTVMic"
        const val CONNECT_TIMEOUT_S = 6L
        const val NOT_ANSWERING = "The desktop didn't answer. If it's open, this network may be stopping devices from talking to each other (common on campus Wi-Fi)."
    }
}

