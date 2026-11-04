import AVFoundation
import SwiftUI
import VisionKit

/// Full-screen sheet that scans the QR code from the desktop's Connect a phone panel.
struct QRScannerSheet: View {
    /// Called once with the scanned join link.
    var onFound: (URL) -> Void

    @Environment(\.dismiss) private var dismiss
    @State private var cameraAllowed: Bool?

    var body: some View {
        NavigationStack {
            content
                .navigationTitle("Scan QR code")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } }
                }
        }
        .task { cameraAllowed = await AVCaptureDevice.requestAccess(for: .video) }
    }

    @ViewBuilder private var content: some View {
        if !DataScannerViewController.isSupported {
            message("This device can't scan QR codes. Type the desktop's address instead.")
        } else if cameraAllowed == false {
            message("Camera access is off. Turn it on in Settings › Privacy & Security › Camera › TSTV Mic.")
        } else if cameraAllowed == true {
            QRScanner { url in
                dismiss()
                onFound(url)
            }
            .ignoresSafeArea(edges: .bottom)
        }
    }

    private func message(_ text: String) -> some View {
        Text(text).multilineTextAlignment(.center).foregroundStyle(.secondary).padding()
    }
}

private struct QRScanner: UIViewControllerRepresentable {
    var onFound: (URL) -> Void

    func makeUIViewController(context: Context) -> DataScannerViewController {
        let scanner = DataScannerViewController(
            recognizedDataTypes: [.barcode(symbologies: [.qr])],
            isGuidanceEnabled: true,
            isHighlightingEnabled: true
        )
        scanner.delegate = context.coordinator
        return scanner
    }

    func updateUIViewController(_ scanner: DataScannerViewController, context: Context) {
        if !scanner.isScanning { try? scanner.startScanning() }
    }

    func makeCoordinator() -> Coordinator { Coordinator(onFound: onFound) }

    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        private let onFound: (URL) -> Void
        private var done = false

        init(onFound: @escaping (URL) -> Void) { self.onFound = onFound }

        func dataScanner(_ scanner: DataScannerViewController, didAdd items: [RecognizedItem], allItems: [RecognizedItem]) {
            for case .barcode(let code) in items {
                // Other QR codes in view are ignored.
                guard !done, let text = code.payloadStringValue, let url = URL(string: text), JoinLink(url) != nil else { continue }
                done = true
                onFound(url)
            }
        }
    }
}
