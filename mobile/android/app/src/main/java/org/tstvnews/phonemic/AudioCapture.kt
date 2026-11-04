package org.tstvnews.phonemic

import android.annotation.SuppressLint
import android.content.Context
import android.media.AudioDeviceCallback
import android.media.AudioDeviceInfo
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioRecord
import android.media.AudioRecordingConfiguration
import android.media.MediaRecorder
import android.media.audiofx.AutomaticGainControl
import android.media.audiofx.NoiseSuppressor
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.Process
import android.util.Log
import java.util.concurrent.Executor
import java.util.concurrent.atomic.AtomicInteger
import java.util.concurrent.atomic.AtomicLong

/**
 * Captures the chosen microphone as 48 kHz mono and hands out 10 ms frames on a capture thread.
 * Android converts Bluetooth mics to the requested rate itself. By default there's no echo
 * cancellation, noise suppression or automatic gain: the board's Fat Channel does the processing.
 */
class AudioCapture(context: Context, private val onFrame: (FloatArray) -> Unit) {
    /** Called on the main thread after the mics or the interruption state change. */
    var onChange: (() -> Unit)? = null
    var noiseSuppression = false
    var autoGain = false

    var isRunning = false
        private set
    var isInterrupted = false
        private set

    private val audio = context.getSystemService(AudioManager::class.java)
    private val main = Handler(Looper.getMainLooper())
    private var preferredMicId: Int? = null
    private var worker: Worker? = null
    private val peakBits = AtomicInteger(0)
    private val framesEmitted = AtomicLong(0)
    private var lastSeenFrames = 0L
    private var silentChecks = 0
    private var bufferMs = 20.0

    private val deviceCallback = object : AudioDeviceCallback() {
        override fun onAudioDevicesAdded(added: Array<out AudioDeviceInfo>) { onChange?.invoke() }
        override fun onAudioDevicesRemoved(removed: Array<out AudioDeviceInfo>) {
            // If the chosen mic went away, fall back to the system's choice.
            if (removed.any { it.id == preferredMicId }) selectMic(null)
            onChange?.invoke()
        }
    }

    fun start() {
        if (isRunning) return
        startWorker()
        audio.registerAudioDeviceCallback(deviceCallback, main)
        isRunning = true
    }

    fun stop() {
        if (!isRunning) return
        isRunning = false
        audio.unregisterAudioDeviceCallback(deviceCallback)
        worker?.finish()
        worker = null
        routeBluetooth(false)
    }

    /** Re-applies settings (e.g. after a processing switch changes). */
    fun reconfigure() {
        if (!isRunning) return
        worker?.finish()
        startWorker()
        onChange?.invoke()
    }

    fun selectMic(id: Int?) {
        preferredMicId = id
        // A Bluetooth mic needs the phone's call-audio link open before it delivers sound.
        routeBluetooth(availableMics.firstOrNull { it.id == id }?.kind == "bluetooth")
        reconfigure()
    }

    val availableMics: List<Mic>
        get() {
            val all = audio.getDevices(AudioManager.GET_DEVICES_INPUTS).mapNotNull { d ->
                val kind = when (d.type) {
                    AudioDeviceInfo.TYPE_BUILTIN_MIC -> "builtIn"
                    AudioDeviceInfo.TYPE_BLUETOOTH_SCO, AudioDeviceInfo.TYPE_BLE_HEADSET -> "bluetooth"
                    AudioDeviceInfo.TYPE_WIRED_HEADSET, AudioDeviceInfo.TYPE_USB_DEVICE, AudioDeviceInfo.TYPE_USB_HEADSET -> "wired"
                    else -> return@mapNotNull null
                }
                val name = if (kind == "builtIn") "Phone mic" else d.productName?.toString()?.ifBlank { null } ?: "External mic"
                Mic(d.id, name, kind, d.type)
            }
            // Phones list several built-in mics; offer just one.
            return all.distinctBy { if (it.kind == "builtIn") "builtIn" else it.id.toString() }
        }

    val currentMic: Mic?
        get() = availableMics.firstOrNull { it.id == preferredMicId } ?: availableMics.firstOrNull { it.kind == "builtIn" }

    /** Phone-side delay: OS buffering plus one 10 ms frame. An estimate; Android doesn't report it exactly. */
    val captureLatencyMs: Double get() = bufferMs + 10

    /** Peak level since the last call, 0…1. */
    fun takePeak(): Float = java.lang.Float.intBitsToFloat(peakBits.getAndSet(0))

    /**
     * Call about once a second. Returns false if no audio has arrived for a couple of seconds,
     * and keeps trying to restart capture until it does.
     */
    fun checkAudioFlowing(): Boolean {
        val frames = framesEmitted.get()
        val same = frames == lastSeenFrames
        lastSeenFrames = frames
        if (!isRunning || isInterrupted || !same) {
            silentChecks = 0
            return true
        }
        silentChecks += 1
        if (silentChecks % 3 == 0) {
            Log.w(TAG, "No audio for $silentChecks s, restarting capture")
            reconfigure()
        }
        return silentChecks < 2
    }

    private fun routeBluetooth(on: Boolean) {
        @Suppress("DEPRECATION")
        if (Build.VERSION.SDK_INT >= 31) {
            val device = audio.availableCommunicationDevices.firstOrNull {
                it.type == AudioDeviceInfo.TYPE_BLUETOOTH_SCO || it.type == AudioDeviceInfo.TYPE_BLE_HEADSET
            }
            if (on && device != null) {
                audio.mode = AudioManager.MODE_IN_COMMUNICATION
                audio.setCommunicationDevice(device)
            } else {
                audio.clearCommunicationDevice()
                audio.mode = AudioManager.MODE_NORMAL
            }
        } else if (on) {
            audio.mode = AudioManager.MODE_IN_COMMUNICATION
            audio.startBluetoothSco()
            audio.isBluetoothScoOn = true
        } else {
            audio.isBluetoothScoOn = false
            audio.stopBluetoothSco()
            audio.mode = AudioManager.MODE_NORMAL
        }
    }

    @SuppressLint("MissingPermission") // AppModel checks the permission before starting capture.
    private fun startWorker() {
        val unprocessed = audio.getProperty(AudioManager.PROPERTY_SUPPORT_AUDIO_SOURCE_UNPROCESSED) == "true"
        val source = if (unprocessed && !noiseSuppression && !autoGain) {
            MediaRecorder.AudioSource.UNPROCESSED
        } else {
            MediaRecorder.AudioSource.VOICE_RECOGNITION
        }
        val minBytes = AudioRecord.getMinBufferSize(Wire.SAMPLE_RATE, AudioFormat.CHANNEL_IN_MONO, AudioFormat.ENCODING_PCM_FLOAT)
        val bytes = maxOf(minBytes, Wire.FRAME_SAMPLES * 2 * Float.SIZE_BYTES)
        bufferMs = bytes.toDouble() / Float.SIZE_BYTES / Wire.SAMPLE_RATE * 1000 / 2
        val record = AudioRecord.Builder()
            .setAudioSource(source)
            .setAudioFormat(
                AudioFormat.Builder().setSampleRate(Wire.SAMPLE_RATE)
                    .setChannelMask(AudioFormat.CHANNEL_IN_MONO)
                    .setEncoding(AudioFormat.ENCODING_PCM_FLOAT).build(),
            )
            .setBufferSizeInBytes(bytes)
            .build()
        if (record.state != AudioRecord.STATE_INITIALIZED) {
            record.release()
            Log.e(TAG, "Couldn't open the microphone")
            return
        }
        audio.getDevices(AudioManager.GET_DEVICES_INPUTS).firstOrNull { it.id == preferredMicId }?.let { record.preferredDevice = it }
        val effects = buildList {
            if (noiseSuppression && NoiseSuppressor.isAvailable()) NoiseSuppressor.create(record.audioSessionId)?.let { it.enabled = true; add(it) }
            if (autoGain && AutomaticGainControl.isAvailable()) AutomaticGainControl.create(record.audioSessionId)?.let { it.enabled = true; add(it) }
        }
        // A phone call or another app taking the mic shows up as this recording being silenced.
        val callback = object : AudioManager.AudioRecordingCallback() {
            override fun onRecordingConfigChanged(configs: List<AudioRecordingConfiguration>) {
                if (Build.VERSION.SDK_INT < 29) return
                val silenced = configs.firstOrNull { it.clientAudioSessionId == record.audioSessionId }?.isClientSilenced ?: false
                if (silenced != isInterrupted) {
                    isInterrupted = silenced
                    onChange?.invoke()
                }
            }
        }
        record.registerAudioRecordingCallback(Executor { main.post(it) }, callback)
        worker = Worker(record, callback, effects).also { it.start() }
        Log.i(TAG, "Capture started: source $source, buffer $bytes bytes, mic ${currentMic?.name}")
    }

    private inner class Worker(
        private val record: AudioRecord,
        private val callback: AudioManager.AudioRecordingCallback,
        private val effects: List<android.media.audiofx.AudioEffect>,
    ) : Thread("capture") {
        @Volatile private var running = true

        override fun run() {
            Process.setThreadPriority(Process.THREAD_PRIORITY_URGENT_AUDIO)
            val frame = FloatArray(Wire.FRAME_SAMPLES)
            try {
                record.startRecording()
                while (running) {
                    var filled = 0
                    while (running && filled < frame.size) {
                        val n = record.read(frame, filled, frame.size - filled, AudioRecord.READ_BLOCKING)
                        if (n < 0) {
                            Log.e(TAG, "Microphone read failed: $n")
                            return
                        }
                        filled += n
                    }
                    if (filled < frame.size) return
                    var peak = 0f
                    for (s in frame) peak = maxOf(peak, kotlin.math.abs(s))
                    peakBits.accumulateAndGet(java.lang.Float.floatToIntBits(peak)) { a, b ->
                        if (java.lang.Float.intBitsToFloat(b) > java.lang.Float.intBitsToFloat(a)) b else a
                    }
                    framesEmitted.incrementAndGet()
                    onFrame(frame)
                }
            } catch (e: IllegalStateException) {
                Log.e(TAG, "Couldn't start the microphone", e)
            } finally {
                record.unregisterAudioRecordingCallback(callback)
                effects.forEach { it.release() }
                try { record.stop() } catch (_: IllegalStateException) {}
                record.release()
            }
        }

        fun finish() {
            running = false
            join(300)
        }
    }

    private companion object {
        const val TAG = "TSTVMic"
    }
}
