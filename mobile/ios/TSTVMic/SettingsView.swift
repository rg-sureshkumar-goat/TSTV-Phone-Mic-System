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
                        .disabled(!Self.highQualityAvailable)
                } footer: {
                    Text(Self.highQualityAvailable
                         ? "Full-bandwidth sound from AirPods Pro 2, AirPods 4 and newer instead of the phone-call sound. Apple says it may add delay — compare both settings in the lip-sync test."
                         : "Needs iOS 26 or later.")
                }

                Section {
                    LabeledContent("Echo cancellation", value: "Off")
                    LabeledContent("Noise suppression", value: "Off")
                    LabeledContent("Auto gain", value: "Off")
                } header: {
                    Text("Audio processing")
                } footer: {
                    Text("The mixer's Fat Channel handles EQ and dynamics. Switchable processing comes in a later version.")
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
