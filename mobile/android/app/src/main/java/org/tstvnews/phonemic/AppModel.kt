package org.tstvnews.phonemic

import android.app.Application
import android.content.Context
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.os.BatteryManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import java.net.InetAddress
import java.util.UUID
import kotlin.math.log10
import kotlin.math.max

/** App state and the glue between the microphone, the network and the screens. Main thread only. */
class AppModel(private val app: Application) {
    enum class Phase { Idle, Connecting, Connected, Reconnecting }

    private val prefs = app.getSharedPreferences("settings", Context.MODE_PRIVATE)
    private val main = Handler(Looper.getMainLooper())
    private val nsd = app.getSystemService(NsdManager::class.java)
    private val client = StreamClient(nsd)
    private val capture = AudioCapture(app) { client.sendFrame(it) }
    private val discovery = Discovery(nsd)
    private val deviceId = prefs.getString("deviceId", null) ?: UUID.randomUUID().toString().also { prefs.edit().putString("deviceId", it).apply() }

    var name by mutableStateOf(prefs.getString("name", "") ?: "")
        private set
    var manualHost by mutableStateOf(prefs.getString("manualHost", "") ?: "")
        private set
    var joinCode by mutableStateOf(prefs.getString("joinCode", "") ?: "")
        private set
    var noiseSuppression by mutableStateOf(prefs.getBoolean("noiseSuppression", false))
        private set
    var autoGain by mutableStateOf(prefs.getBoolean("autoGain", false))
        private set

    var phase by mutableStateOf(Phase.Idle)
        private set
    var target by mutableStateOf<DesktopTarget?>(null)
        private set
    var desktops by mutableStateOf<List<NsdServiceInfo>>(emptyList())
        private set
    var errorMessage by mutableStateOf<String?>(null)
    /** A desktop to join once the talent has entered what's missing (a name or a join code). */
    var pendingTarget by mutableStateOf<DesktopTarget?>(null)
        private set
    var needsCode by mutableStateOf(false)
        private set
    var talentMuted by mutableStateOf(false)
        private set
    var operatorMuted by mutableStateOf(false)
        private set
    var interrupted by mutableStateOf(false)
        private set
    /** Capture is supposed to be running but no audio is arriving from the mic. */
    var micStalled by mutableStateOf(false)
        private set
    var desktopName by mutableStateOf("")
        private set
    var slot by mutableStateOf(0)
        private set
    var channel by mutableStateOf(0)
        private set
    var latencyMs by mutableStateOf<Double?>(null)
        private set
    /** Mic level in dBFS for the meter. */
    var levelDb by mutableStateOf(-90f)
        private set
    var mics by mutableStateOf<List<Mic>>(emptyList())
        private set
    var currentMic by mutableStateOf<Mic?>(null)
        private set
    var showFocusTip by mutableStateOf(true)

    /** On air = connected, not muted by anyone, mic working. */
    val isLive: Boolean get() = phase == Phase.Connected && !talentMuted && !operatorMuted && !interrupted && !micStalled

    /** Set by the screen: runs the action once the permissions it needs are granted. */
    var withPermissions: (action: () -> Unit) -> Unit = { it() }

    private var wantsConnection = false
    private var lastAddress: InetAddress? = null
    private var reconnectAttempts = 0
    private var reconnect: Runnable? = null
    private var ticking = false
    private var lastStatus: String? = null
    private var lastStatusAt = 0L

    init {
        capture.noiseSuppression = noiseSuppression
        capture.autoGain = autoGain
        capture.onChange = { main.post { captureChanged() } }
        client.onEvent = { event -> main.post { handle(event) } }
        discovery.onChange = { desktops = it }
        discovery.start()
    }

    fun updateName(value: String) { name = value; prefs.edit().putString("name", value).apply() }
    fun updateManualHost(value: String) { manualHost = value; prefs.edit().putString("manualHost", value).apply() }
    fun updateJoinCode(value: String) { joinCode = value; prefs.edit().putString("joinCode", value).apply() }

    fun updateNoiseSuppression(on: Boolean) {
        noiseSuppression = on
        prefs.edit().putBoolean("noiseSuppression", on).apply()
        capture.noiseSuppression = on
        capture.reconfigure()
    }

    fun updateAutoGain(on: Boolean) {
        autoGain = on
        prefs.edit().putBoolean("autoGain", on).apply()
        capture.autoGain = on
        capture.reconfigure()
    }

    val canConnect: Boolean get() = name.isNotBlank()

    fun connect(to: DesktopTarget) {
        pendingTarget = null
        needsCode = false
        withPermissions { startConnection(to) }
    }

    fun connectToManualHost() {
        val host = manualHost.trim()
        if (host.isNotEmpty()) connect(DesktopTarget.Host(host))
    }

    fun connectToPending() { pendingTarget?.let { connect(it) } }

    /** A join link from the in-app scanner, or from the phone's camera app. */
    fun openLink(link: String) {
        val parsed = JoinLink.parse(link)
        if (parsed == null) {
            errorMessage = "That isn't a TSTV Phone Mics code."
            return
        }
        if (phase != Phase.Idle) disconnect()
        parsed.code?.let { updateJoinCode(it) }
        val target = DesktopTarget.Named(parsed.name, parsed.hosts)
        if (name.isBlank()) {
            pendingTarget = target
            needsCode = false
            errorMessage = "Type your name, then tap Connect."
        } else {
            connect(target)
        }
    }

    fun disconnect() {
        wantsConnection = false
        reconnect?.let { main.removeCallbacks(it) }
        client.stop()
        capture.stop()
        ticking = false
        micStalled = false
        phase = Phase.Idle
        operatorMuted = false
        latencyMs = null
        MicService.stop(app)
        discovery.start()
    }

    fun toggleMute() {
        talentMuted = !talentMuted
        client.setTalentMuted(talentMuted)
        sendStatus(force = true)
    }

    fun selectMic(mic: Mic) {
        capture.selectMic(mic.id)
        captureChanged()
    }

    private fun startConnection(to: DesktopTarget) {
        errorMessage = null
        MicService.start(app)
        capture.start()
        target = to
        lastAddress = null
        reconnectAttempts = 0
        wantsConnection = true
        phase = Phase.Connecting
        discovery.stop()
        client.setTalentMuted(talentMuted)
        client.start(to, hello())
        captureChanged()
        startTicking()
    }

    private fun hello() = Wire.hello(
        deviceId = deviceId,
        name = name.trim(),
        model = "${Build.MANUFACTURER} ${Build.MODEL}",
        appVersion = app.packageManager.getPackageInfo(app.packageName, 0).versionName ?: "?",
        code = joinCode,
    )

    private fun handle(event: StreamClient.Event) {
        when (event) {
            is StreamClient.Event.Connected -> {
                phase = Phase.Connected
                lastAddress = event.address
                reconnectAttempts = 0
                desktopName = event.welcome.desktopName
                slot = event.welcome.slot
                channel = event.welcome.channel
                errorMessage = null
                sendStatus(force = true)
            }
            is StreamClient.Event.Rejected -> {
                val rejected = target
                disconnect()
                errorMessage = event.message
                if (event.reason == "code") {
                    pendingTarget = rejected
                    needsCode = true
                }
            }
            is StreamClient.Event.OperatorMuted -> operatorMuted = event.muted
            is StreamClient.Event.Stats -> {
                latencyMs = event.latencyMs.takeIf { it > 0 }
                channel = event.channel
            }
            is StreamClient.Event.Closed -> {
                val target = target
                if (!wantsConnection || target == null) return
                phase = Phase.Reconnecting
                latencyMs = null
                event.reason?.let { errorMessage = it }
                // Cycle through the last known address (quick) and the original target (finds the
                // desktop again if its address changed), or each address from a QR code.
                reconnectAttempts += 1
                val options = mutableListOf<DesktopTarget>()
                lastAddress?.let { options += DesktopTarget.Address(it) }
                if (target is DesktopTarget.Named) target.hosts.forEach { options += DesktopTarget.Host(it) } else options += target
                val next = options[(reconnectAttempts - if (lastAddress == null) 0 else 1) % options.size]
                reconnect?.let { main.removeCallbacks(it) }
                reconnect = Runnable { if (wantsConnection) client.start(next, hello()) }.also { main.postDelayed(it, 2000) }
            }
        }
    }

    private fun startTicking() {
        if (ticking) return
        ticking = true
        var count = 0
        lateinit var tick: Runnable
        tick = Runnable {
            if (!ticking) return@Runnable
            updateMeter()
            count += 1
            if (count % 20 == 0) {
                micStalled = !capture.checkAudioFlowing()
                sendStatus(force = false)
            }
            main.postDelayed(tick, 50)
        }
        main.post(tick)
    }

    private fun updateMeter() {
        val peak = capture.takePeak()
        val db = if (peak > 0) 20 * log10(peak) else -90f
        // Rise instantly, fall at about 20 dB/s.
        levelDb = max(db, levelDb - 1)
    }

    private fun captureChanged() {
        mics = capture.availableMics
        currentMic = capture.currentMic
        interrupted = capture.isInterrupted
        sendStatus(force = false)
    }

    /** Sends a status message if something changed (or every 2 s). */
    private fun sendStatus(force: Boolean) {
        if (phase != Phase.Connected) return
        val percent = app.getSystemService(BatteryManager::class.java).getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY)
        val status = Wire.status(
            talentMuted = talentMuted,
            mic = currentMic,
            captureLatencyMs = capture.captureLatencyMs + client.encoderDelayMs,
            battery = if (percent in 0..100) percent / 100.0 else null,
            interrupted = interrupted,
        )
        val now = System.currentTimeMillis()
        if (force || status != lastStatus || now - lastStatusAt >= 2000) {
            client.send(status)
            lastStatus = status
            lastStatusAt = now
        }
    }
}
