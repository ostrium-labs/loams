import LoamsCore
import SwiftUI
import Vision
import VisionKit

struct WelcomeView: View {
    @Environment(AppModel.self) private var model
    @State private var pasted = ""
    @State private var userCode = ""
    @State private var scanning = false

    var body: some View {
        @Bindable var model = model
        NavigationStack {
            Form {
                Section {
                    Text("Approve operations and watch your Loams instance from this phone.")
                    if let message = model.message {
                        Text(message).foregroundStyle(.red)
                    }
                }
                Section("Pair with a QR code") {
                    Text("In the console or desktop app, open Devices, then Pair a phone.").font(.footnote)
                    if DataScannerViewController.isSupported {
                        Button("Scan QR code") { scanning = true }
                    }
                    TextField("Or paste the pairing payload", text: $pasted, axis: .vertical)
                        .lineLimit(2...4)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                    Button("Pair") { Task { await model.pair(payloadText: pasted) } }
                        .disabled(model.busy || pasted.isEmpty)
                    if AppModel.allowInsecureLoopback {
                        Button("Debug: pair with the local mock") { Task { await model.pairWithMock() } }
                            .disabled(model.busy)
                    }
                }
                Section("Instance address") {
                    TextField("https://loams.example.com", text: $model.serverURL)
                        .keyboardType(.URL)
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                }
                Section("Pair with a typed code") {
                    Text("You will compare a fingerprint with the console, because nothing is pinned in advance.").font(.footnote)
                    TextField("8-digit code", text: $userCode).keyboardType(.numberPad)
                    Button("Continue") { Task { await model.startTyped() } }
                        .disabled(model.busy || userCode.count != 8)
                }
                Section("Sign in with Authentik") {
                    Button("Sign in") { Task { await model.signInWithBrowser() } }
                        .disabled(model.busy)
                }
                Section {
                    Button("Try the demo (no server)") { model.startDemo() }
                }
            }
            .navigationTitle("Loams")
            .sheet(isPresented: $scanning) {
                QRScanner { text in
                    scanning = false
                    Task { await model.pair(payloadText: text) }
                }
                .ignoresSafeArea()
            }
            .alert("Compare this fingerprint", isPresented: Binding(get: { model.confirm != nil }, set: { if !$0 { model.confirm = nil } })) {
                Button("It matches") { Task { await model.finishTyped(userCode: userCode) } }
                Button("Cancel", role: .cancel) { model.confirm = nil }
            } message: {
                Text("Continue only if the console shows exactly:\n\(Self.grouped(model.confirm?.thumbprint ?? ""))\n\nThis pairing was not pinned in advance.")
            }
        }
    }

    static func grouped(_ s: String) -> String {
        stride(from: 0, to: s.count, by: 4).map { i -> String in
            let start = s.index(s.startIndex, offsetBy: i)
            return String(s[start..<(s.index(start, offsetBy: 4, limitedBy: s.endIndex) ?? s.endIndex)])
        }.joined(separator: " ")
    }
}

/// VisionKit's QR scanner (first-party, iOS 16+; not available in the simulator).
struct QRScanner: UIViewControllerRepresentable {
    let onCode: (String) -> Void

    func makeUIViewController(context: Context) -> DataScannerViewController {
        let vc = DataScannerViewController(recognizedDataTypes: [.barcode(symbologies: [.qr])], qualityLevel: .balanced, isHighlightingEnabled: true)
        vc.delegate = context.coordinator
        try? vc.startScanning()
        return vc
    }

    func updateUIViewController(_ uiViewController: DataScannerViewController, context: Context) {}

    func makeCoordinator() -> Coordinator { Coordinator(onCode: onCode) }

    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        let onCode: (String) -> Void
        private var done = false

        init(onCode: @escaping (String) -> Void) {
            self.onCode = onCode
        }

        func dataScanner(_ dataScanner: DataScannerViewController, didAdd addedItems: [RecognizedItem], allItems: [RecognizedItem]) {
            guard !done else { return }
            for item in addedItems {
                if case .barcode(let code) = item, let text = code.payloadStringValue {
                    done = true
                    dataScanner.stopScanning()
                    onCode(text)
                    return
                }
            }
        }
    }
}
