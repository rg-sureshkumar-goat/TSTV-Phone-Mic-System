import SwiftUI

/// On-air screen: a huge mute button, level meter and mic picker.
struct LiveView: View {
    @Environment(AppModel.self) private var model
    @State private var confirmDisconnect = false

    private static let liveGreen = Color(red: 0.12, green: 0.62, blue: 0.31)
    private static let mutedRed = Color(red: 0.82, green: 0.08, blue: 0.17)

    var body: some View {
        VStack(spacing: 14) {
            header

            if model.interrupted {
                banner("Another app or a call has taken the microphone. Audio will resume when it ends.",
                       icon: "exclamationmark.triangle.fill", color: .orange)
            } else if model.showFocusTip {
                banner("Turn on Do Not Disturb so calls and notifications can't interrupt you on air.",
                       icon: "moon.fill", color: .indigo, dismiss: { model.showFocusTip = false })
            }

            muteButton

            LevelMeter(db: model.levelDb, dimmed: !model.isLive)
                .frame(height: 14)

            HStack {
                micPicker
                Spacer()
                Button("Disconnect", role: .destructive) { confirmDisconnect = true }
                    .confirmationDialog("Stop sending audio?", isPresented: $confirmDisconnect, titleVisibility: .visible) {
                        Button("Disconnect", role: .destructive) { model.disconnect() }
                    }
            }
            .font(.callout)
        }
        .padding()
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline) {
            Circle()
                .fill(model.phase == .connected ? .green : .orange)
                .frame(width: 10, height: 10)
            VStack(alignment: .leading, spacing: 2) {
                Text(statusLine).font(.subheadline.weight(.semibold)).lineLimit(1)
                if model.phase != .connected, let error = model.errorMessage {
                    Text(error).font(.caption).foregroundStyle(.secondary).lineLimit(4)
                }
            }
            Spacer()
            VStack(alignment: .trailing, spacing: 2) {
                if model.channel > 0 { Text("Channel \(model.channel)").font(.subheadline.weight(.semibold)) }
                if let latency = model.latencyMs {
                    Text("≈ \(Int(latency.rounded())) ms delay").font(.caption).foregroundStyle(.secondary)
                }
            }
        }
    }

    private var statusLine: String {
        switch model.phase {
        case .connected: "Connected · \(model.desktopName)"
        case .connecting: "Connecting to \(model.target?.label ?? "desktop")…"
        case .reconnecting: "Reconnecting to \(model.target?.label ?? "desktop")…"
        case .idle: ""
        }
    }

    private var muteButton: some View {
        let (title, subtitle, color, icon) = buttonLook
        return Button(action: model.toggleMute) {
            VStack(spacing: 16) {
                Image(systemName: icon).font(.system(size: 88, weight: .bold))
                Text(title).font(.system(size: 64, weight: .black)).minimumScaleFactor(0.5)
                Text(subtitle).font(.title3.weight(.medium)).multilineTextAlignment(.center).opacity(0.9)
            }
            .foregroundStyle(.white)
            .padding()
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(color, in: RoundedRectangle(cornerRadius: 36, style: .continuous))
        }
        .buttonStyle(.plain)
        .sensoryFeedback(.impact(weight: .heavy), trigger: model.talentMuted)
        .accessibilityLabel(model.talentMuted ? "Muted. Double-tap to go live." : "Live. Double-tap to mute.")
    }

    private var buttonLook: (String, String, Color, String) {
        if model.phase != .connected {
            return ("OFFLINE", model.talentMuted ? "Not connected · you are muted" : "Not connected to the desktop", .gray, "wifi.slash")
        }
        if model.talentMuted {
            return ("MUTED", "Tap to go live", Self.mutedRed, "mic.slash.fill")
        }
        if model.operatorMuted {
            return ("MUTED", "Muted by the board operator", Self.mutedRed, "slider.horizontal.3")
        }
        if model.interrupted {
            return ("NO MIC", "Microphone in use by another app", .orange, "exclamationmark.triangle.fill")
        }
        if model.micStalled {
            return ("NO MIC", "The microphone isn't sending audio. Trying to restart it…", .orange, "exclamationmark.triangle.fill")
        }
        return ("LIVE", "Tap to mute", Self.liveGreen, "mic.fill")
    }

    private var micPicker: some View {
        Menu {
            ForEach(model.mics) { mic in
                Button {
                    model.selectMic(mic)
                } label: {
                    if mic.id == model.currentMic?.id {
                        Label(mic.name, systemImage: "checkmark")
                    } else {
                        Text(mic.name)
                    }
                }
            }
        } label: {
            Label(model.currentMic?.name ?? "Microphone", systemImage: micIcon)
                .lineLimit(1)
        }
    }

    private var micIcon: String {
        switch model.currentMic?.kind {
        case "bluetooth": "airpods"
        case "wired": "headphones"
        default: "iphone"
        }
    }

    private func banner(_ text: String, icon: String, color: Color, dismiss: (() -> Void)? = nil) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: icon)
            Text(text).font(.callout).frame(maxWidth: .infinity, alignment: .leading)
            if let dismiss {
                Button(action: dismiss) { Image(systemName: "xmark") }
                    .accessibilityLabel("Dismiss")
            }
        }
        .padding(12)
        .background(color.opacity(0.25), in: RoundedRectangle(cornerRadius: 12))
    }
}

/// Horizontal level meter, −60…0 dBFS.
struct LevelMeter: View {
    var db: Float
    var dimmed: Bool

    var body: some View {
        GeometryReader { geo in
            let fraction = CGFloat(max(0, min(1, (db + 60) / 60)))
            ZStack(alignment: .leading) {
                Capsule().fill(Color.white.opacity(0.12))
                Capsule()
                    .fill(LinearGradient(colors: [.green, .green, .yellow, .red],
                                         startPoint: .leading, endPoint: .trailing))
                    .mask(alignment: .leading) {
                        Rectangle().frame(width: geo.size.width * fraction)
                    }
                    .opacity(dimmed ? 0.35 : 1)
            }
        }
        .accessibilityLabel("Microphone level")
    }
}
