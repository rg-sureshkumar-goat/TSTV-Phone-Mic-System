import SwiftUI

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        @Bindable var model = model
        NavigationStack {
            Form {
                Section {
                    Toggle("AirPods high-quality recording", isOn: $model.bluetoothHighQuality)
                        .disabled(!Self.highQualityAvailable || model.noiseSuppression)
                } footer: {
                    Text(Self.highQualityAvailable
                         ? "Full-bandwidth sound from AirPods Pro 2, AirPods 4 and newer instead of the phone-call sound. Apple says it may add delay — compare both settings in the lip-sync test."
                         : "Needs iOS 26 or later.")
                }

                Section {
                    Toggle("Noise suppression", isOn: $model.noiseSuppression)
                    Toggle("Auto gain", isOn: $model.autoGain)
                        .disabled(!model.noiseSuppression)
                } header: {
                    Text("Audio processing")
                } footer: {
                    Text("Leave these off unless there's a noise problem: the mixer's Fat Channel normally does this job. They use Apple's voice processing, which can change the sound and may add delay, and can't be combined with AirPods high-quality recording. Auto gain needs noise suppression on.")
                }

                Section("About") {
                    LabeledContent("Version", value: Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "?")
                    LabeledContent("Model", value: AppModel.modelIdentifier)
                }
            }
            .navigationTitle("Settings")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } }
            }
        }
    }

    private static var highQualityAvailable: Bool {
        if #available(iOS 26.0, *) { true } else { false }
    }
}
