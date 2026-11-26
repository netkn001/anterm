import SwiftUI
import WebKit

/// Native `WKWebView` sheet backed by `MobilePreviewProxy` (`127.0.0.1:<local_port>`),
/// tunneling HTTP & WebSocket HMR traffic directly over E2EE `iroh` QUIC without VPN.
struct WebPreviewSheet: View {
    let url: URL
    let onClose: () -> Void

    @State private var reloadToken = UUID()

    var body: some View {
        NavigationStack {
            PreviewWebView(url: url, reloadToken: reloadToken)
                .ignoresSafeArea(edges: .bottom)
                .navigationTitle(url.absoluteString)
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) {
                        Button("Close") { onClose() }
                    }
                    ToolbarItem(placement: .topBarTrailing) {
                        Button {
                            reloadToken = UUID()
                        } label: {
                            Image(systemName: "arrow.clockwise")
                        }
                    }
                }
        }
    }
}

private struct PreviewWebView: UIViewRepresentable {
    let url: URL
    let reloadToken: UUID

    func makeUIView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        config.allowsInlineMediaPlayback = true
        let webView = WKWebView(frame: .zero, configuration: config)
        webView.load(URLRequest(url: url))
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {
        if webView.url != url {
            webView.load(URLRequest(url: url))
        } else {
            webView.reload()
        }
    }
}
