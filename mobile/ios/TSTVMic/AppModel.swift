import AVFoundation
import Observation
import UIKit

/// App state and the glue between the microphone, the network and the screens.
@MainActor
@Observable
final class AppModel {
    enum Phase: Equatable {
        case idle
        case connecting
        case connected
        case reconnecting
    }

    // Saved between launches
    var name: String { didSet { defaults.set(name, forKey: "name") } }
    var manualHost: String { didSet { defaults.set(manualHost, forKey: "manualHost") } }
    var bluetoothHighQuality: Bool {
        didSet {
            defaults.set(bluetoothHighQuality, forKey: "bluetoothHighQuality")
            capture.bluetoothHighQuality = bluetoothHighQuality
            capture.reconfigure()
        }
    }

    private(set) var phase: Phase = .idle
    private(set) var target: DesktopTarget?
    private(set) var desktops: [DesktopTarget] = []
    private(set) var errorMessage: String?

    private(set) var talentMuted = false
    private(set) var operatorMuted = false
    private(set) var interrupted = false
    private(set) var desktopName = ""
    private(set) var channel = 0
    private(set) var latencyMs: Double?
    private(set) var lossPct = 0.0
    /// Mic level in dBFS for the meter.
    private(set) var levelDb: Float = -90
    private(set) var mics: [MicOption] = []
    private(set) var currentMic: MicOption?
    var showFocusTip = true

    /// On air = connected, not muted by anyone, mic not taken by another app.
    var isLive: Bool { phase == .connected && !talentMuted && !operatorMuted && !interrupted }

    private let defaults = UserDefaults.standard
    private let deviceId: String
    private let capture = AudioCapture()
    private let client = StreamClient()
    private let discovery = Discovery()
    private var wantsConnection = false
    private var reconnectTask: Task<Void, Never>?
    /// IP address of the desktop from the last successful connection.
    private var lastHost: String?
    private var reconnectAttempts = 0
    private var timers: [Timer] = []
    private var lastStatus: StatusMessage?
    private var lastStatusSent = Date.distantPast

    init() {
        name = defaults.string(forKey: "name") ?? ""
        manualHost = defaults.string(forKey: "manualHost") ?? ""
        bluetoothHighQuality = defaults.bool(forKey: "bluetoothHighQuality")
        if let id = defaults.string(forKey: "deviceId") {
            deviceId = id
        } else {
            deviceId = UUID().uuidString
            defaults.set(deviceId, forKey: "deviceId")
        }
        capture.bluetoothHighQuality = bluetoothHighQuality

        let client = self.client
        capture.onFrame = { pcm in client.sendFrame(pcm) }
        capture.onChange = { [weak self] in self?.captureChanged() }
        client.onEvent = { [weak self] event in
            Task { @MainActor in self?.handle(event) }
        }
        discovery.onChange = { [weak self] found in self?.desktops = found }
        discovery.start()
        UIDevice.current.isBatteryMonitoringEnabled = true
    }

    // MARK: - Actions

    func connect(to target: DesktopTarget) {
        Task { await startConnection(to: target) }
    }

    func connectToManualHost() {
        let host = manualHost.trimmingCharacters(in: .whitespaces)
        guard !host.isEmpty else { return }
        connect(to: .host(host))
    }

    func disconnect() {
        wantsConnection = false
        reconnectTask?.cancel()
        client.stop()
        capture.stop()
        timers.forEach { $0.invalidate() }
        timers = []
        UIApplication.shared.isIdleTimerDisabled = false
        phase = .idle
        operatorMuted = false
        latencyMs = nil
        discovery.start()
    }

    func toggleMute() {
        talentMuted.toggle()
        client.setTalentMuted(talentMuted)
        sendStatus(force: true)
    }

    func selectMic(_ mic: MicOption) {
        capture.selectMic(id: mic.id)
        captureChanged()
    }

    /// The app came back to the foreground.
    func appBecameActive() {
        capture.resumeIfNeeded()
        captureChanged()
    }

    // MARK: - Connection

    private func startConnection(to target: DesktopTarget) async {
        errorMessage = nil
        guard await AVAudioApplication.requestRecordPermission() else {
            errorMessage = "Microphone access is off. Turn it on in Settings › Privacy & Security › Microphone › TSTV Mic."
            return
        }
        do {
            try capture.start()
        } catch {
            errorMessage = "Couldn't start the microphone: \(error.localizedDescription)"
            return
        }
        self.target = target
        lastHost = nil
        reconnectAttempts = 0
        wantsConnection = true
        phase = .connecting
        discovery.stop()
        UIApplication.shared.isIdleTimerDisabled = true
        client.setTalentMuted(talentMuted)
        client.start(target, hello: hello())
        captureChanged()
        startTimers()
    }

    private func hello() -> HelloMessage {
        HelloMessage(
            deviceId: deviceId,
            name: name.trimmingCharacters(in: .whitespaces),
            model: Self.modelIdentifier,
            appVersion: Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "?"
        )
    }

    private func handle(_ event: StreamClient.Event) {
        switch event {
        case .connected(let welcome, let host):
            phase = .connected
            lastHost = host.isEmpty ? nil : host
            reconnectAttempts = 0
            desktopName = welcome.desktopName
            channel = welcome.channel
            errorMessage = nil
            sendStatus(force: true)
        case .rejected(let message):
            wantsConnection = false
            disconnect()
            errorMessage = message
        case .operatorMuted(let muted):
            operatorMuted = muted
        case .stats(let latency, let loss, let channel):
            latencyMs = latency > 0 ? latency : nil
            lossPct = loss
            self.channel = channel
        case .closed(let reason):
            guard wantsConnection, let target else { return }
            phase = .reconnecting
            latencyMs = nil
            if let reason { errorMessage = reason }
            reconnectTask?.cancel()
            // Alternate between the last known IP address (quick) and the original target
            // (finds the desktop again if its address changed).
            reconnectAttempts += 1
            let next: DesktopTarget = if reconnectAttempts % 2 == 1, let lastHost { .host(lastHost) } else { target }
            reconnectTask = Task { [weak self] in
                try? await Task.sleep(for: .seconds(2))
                guard let self, !Task.isCancelled, self.wantsConnection else { return }
                self.client.start(next, hello: self.hello())
            }
        }
    }

    private func startTimers() {
        timers.forEach { $0.invalidate() }
        let meter = Timer(timeInterval: 0.05, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.updateMeter() }
        }
        let status = Timer(timeInterval: 1, repeats: true) { [weak self] _ in
            MainActor.assumeIsolated { self?.sendStatus(force: false) }
        }
        timers = [meter, status]
        timers.forEach { RunLoop.main.add($0, forMode: .common) }
    }

    private func updateMeter() {
        let peak = capture.takePeak()
        let db = peak > 0 ? 20 * log10(peak) : -90
        // Rise instantly, fall at about 20 dB/s.
        levelDb = max(db, levelDb - 1)
    }

    private func captureChanged() {
        mics = capture.availableMics
        currentMic = capture.currentMic
        interrupted = capture.isInterrupted
        sendStatus(force: false)
    }

    /// Sends a status message if something changed (or every 2 s).
    private func sendStatus(force: Bool) {
        guard phase == .connected else { return }
        let battery = UIDevice.current.batteryLevel
        let status = StatusMessage(
            talentMuted: talentMuted,
            input: currentMic.map { InputInfo(name: $0.name, kind: $0.kind) },
            captureLatencyMs: capture.captureLatencyMs + client.encoderDelayMs,
            inputSampleRate: capture.inputSampleRate,
            battery: battery >= 0 ? Double(battery) : nil,
            interrupted: interrupted
        )
        let changed = status.talentMuted != lastStatus?.talentMuted || status.input != lastStatus?.input
            || status.interrupted != lastStatus?.interrupted
        if force || changed || Date().timeIntervalSince(lastStatusSent) >= 2 {
            client.sendStatus(status)
            lastStatus = status
            lastStatusSent = Date()
        }
    }

    static let modelIdentifier: String = {
        var info = utsname()
        uname(&info)
        return withUnsafeBytes(of: &info.machine) { raw in
            String(decoding: raw.prefix(while: { $0 != 0 }), as: UTF8.self)
        }
    }()
}
