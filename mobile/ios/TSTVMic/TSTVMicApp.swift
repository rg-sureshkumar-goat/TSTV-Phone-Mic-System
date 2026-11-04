import SwiftUI

@main
struct TSTVMicApp: App {
    @State private var model = AppModel()
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            ContentView()
                .environment(model)
                .preferredColorScheme(.dark)
                .onOpenURL { model.open(link: $0) }
        }
        .onChange(of: scenePhase) { _, phase in
            if phase == .active { model.appBecameActive() }
        }
    }
}

struct ContentView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        if model.phase == .idle {
            ConnectView()
        } else {
            LiveView()
        }
    }
}
