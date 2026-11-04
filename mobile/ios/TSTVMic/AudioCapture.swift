import AVFoundation
import Synchronization

/// A microphone the talent can pick.
struct MicOption: Identifiable, Hashable {
    var id: String   // AVAudioSessionPortDescription.uid
    var name: String
    /// builtIn | bluetooth | wired | other (protocol values)
    var kind: String

    init(_ port: AVAudioSessionPortDescription) {
        id = port.uid
        name = port.portName
        switch port.portType {
        case .builtInMic: kind = "builtIn"
        case .bluetoothHFP, .bluetoothLE, .bluetoothA2DP: kind = "bluetooth"
        case .headsetMic, .usbAudio, .lineIn: kind = "wired"
        default: kind = "other"
        }
    }
}

/// Captures the chosen microphone, converts it to 48 kHz mono and hands out 10 ms frames
/// on a dedicated capture thread.
///
/// Signal path: hardware → AVAudioSinkNode (real-time thread, copies into a lock-free ring)
/// → capture thread (sample-rate conversion if needed, 480-sample frames) → `onFrame`.
/// The audio session uses measurement mode, so iOS applies no echo cancellation, noise
/// suppression or automatic gain: the board's Fat Channel does the processing.
final class AudioCapture: @unchecked Sendable {
    /// Called on the capture thread with each 10 ms frame (480 samples, 48 kHz).
    var onFrame: ((UnsafePointer<Float>) -> Void)?
    /// Called on the main thread after the route, format or interruption state changes.
    var onChange: (() -> Void)?

    /// Use the iOS 26+ full-bandwidth Bluetooth mode (AirPods Pro 2 / AirPods 4 and newer).
    var bluetoothHighQuality = false
    /// Mic the talent picked (port uid), or nil for the system's choice.
    var preferredMicID: String?

    private(set) var isRunning = false
    private(set) var isInterrupted = false
    /// Hardware sample rate of the current mic route.
    private(set) var inputSampleRate: Double = 0

    private var engine = AVAudioEngine()
    private var sink: AVAudioSinkNode?
    private let ring = SampleRing(capacity: 96_000)
    private let wake = DispatchSemaphore(value: 0)
    /// Bumped to stop the current capture thread (each thread runs while it matches).
    private let workerGeneration = Atomic<Int>(0)
    /// Largest sample magnitude since `takePeak()` (Float bits).
    private let peakBits = Atomic<UInt32>(0)
    /// 10 ms frames produced so far, for noticing that audio has stopped.
    private let framesEmitted = Atomic<Int>(0)
    private var lastSeenFrames = 0
    private var silentChecks = 0
    private var restartWork: DispatchWorkItem?

    // Owned by the capture thread; swapped under `formatLock` when the route changes.
    private let formatLock = NSLock()
    private var converter: AVAudioConverter?
    private var frame = [Float](repeating: 0, count: Wire.frameSamples)
    private var frameFill = 0
    private static let format48k = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: Wire.sampleRate, channels: 1, interleaved: false)!

    private var observers: [NSObjectProtocol] = []

    enum CaptureError: LocalizedError {
        case noInput
        var errorDescription: String? { "No microphone is available." }
    }

    // MARK: - Start / stop (main thread)

    func start() throws {
        guard !isRunning else { return }
        try configureSession()
        try startEngine()
        startWorker()
        observeNotifications()
        isRunning = true
    }

    func stop() {
        guard isRunning else { return }
        isRunning = false
        observers.forEach(NotificationCenter.default.removeObserver)
        observers = []
        engine.stop()
        workerGeneration.add(1, ordering: .relaxed)
        wake.signal()
        try? AVAudioSession.sharedInstance().setActive(false, options: .notifyOthersOnDeactivation)
    }

    /// Re-applies session settings (e.g. after the Bluetooth quality toggle changes).
    func reconfigure() {
        guard isRunning else { return }
        do {
            engine.stop()
            try configureSession()
            try startEngine()
        } catch {
            NSLog("Capture reconfigure failed: \(error)")
        }
        onChange?()
    }

    func selectMic(id: String?) {
        preferredMicID = id
        let session = AVAudioSession.sharedInstance()
        let port = session.availableInputs?.first { $0.uid == id }
        try? session.setPreferredInput(port)
        // The engine reports a configuration change if the format changed; restart anyway
        // so the new route is picked up straight away.
        reconfigure()
    }

    /// Call about once a second. Returns false if no audio has arrived for a couple of seconds,
    /// and keeps trying to restart capture until it does.
    func checkAudioFlowing() -> Bool {
        let frames = framesEmitted.load(ordering: .relaxed)
        defer { lastSeenFrames = frames }
        guard isRunning, !isInterrupted, frames == lastSeenFrames else {
            silentChecks = 0
            return true
        }
        silentChecks += 1
        if silentChecks % 3 == 0 {
            NSLog("Capture: no audio for \(silentChecks) s, restarting")
            reconfigure()
        }
        return silentChecks < 2
    }

    var availableMics: [MicOption] {
        (AVAudioSession.sharedInstance().availableInputs ?? []).map(MicOption.init)
    }

    var currentMic: MicOption? {
        AVAudioSession.sharedInstance().currentRoute.inputs.first.map(MicOption.init)
    }

    /// Phone-side delay: mic hardware + OS buffer + one 10 ms frame (+ resampler if used).
    var captureLatencyMs: Double {
        let s = AVAudioSession.sharedInstance()
        let resampler = inputSampleRate == Wire.sampleRate ? 0 : 1.5
        return (s.inputLatency + s.ioBufferDuration) * 1000 + 10 + resampler
    }

    /// Peak level since the last call, 0…1.
    func takePeak() -> Float {
        Float(bitPattern: peakBits.exchange(0, ordering: .relaxed))
    }

    // MARK: - Session and engine

    private func configureSession() throws {
        let session = AVAudioSession.sharedInstance()
        var options: AVAudioSession.CategoryOptions = [.allowBluetoothHFP]
        var mode: AVAudioSession.Mode = .measurement
        if bluetoothHighQuality, #available(iOS 26.0, *) {
            // Only allowed with the default mode. Falls back to HFP if the earbuds can't do it.
            options.insert(.bluetoothHighQualityRecording)
            mode = .default
        }
        try session.setCategory(.playAndRecord, mode: mode, options: options)
        try? session.setPreferredSampleRate(Wire.sampleRate)
        try? session.setPreferredIOBufferDuration(0.005)
        // Don't let a ringtone or alert interrupt the stream.
        try? session.setPrefersNoInterruptionsFromSystemAlerts(true)
        try session.setActive(true)
        if let id = preferredMicID, let port = session.availableInputs?.first(where: { $0.uid == id }) {
            try? session.setPreferredInput(port)
        }
    }

    private func startEngine() throws {
        engine.stop()
        // A fresh engine every time: a reused one keeps reporting the previous route's format,
        // and connecting with that fails silently (AirPods run at 24 kHz, the phone mic at 48).
        let engine = AVAudioEngine()
        self.engine = engine

        let input = engine.inputNode
        let hwFormat = input.outputFormat(forBus: 0)
        guard hwFormat.sampleRate > 0, hwFormat.channelCount > 0 else { throw CaptureError.noInput }

        let ring = self.ring
        let wake = self.wake
        let sink = AVAudioSinkNode { _, frameCount, bufferList in
            // Real-time thread: copy channel 0 and wake the capture thread. Nothing else.
            let buffers = UnsafeMutableAudioBufferListPointer(UnsafeMutablePointer(mutating: bufferList))
            if let data = buffers.first?.mData?.assumingMemoryBound(to: Float.self) {
                ring.write(data, count: Int(frameCount))
                wake.signal()
            }
            return noErr
        }
        engine.attach(sink)
        engine.connect(input, to: sink, format: hwFormat)
        self.sink = sink

        formatLock.lock()
        inputSampleRate = hwFormat.sampleRate
        if hwFormat.sampleRate == Wire.sampleRate {
            converter = nil
        } else {
            let mono = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: hwFormat.sampleRate, channels: 1, interleaved: false)!
            converter = AVAudioConverter(from: mono, to: Self.format48k)
            converter?.sampleRateConverterQuality = AVAudioQuality.high.rawValue
        }
        formatLock.unlock()

        engine.prepare()
        try engine.start()
        NSLog("Capture: \(hwFormat.sampleRate) Hz, \(hwFormat.channelCount) ch, mic: \(currentMic?.name ?? "?")")
    }

    private func observeNotifications() {
        let nc = NotificationCenter.default
        let session = AVAudioSession.sharedInstance()
        observers.append(nc.addObserver(forName: AVAudioSession.interruptionNotification, object: session, queue: .main) { [weak self] note in
            guard let self, let raw = note.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt,
                  let type = AVAudioSession.InterruptionType(rawValue: raw) else { return }
            switch type {
            case .began:
                self.isInterrupted = true
            case .ended:
                self.isInterrupted = false
                self.reconfigure()
            @unknown default:
                break
            }
            self.onChange?()
        })
        observers.append(nc.addObserver(forName: .AVAudioEngineConfigurationChange, object: nil, queue: .main) { [weak self] note in
            guard let self, self.isRunning, !self.isInterrupted, note.object as? AVAudioEngine === self.engine else { return }
            // Ignore it if capture is already running on the current route.
            if !self.engine.isRunning || session.sampleRate != self.inputSampleRate {
                self.scheduleRestart()
            }
        })
        observers.append(nc.addObserver(forName: AVAudioSession.routeChangeNotification, object: session, queue: .main) { [weak self] _ in
            self?.onChange?()
        })
        observers.append(nc.addObserver(forName: AVAudioSession.mediaServicesWereResetNotification, object: session, queue: .main) { [weak self] _ in
            self?.reconfigure()
        })
    }

    /// iOS can report several changes while a Bluetooth route comes up, so restart once it settles.
    private func scheduleRestart() {
        restartWork?.cancel()
        let work = DispatchWorkItem { [weak self] in
            guard let self, self.isRunning, !self.isInterrupted else { return }
            do {
                try self.startEngine()
            } catch {
                NSLog("Capture restart failed: \(error)")
            }
            self.onChange?()
        }
        restartWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.25, execute: work)
    }

    /// Call when the app comes back to the foreground: restarts capture if iOS stopped it
    /// (e.g. a call ended while the app was in the background).
    func resumeIfNeeded() {
        guard isRunning, !engine.isRunning else { return }
        isInterrupted = false
        reconfigure()
    }

    // MARK: - Capture thread

    private func startWorker() {
        let generation = workerGeneration.add(1, ordering: .relaxed).newValue
        let thread = Thread { [weak self] in self?.workerLoop(generation: generation) }
        thread.name = "capture"
        thread.qualityOfService = .userInteractive
        thread.start()
    }

    private func workerLoop(generation: Int) {
        let chunkSize = 4096
        let chunk = UnsafeMutablePointer<Float>.allocate(capacity: chunkSize)
        defer { chunk.deallocate() }
        while workerGeneration.load(ordering: .relaxed) == generation {
            _ = wake.wait(timeout: .now() + .milliseconds(50))
            while true {
                let n = ring.read(into: chunk, max: chunkSize)
                if n == 0 { break }
                process(chunk, count: n)
            }
        }
    }

    private func process(_ samples: UnsafePointer<Float>, count: Int) {
        formatLock.lock()
        let converter = self.converter
        let rate = inputSampleRate
        formatLock.unlock()

        guard let converter else {
            appendToFrames(samples, count: count)
            return
        }
        let inFormat = converter.inputFormat
        guard rate > 0,
              let inBuf = AVAudioPCMBuffer(pcmFormat: inFormat, frameCapacity: AVAudioFrameCount(count)),
              let outBuf = AVAudioPCMBuffer(pcmFormat: Self.format48k,
                                            frameCapacity: AVAudioFrameCount(Double(count) * Wire.sampleRate / rate) + 64)
        else { return }
        inBuf.frameLength = AVAudioFrameCount(count)
        inBuf.floatChannelData![0].update(from: samples, count: count)
        var supplied = false
        var error: NSError?
        converter.convert(to: outBuf, error: &error) { _, status in
            if supplied {
                status.pointee = .noDataNow
                return nil
            }
            supplied = true
            status.pointee = .haveData
            return inBuf
        }
        if outBuf.frameLength > 0 {
            appendToFrames(outBuf.floatChannelData![0], count: Int(outBuf.frameLength))
        }
    }

    private func appendToFrames(_ samples: UnsafePointer<Float>, count: Int) {
        var i = 0
        while i < count {
            let take = min(Wire.frameSamples - frameFill, count - i)
            frame.withUnsafeMutableBufferPointer { f in
                (f.baseAddress! + frameFill).update(from: samples + i, count: take)
            }
            frameFill += take
            i += take
            if frameFill == Wire.frameSamples {
                frameFill = 0
                framesEmitted.add(1, ordering: .relaxed)
                frame.withUnsafeBufferPointer { f in
                    var peak: Float = 0
                    for s in f { peak = max(peak, abs(s)) }
                    peakBits.max(peak.bitPattern)
                    onFrame?(f.baseAddress!)
                }
            }
        }
    }
}

private extension Atomic<UInt32> {
    /// Raise to `value` if larger (valid for non-negative Float bit patterns).
    func max(_ value: UInt32) {
        var current = load(ordering: .relaxed)
        while value > current {
            let (exchanged, original) = compareExchange(expected: current, desired: value, ordering: .relaxed)
            if exchanged { return }
            current = original
        }
    }
}

/// Single-producer / single-consumer ring of samples. `write` is safe on the real-time
/// audio thread (no locks, no allocation); if the reader falls behind, new samples are dropped.
final class SampleRing: @unchecked Sendable {
    private let storage: UnsafeMutablePointer<Float>
    private let capacity: Int
    private let writeCount = Atomic<Int>(0)
    private let readCount = Atomic<Int>(0)

    init(capacity: Int) {
        self.capacity = capacity
        storage = .allocate(capacity: capacity)
        storage.initialize(repeating: 0, count: capacity)
    }

    deinit {
        storage.deallocate()
    }

    func write(_ src: UnsafePointer<Float>, count: Int) {
        let w = writeCount.load(ordering: .relaxed)
        let r = readCount.load(ordering: .acquiring)
        guard capacity - (w - r) >= count else { return }
        for i in 0..<count {
            storage[(w + i) % capacity] = src[i]
        }
        writeCount.store(w + count, ordering: .releasing)
    }

    func read(into dst: UnsafeMutablePointer<Float>, max: Int) -> Int {
        let r = readCount.load(ordering: .relaxed)
        let w = writeCount.load(ordering: .acquiring)
        let n = min(w - r, max)
        for i in 0..<n {
            dst[i] = storage[(r + i) % capacity]
        }
        readCount.store(r + n, ordering: .releasing)
        return n
    }
}
