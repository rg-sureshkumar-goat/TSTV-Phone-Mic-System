import Foundation

/// Wire format shared with the desktop app. The spec is shared/PROTOCOL.md.
enum Wire {
    static let protocolVersion = 1
    static let controlPort: UInt16 = 47800
    static let serviceType = "_tstvmic._tcp"
    static let sampleRate = 48_000.0
    /// 10 ms at 48 kHz.
    static let frameSamples = 480

    private static let magic: [UInt8] = [0x54, 0x56] // "TV"
    private static let version: UInt8 = 1
    static let kindAudio: UInt8 = 1
    static let kindPing: UInt8 = 2
    static let kindPong: UInt8 = 3
    static let codecOpus: UInt8 = 1
    static let flagTalentMuted: UInt8 = 1

    static func audioPacket(token: UInt32, seq: UInt32, muted: Bool, primary: Data, redundant: Data) -> Data {
        var d = Data(capacity: 20 + primary.count + redundant.count)
        appendHeader(&d, kind: kindAudio, token: token)
        appendBE(&d, seq)
        d.append(muted ? flagTalentMuted : 0)
        d.append(codecOpus)
        appendBE(&d, UInt16(primary.count))
        d.append(primary)
        appendBE(&d, UInt16(redundant.count))
        d.append(redundant)
        return d
    }

    /// If `data` is a ping from the desktop, returns the matching pong.
    static func pong(forPing data: Data) -> Data? {
        let b = [UInt8](data)
        guard b.count >= 16, b[0] == magic[0], b[1] == magic[1], b[2] == version, b[3] == kindPing else { return nil }
        var reply = b[0..<16]
        reply[reply.startIndex + 3] = kindPong
        return Data(reply)
    }

    private static func appendHeader(_ d: inout Data, kind: UInt8, token: UInt32) {
        d.append(contentsOf: magic)
        d.append(version)
        d.append(kind)
        appendBE(&d, token)
    }

    private static func appendBE<T: FixedWidthInteger>(_ d: inout Data, _ v: T) {
        withUnsafeBytes(of: v.bigEndian) { d.append(contentsOf: $0) }
    }
}

// MARK: - Control messages (JSON over WebSocket)

struct InputInfo: Codable, Equatable {
    var name: String
    /// builtIn | bluetooth | wired | other
    var kind: String
}

struct HelloMessage: Encodable {
    let type = "hello"
    let `protocol` = Wire.protocolVersion
    var deviceId: String
    var name: String
    let platform = "ios"
    var model: String
    var appVersion: String
}

struct StatusMessage: Encodable, Equatable {
    let type = "status"
    var talentMuted: Bool
    var input: InputInfo?
    var captureLatencyMs: Double
    var inputSampleRate: Double
    var battery: Double?
    var interrupted: Bool
}

struct Welcome: Decodable {
    var slot: Int
    var channel: Int
    var token: UInt32
    var udpPort: UInt16
    var bitrate: Int
    var desktopName: String
}

enum DesktopMessage {
    case welcome(Welcome)
    case reject(message: String)
    case operatorMuted(Bool)
    case stats(latencyMs: Double, lossPct: Double, channel: Int)

    private struct Envelope: Decodable {
        var type: String
        var message: String?
        var operatorMuted: Bool?
        var latencyMs: Double?
        var lossPct: Double?
        var channel: Int?
    }

    static func decode(_ data: Data) -> DesktopMessage? {
        let decoder = JSONDecoder()
        guard let env = try? decoder.decode(Envelope.self, from: data) else { return nil }
        switch env.type {
        case "welcome":
            return (try? decoder.decode(Welcome.self, from: data)).map { .welcome($0) }
        case "reject":
            return .reject(message: env.message ?? "The desktop app refused the connection.")
        case "operator":
            return .operatorMuted(env.operatorMuted ?? false)
        case "stats":
            return .stats(latencyMs: env.latencyMs ?? 0, lossPct: env.lossPct ?? 0, channel: env.channel ?? 0)
        default:
            return nil
        }
    }
}
