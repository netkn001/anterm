import AVFoundation
import SwiftUI

struct PairingView: View {
    @EnvironmentObject private var store: MobileWorkbenchStore
    @Environment(\.dismiss) private var dismiss

    @State private var uriText: String = ""
    @State private var deviceNameDraft: String = ""
    @State private var isPairing: Bool = false
    @State private var showScanner: Bool = false

    private var preview: FfiPairingInfo? {
        let trimmed = uriText.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        return store.parsePairingPreview(trimmed)
    }

    var body: some View {
        NavigationStack {
            Form {
                Section("Scan or Paste Pairing Code") {
                    #if !targetEnvironment(simulator)
                    Button {
                        showScanner = true
                    } label: {
                        Label("Scan QR Code with Camera", systemImage: "qrcode.viewfinder")
                    }
                    #endif

                    TextField("anterm://pair/v1?...", text: $uriText, axis: .vertical)
                        .font(.system(.footnote, design: .monospaced))
                        .textInputAutocapitalization(.never)
                        .autocorrectionDisabled()
                        .lineLimit(3...6)
                }

                if let info = preview {
                    Section("Host Preview") {
                        LabeledContent("Host", value: info.hostName)
                        LabeledContent("Node ID", value: String(info.hostNodeId.prefix(16)) + "...")
                        LabeledContent("Transport", value: info.relayUrl == nil ? "LAN Direct" : "P2P + Relay")
                        if info.isExpired {
                            Label("QR code has expired — generate a new one on Desktop.", systemImage: "exclamationmark.triangle.fill")
                                .foregroundStyle(.orange)
                        }
                    }
                }

                Section {
                    HStack {
                        Text("Device Name")
                        Spacer()
                        TextField("e.g. My iPhone 14 Plus", text: $deviceNameDraft)
                            .multilineTextAlignment(.trailing)
                            .onSubmit {
                                commitDeviceName()
                            }
                    }
                    LabeledContent("Node ID") {
                        Text(String(store.deviceNodeId.prefix(18)) + "...")
                            .font(.system(.caption, design: .monospaced))
                            .foregroundStyle(.secondary)
                    }
                } header: {
                    Text("This Device")
                } footer: {
                    Text("Shown in Desktop Settings → Mobile so you can tell multiple phones/iPads apart.")
                }
            }
            .onAppear {
                deviceNameDraft = store.deviceName
            }
            .onChange(of: deviceNameDraft) { _, newValue in
                let trimmed = newValue.trimmingCharacters(in: .whitespacesAndNewlines)
                if !trimmed.isEmpty && trimmed != store.deviceName {
                    store.updateDeviceName(trimmed)
                }
            }
            .navigationTitle("Pair Desktop")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
                ToolbarItem(placement: .confirmationAction) {
                    Button(isPairing ? "Pairing..." : "Pair") {
                        let cleaned = uriText.trimmingCharacters(in: .whitespacesAndNewlines)
                        isPairing = true
                        Task {
                            await store.pair(uri: cleaned)
                            isPairing = false
                        }
                    }
                    .disabled(preview == nil || preview?.isExpired == true || isPairing)
                }
            }
            .sheet(isPresented: $showScanner) {
                QRScannerView { scanned in
                    uriText = scanned
                    showScanner = false
                }
            }
        }
    }

    private func commitDeviceName() {
        let trimmed = deviceNameDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        if !trimmed.isEmpty {
            store.updateDeviceName(trimmed)
        }
    }
}

struct QRScannerView: UIViewControllerRepresentable {
    var onFound: (String) -> Void

    func makeUIViewController(context: Context) -> QRScannerController {
        let controller = QRScannerController()
        controller.onFound = onFound
        return controller
    }

    func updateUIViewController(_ uiViewController: QRScannerController, context: Context) {}
}

final class QRScannerController: UIViewController, AVCaptureMetadataOutputObjectsDelegate {
    var onFound: ((String) -> Void)?
    private let session = AVCaptureSession()
    private var previewLayer: AVCaptureVideoPreviewLayer?
    private var delivered = false

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .black

        guard let device = AVCaptureDevice.default(for: .video),
              let input = try? AVCaptureDeviceInput(device: device),
              session.canAddInput(input) else {
            return
        }
        session.addInput(input)

        let output = AVCaptureMetadataOutput()
        guard session.canAddOutput(output) else { return }
        session.addOutput(output)
        output.setMetadataObjectsDelegate(self, queue: .main)
        output.metadataObjectTypes = [.qr]

        let preview = AVCaptureVideoPreviewLayer(session: session)
        preview.videoGravity = .resizeAspectFill
        preview.frame = view.bounds
        view.layer.addSublayer(preview)
        self.previewLayer = preview

        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            self?.session.startRunning()
        }
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        previewLayer?.frame = view.bounds
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        if session.isRunning {
            session.stopRunning()
        }
    }

    func metadataOutput(
        _ output: AVCaptureMetadataOutput,
        didOutput metadataObjects: [AVMetadataObject],
        from connection: AVCaptureConnection
    ) {
        guard !delivered,
              let first = metadataObjects.first as? AVMetadataMachineReadableCodeObject,
              let value = first.stringValue,
              value.hasPrefix("anterm://pair/") else {
            return
        }
        delivered = true
        session.stopRunning()
        onFound?(value)
    }
}
