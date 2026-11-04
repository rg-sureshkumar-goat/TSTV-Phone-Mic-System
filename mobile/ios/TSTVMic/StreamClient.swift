import Foundation
import Network

/// Where to connect: a desktop found by Bonjour, or an IP address typed in.
enum DesktopTarget: Hashable {
    case service(name: String, endpoint: NWEndpoint)
    case host(NWEndpoint.Host)

    var label: String {
        switch self {
        case .service(let name, _): name
        case .host(let host): "\(host)"
        }
    }
}

/// Talks to the desktop app: WebSocket control channel plus UDP audio (shared/PROTOCOL.md).
/// Lives as long as the app; `start` / `stop` connect and disconnect.
final class StreamClient: @unchecked Sendable {
    enum Event {
        /// `host` is the desktop's IP address, for faster reconnects.
        case connected(Welcome, host: NWEndpoint.Host)
        case rejected(String)
        case operatorMuted(Bool)
        case stats(latencyMs: Double, lossPct: Double, channel: Int)
        case closed(String?)
    }

    /// Called on the network queue.
    var onEvent: ((Event) -> Void)?

    private let queue = DispatchQueue(label: "org.tstvnews.phonemic.network", qos: .userInteractive)
    private var control: NWConnection?
    private var resolver: NWConnection?
    private var host: NWEndpoint.Host?
    private var session = 0
    private var welcomed = false
    /// Give up on a connection attempt that hasn't been welcomed after this long.
    private static let connectTimeout: TimeInterval = 6

    // Audio sender state: written from the network and main threads, read on the capture thread.
    private let lock = NSLock()
    private var audio: NWConnection?
    private var token: UInt32?
    private var encoder: OpusEncoder?
    private var seq = UInt32.random(in: 0 ... .max)
    private var previous = Data()
    private var lastFrameAt: UInt64 = 0
    private var talentMuted = false

    /// Opus look-ahead of the current encoder, for the latency report.
    private(set) var encoderDelayMs: Double = 2.5

    func start(_ target: DesktopTarget, hello: HelloMessage) {
        queue.async {
            self.teardown()
            self.session += 1
            self.welcomed = false
            let session = self.session
            // Some failures (e.g. a Bonjour lookup while the desktop restarts) never report
            // an error, so time the attempt out and let the app retry.
            self.queue.asyncAfter(deadline: .now() + Self.connectTimeout) {
                guard session == self.session, !self.welcomed else { return }
                self.finish(session, reason: Self.notAnswering)
            }
            switch target {
            case .host(let host):
                self.openControl(host: host, hello: hello, session: session)
            case .service(_, let endpoint):
                self.resolve(endpoint, session: session) { host in
                    self.openControl(host: host, hello: hello, session: session)
                }
            }
        }
    }

    func stop() {
        queue.async {
            self.session += 1
            self.teardown()
        }
    }

    func setTalentMuted(_ muted: Bool) {
        lock.withLock { talentMuted = muted }
    }

    func sendStatus(_ status: StatusMessage) {
        queue.async { self.sendJSON(status) }
    }

    /// Capture thread: encodes and sends one 10 ms frame. Does nothing until connected.
    func sendFrame(_ pcm: UnsafePointer<Float>) {
        lock.lock()
        defer { lock.unlock() }
        guard let token, let audio, let encoder else { return }

        // If capture paused (route change, interruption), skip sequence numbers for the gap
        // so the desktop keeps the timing right.
        let now = DispatchTime.now().uptimeNanoseconds
        if lastFrameAt != 0, now > lastFrameAt + 100_000_000 {
            seq &+= UInt32(min((now - lastFrameAt) / 10_000_000, 10_000)) - 1
        }
        lastFrameAt = now

        let primary = talentMuted ? Data() : (encoder.encode(pcm) ?? Data())
        let packet = Wire.audioPacket(token: token, seq: seq, muted: talentMuted, primary: primary, redundant: previous)
        previous = primary
        seq &+= 1
        audio.send(content: packet, completion: .idempotent)
    }

    // MARK: - Network queue

    /// Finds the address behind a Bonjour service (IPv4 or IPv6, whichever connects).
    private func resolve(_ endpoint: NWEndpoint, session: Int, then: @escaping (NWEndpoint.Host) -> Void) {
        let conn = NWConnection(to: endpoint, using: .tcp)
        resolver = conn
        conn.stateUpdateHandler = { [weak self] state in
            guard let self, session == self.session else { return }
            switch state {
            case .ready:
                if case .hostPort(let host, _)? = conn.currentPath?.remoteEndpoint {
                    conn.cancel()
                    self.resolver = nil
                    then(host)
                }
            case .failed(let error), .waiting(let error):
                conn.cancel()
                self.resolver = nil
                self.finish(session, reason: Self.explain(error, path: conn.currentPath, host: nil))
            default:
                break
            }
        }
        conn.start(queue: queue)
    }

    private func openControl(host: NWEndpoint.Host, hello: HelloMessage, session: Int) {
        guard let url = Self.webSocketURL(host) else {
            finish(session, reason: "\"\(host)\" isn't a valid address.")
            return
        }
        self.host = host
        let ws = NWProtocolWebSocket.Options()
        ws.autoReplyPing = true
        let tcp = NWProtocolTCP.Options()
        tcp.noDelay = true
        tcp.connectionTimeout = 5
        let params = NWParameters(tls: nil, tcp: tcp)
        params.defaultProtocolStack.applicationProtocols.insert(ws, at: 0)

        let conn = NWConnection(to: .url(url), using: params)
        control = conn
        conn.stateUpdateHandler = { [weak self] state in
            guard let self, session == self.session else { return }
            switch state {
            case .ready:
                self.sendJSON(hello)
            case .waiting(let error), .failed(let error):
                self.finish(session, reason: Self.explain(error, path: conn.currentPath, host: host))
            case .cancelled:
                self.finish(session, reason: nil)
            default:
                break
            }
        }
        receiveControl(conn, session: session)
        conn.start(queue: queue)
    }

    private func receiveControl(_ conn: NWConnection, session: Int) {
        conn.receiveMessage { [weak self] data, context, _, error in
            guard let self, session == self.session else { return }
            let meta = context?.protocolMetadata(definition: NWProtocolWebSocket.definition) as? NWProtocolWebSocket.Metadata
            if meta?.opcode == .close {
                self.finish(session, reason: "The desktop app closed the connection.")
                return
            }
            if let data, meta?.opcode == .text, let msg = DesktopMessage.decode(data) {
                self.handle(msg, session: session)
            }
            if let error {
                self.finish(session, reason: "The connection to the desktop was lost.")
                return
            }
            self.receiveControl(conn, session: session)
        }
    }

    private func handle(_ msg: DesktopMessage, session: Int) {
        switch msg {
        case .welcome(let welcome):
            welcomed = true
            openAudio(welcome)
            if let host { onEvent?(.connected(welcome, host: host)) }
        case .reject(let message):
            onEvent?(.rejected(message))
            self.session += 1
            teardown()
        case .operatorMuted(let muted):
            onEvent?(.operatorMuted(muted))
        case .stats(let latency, let loss, let channel):
            onEvent?(.stats(latencyMs: latency, lossPct: loss, channel: channel))
        }
    }

    private func openAudio(_ welcome: Welcome) {
        guard let host, let port = NWEndpoint.Port(rawValue: welcome.udpPort) else { return }
        let params = NWParameters.udp
        // Puts the packets in the Wi-Fi voice queue (WMM AC_VO) on routers that support it.
        params.serviceClass = .interactiveVoice
        let conn = NWConnection(host: host, port: port, using: params)
        conn.start(queue: queue)
        receivePings(conn)

        let encoder = OpusEncoder(bitrate: welcome.bitrate)
        encoderDelayMs = encoder?.lookaheadMs ?? 2.5
        lock.withLock {
            self.audio = conn
            self.token = welcome.token
            self.encoder = encoder
            self.previous = Data()
            self.lastFrameAt = 0
        }
    }

    private func receivePings(_ conn: NWConnection) {
        conn.receiveMessage { [weak self] data, _, _, error in
            if let data, let pong = Wire.pong(forPing: data) {
                conn.send(content: pong, completion: .idempotent)
            }
            if error == nil, self != nil {
                self?.receivePings(conn)
            }
        }
    }

    private func sendJSON<T: Encodable>(_ value: T) {
        guard let control, let data = try? JSONEncoder().encode(value) else { return }
        let meta = NWProtocolWebSocket.Metadata(opcode: .text)
        let context = NWConnection.ContentContext(identifier: "text", metadata: [meta])
        control.send(content: data, contentContext: context, isComplete: true, completion: .contentProcessed { _ in })
    }

    /// Ends `session` once, reporting why.
    private func finish(_ session: Int, reason: String?) {
        guard session == self.session else { return }
        self.session += 1
        teardown()
        onEvent?(.closed(reason))
    }

    private func teardown() {
        resolver?.cancel()
        resolver = nil
        control?.cancel()
        control = nil
        host = nil
        lock.withLock {
            audio?.cancel()
            audio = nil
            token = nil
            encoder = nil
        }
    }

    /// NWProtocolWebSocket only works with URL endpoints.
    private static func webSocketURL(_ host: NWEndpoint.Host) -> URL? {
        var text = "\(host)"
        if case .ipv6 = host {
            // IPv6 needs brackets, and the "%" before a link-local address's interface name
            // must be escaped.
            text = "[\(text.replacingOccurrences(of: "%", with: "%25"))]"
        }
        return URL(string: "ws://\(text):\(Wire.controlPort)/")
    }

    // MARK: - Explaining failures

    static let localNetworkOff = "Local Network access is off for TSTV Mic. Turn it on in Settings › Privacy & Security › Local Network, then reopen the app."
    /// The error code for "blocked by privacy settings" (kDNSServiceErr_PolicyDenied).
    static let dnsPolicyDenied: Int32 = -65570
    private static let notAnswering = "The desktop didn't answer. If it's open, this network may be stopping devices from talking to each other (common on campus Wi-Fi)."

    /// Turns a network error into something the talent or TD can act on.
    private static func explain(_ error: NWError, path: NWPath?, host: NWEndpoint.Host?) -> String {
        if path?.unsatisfiedReason == .localNetworkDenied { return localNetworkOff }
        let desktop = host.map { "the desktop at \($0)" } ?? "the desktop"
        switch error {
        case .dns(dnsPolicyDenied):
            return localNetworkOff
        case .posix(.ECONNREFUSED):
            return "Nothing is answering at \(desktop). Is the desktop app open, and is this still its address?"
        case .posix(.ETIMEDOUT), .posix(.EHOSTUNREACH), .posix(.EHOSTDOWN), .posix(.ENETUNREACH), .posix(.ENETDOWN):
            return "Can't reach \(desktop). This network may be stopping devices from talking to each other (common on campus Wi-Fi)."
        default:
            return "Can't reach \(desktop) (\(error))."
        }
    }
}
