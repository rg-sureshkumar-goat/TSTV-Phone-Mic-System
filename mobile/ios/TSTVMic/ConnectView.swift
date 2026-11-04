import SwiftUI

/// First screen: enter a name, then pick a desktop found on the network or type its address.
struct ConnectView: View {
    @Environment(AppModel.self) private var model
    @State private var showSettings = false
    @State private var showScanner = false
    @FocusState private var focused: Bool

    var body: some View {
        @Bindable var model = model
        NavigationStack {
            Form {
                Section("Your name") {
                    TextField("Shown on the board operator's screen", text: $model.name)
                        .textContentType(.name)
                        .focused($focused)
                }

                if let pending = model.pendingTarget {
                    Section {
                        if model.needsCode {
                            TextField("Join code", text: $model.joinCode)
                                .keyboardType(.numberPad)
                                .focused($focused)
                        }
                        Button("Connect") {
                            focused = false
                            model.connectToPending()
                        }
                        .disabled(!canConnect || (model.needsCode && model.joinCode.isEmpty))
                    } header: {
                        Text("Join \(pending.label)")
                    } footer: {
                        if let error = model.errorMessage {
                            Text(error).foregroundStyle(.red)
                        }
                    }
                }

                Section {
                    Button {
                        focused = false
                        showScanner = true
                    } label: {
                        Label("Scan QR code", systemImage: "qrcode.viewfinder")
                    }
                } footer: {
                    Text("On the desktop app, click Connect a phone to show the code.")
                }

                if model.localNetworkDenied {
                    Section {
                        Label(StreamClient.localNetworkOff, systemImage: "exclamationmark.triangle.fill")
                            .foregroundStyle(.red)
                    }
                }

                Section {
                    if model.desktops.isEmpty {
                        HStack(spacing: 12) {
                            ProgressView()
                            Text("Looking for the desktop app…").foregroundStyle(.secondary)
                        }
                    }
                    ForEach(model.desktops, id: \.self) { desktop in
                        Button {
                            focused = false
                            model.connect(to: desktop)
                        } label: {
                            Label(desktop.label, systemImage: "desktopcomputer")
                        }
                        .disabled(!canConnect)
                    }
                } header: {
                    Text("Desktops on this network")
                }

                Section {
                    TextField("e.g. 10.150.143.42", text: $model.manualHost)
                        .keyboardType(.numbersAndPunctuation)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .focused($focused)
                        .onSubmit { if canConnect { model.connectToManualHost() } }
                    Button("Connect") {
                        focused = false
                        model.connectToManualHost()
                    }
                    .disabled(!canConnect || model.manualHost.trimmingCharacters(in: .whitespaces).isEmpty)
                } header: {
                    Text("Or type the desktop's IP address")
                } footer: {
                    Text("It's shown at the top of the desktop app. Use this if the desktop doesn't appear above (some networks block discovery).")
                }

                if model.pendingTarget == nil, let error = model.errorMessage {
                    Section {
                        Label(error, systemImage: "exclamationmark.triangle.fill")
                            .foregroundStyle(.red)
                    }
                }
            }
            .navigationTitle("TSTV Phone Mic")
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button { showSettings = true } label: { Image(systemName: "gearshape") }
                        .accessibilityLabel("Settings")
                }
            }
            .sheet(isPresented: $showSettings) { SettingsView() }
            .fullScreenCover(isPresented: $showScanner) {
                QRScannerSheet { url in model.open(link: url) }
            }
        }
    }

    private var canConnect: Bool {
        !model.name.trimmingCharacters(in: .whitespaces).isEmpty
    }
}
