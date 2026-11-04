import AVFoundation
import Network
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
    var joinCode: String { didSet { defaults.set(joinCode, forKey: "joinCode") } }
    var bluetoothHighQuality: Bool {
        didSet {
            defaults.set(bluetoothHighQuality, forKey: "bluetoothHighQuality")
            capture.bluetoothHighQuality = bluetoothHighQuality
            capture.reconfigure()
        }
    }
    var noiseSuppression: Bool {
        didSet {
            defaults.set(noiseSuppression, forKey: "noiseSuppression")
            capture.noiseSuppression = noiseSuppression
            capture.reconfigure()
        }
    }
    var autoGain: Bool {
        didSet {
            defaults.set(autoGain, forKey: "autoGain")
            capture.autoGain = autoGain
            capture.reconfigure()
        }
    }

    private(set) var phase: Phase = .idle
    private(set) var target: DesktopTarget?
    private(set) var desktops: [DesktopTarget] = []
    private(set) var errorMessage: String?
    /// A desktop to join once the talent has entered what's missing (a name or a join code).
    private(set) var pendingTarget: DesktopTarget?
    private(set) var needsCode = false
    /// iOS is blocking this app from the local network (permission turned off).
    private(set) var localNetworkDenied = false

    private(set) var talentMuted = false
    private(set) var operatorMuted = false
    private(set) var interrupted = false
    /// Capture is supposed to be running but no audio is arriving from the mic.
    private(set) var micStalled = false
    private(set) var desktopName = ""
    private(set) var slot = 0
    private(set) var channel = 0
    private(set) var latencyMs: Double?
    private(set) var lossPct = 0.0
    /// Mic level in dBFS for the meter.
    private(set) var levelDb: Float = -90
    private(set) var mics: [MicOption] = []
    private(set) var currentMic: MicOption?
    var showFocusTip = true

    /// On air = connected, not muted by anyone, mic not taken by another app.
    var isLive: Bool { phase == .connected && !talentMuted && !operatorMuted && !interrupted && !micStalled }

    private let defaults = UserDefaults.standard
    private let deviceId: String
    private let capture = AudioCapture()
    private let client = StreamClient()
    private let discovery = Discovery()
    private var wantsConnection = false
    private var reconnectTask: Task<Void, Never>?
    /// IP address of the desktop from the last successful connection.
    private var lastHost: NWEndpoint.Host?
    private var reconnectAttempts = 0
    private var timers: [Timer] = []
    private var lastStatus: StatusMessage?
    private var lastStatusSent = Date.distantPast

    init() {
        name = defaults.string(forKey: "name") ?? ""
        manualHost = defaults.string(forKey: "manualHost") ?? ""
        joinCode = defaults.string(forKey: "joinCode") ?? ""
        bluetoothHighQuality = defaults.bool(forKey: "bluetoothHighQuality")
        noiseSuppression = defaults.bool(forKey: "noiseSuppression")
        autoGain = defaults.bool(forKey: "autoGain")
        if let id = defaults.string(forKey: "deviceId") {
            deviceId = id
        } else {
            deviceId = UUID().uuidString
            defaults.set(deviceId, forKey: "deviceId")
        }
        capture.bluetoothHighQuality = bluetoothHighQuality
        capture.noiseSuppression = noiseSuppression
        capture.autoGain = autoGain

        let client = self.client
        capture.onFrame = { pcm in client.sendFrame(pcm) }
        capture.onChange = { [weak self] in self?.captureChanged() }
        client.onEvent = { [weak self] event in
            Task { @MainActor in self?.handle(event) }
        }
        discovery.onChange = { [weak self] found in self?.desktops = found }
        discovery.onLocalNetworkDenied = { [weak self] denied in self?.localNetworkDenied = denied }
        discovery.start()
        UIDevice.current.isBatteryMonitoringEnabled = true
    }

    // MARK: - Actions

    func connect(to target: DesktopTarget) {
        pendingTarget = nil
        needsCode = false
        Task { await startConnection(to: target) }
    }

    /// A join link from the in-app scanner, or from the iPhone's Camera app.
    func open(link url: URL) {
        guard let link = JoinLink(url) else {
            errorMessage = "That isn't a TSTV Phone Mics code."
            return
        }
        if phase != .idle { disconnect() }
        if let code = link.code { joinCode = code }
        let target = DesktopTarget.named(link.name, hosts: link.hosts)
        if name.trimmingCharacters(in: .whitespaces).isEmpty {
            pendingTarget = target
            needsCode = false
            errorMessage = "Type your name, then tap Connect."
        } else {
            connect(to: target)
        }
    }

    func connectToPending() {
        if let pendingTarget { connect(to: pendingTarget) }
    }

    func connectToManualHost() {
        let host = manualHost.trimmingCharacters(in: .whitespaces)
        guard !host.isEmpty else { return }
        connect(to: .host(NWEndpoint.Host(host)))
    }

    func disconnect() {
        wantsConnection = false
        reconnectTask?.cancel()
        client.stop()
        capture.stop()
        timers.forEach { $0.invalidate() }
        timers = []
        micStalled = false
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
            appVersion: Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "?",
            code: joinCode.isEmpty ? nil : joinCode
        )
    }

    private func handle(_ event: StreamClient.Event) {
        switch event {
        case .connected(let welcome, let host):
            phase = .connected
            lastHost = host
            reconnectAttempts = 0
            desktopName = welcome.desktopName
            slot = welcome.slot
            channel = welcome.channel
            errorMessage = nil
            sendStatus(force: true)
        case .rejected(let reason, let message):
            let rejected = target
            wantsConnection = false
            disconnect()
            errorMessage = message
            if reason == "code" {
                pendingTarget = rejected
                needsCode = true
            }
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
            // Cycle through the last known IP address (quick) and the original target (finds
            // the desktop again if its address changed), or each address from a QR code.
            reconnectAttempts += 1
            var options: [DesktopTarget] = [target]
            if case .named(_, let hosts) = target { options = hosts.map { .host($0) } }
            if let lastHost { options.insert(.host(lastHost), at: 0) }
            let next = options[(reconnectAttempts - (lastHost == nil ? 0 : 1)) % options.count]
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
            MainActor.assumeIsolated {
                guard let self else { return }
                self.micStalled = !self.capture.checkAudioFlowing()
                self.sendStatus(force: false)
            }
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
