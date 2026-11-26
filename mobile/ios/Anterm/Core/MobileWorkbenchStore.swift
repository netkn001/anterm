import Foundation
import SwiftUI
import UIKit

/// Visual rendering layer chosen for the active mobile pane.
/// Layers 1 and 2 have 0% impact on the desktop PTY dimensions or layout.
enum PaneRenderLayer: String, CaseIterable, Identifiable {
    case transcript = "Agent"
    case reflow = "Reflow"
    case metal = "Terminal"

    var id: String { rawValue }
}

/// Mobile-side tab card model combining desktop tab structure with live Agent metadata.
struct MobileTabItem: Identifiable, Hashable {
    let id: String
    let title: String
    let tag: String?
    let desktopFocused: Bool
    let hibernated: Bool
    var panes: [MobilePaneItem]
}

struct MobilePaneItem: Identifiable, Hashable {
    let id: UInt64
    var title: String
    var cwd: String?
    var agentName: String?
    var agentStatus: FfiAgentStatus
    var agentDetail: String?
    var hasTranscript: Bool
    var listeningPorts: [UInt16]
}

@MainActor
final class MobileWorkbenchStore: ObservableObject {
    @Published var pairedHosts: [FfiPairedHost] = []
    @Published var activeHost: FfiPairedHost?
    @Published var deviceName: String = UIDevice.current.name
    @Published var connectionStatus: String = "Disconnected"
    @Published var errorBanner: String?

    @Published var tabs: [MobileTabItem] = []
    @Published var routes: [FfiRemoteRoute] = []
    @Published var desktopActiveTabId: String?
    @Published var selectedTag: String? = nil
    @Published var isRefreshingWorkspace: Bool = false

    @Published var activePane: MobilePaneItem?
    @Published var renderLayer: PaneRenderLayer = .transcript
    @Published var transcriptEntries: [FfiTranscriptEntry] = []
    @Published var reflowLines: [FfiReflowLine] = []
    @Published var metalFrameBytes: Data = Data()
    @Published var hasSizeLease: Bool = false

    @Published var activePreviewURL: URL?
    @Published var showPairingSheet: Bool = false

    private var client: MobileClient?
    private var hostSession: MobileHostSession?
    private var paneSession: MobilePaneSession?
    private var transcriptSession: MobileTranscriptSession?
    private var previewProxy: MobilePreviewProxy?
    private var pollTask: Task<Void, Never>?
    private var workspaceEventTask: Task<Void, Never>?

    init() {
        bootstrapClient()
    }

    private func bootstrapClient() {
        let dir = FileManager.default
            .urls(for: .applicationSupportDirectory, in: .userDomainMask)
            .first?
            .appendingPathComponent("anterm-mobile", isDirectory: true)
            ?? FileManager.default.temporaryDirectory.appendingPathComponent("anterm-mobile")

        do {
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            let created = try MobileClient(
                storageDir: dir.path,
                deviceName: UIDevice.current.name
            )
            self.client = created
            self.deviceName = created.deviceName()
            self.pairedHosts = created.listPairedHosts()
        } catch {
            self.errorBanner = "Failed to initialize mobile core: \(error.localizedDescription)"
        }
    }

    var deviceNodeId: String {
        client?.deviceNodeId() ?? "unavailable"
    }

    var waitingPanes: [MobilePaneItem] {
        tabs.flatMap(\.panes).filter { $0.agentStatus == .waiting }
    }

    var availableTags: [String] {
        let raw = tabs.compactMap(\.tag)
        return Array(Set(raw)).sorted()
    }

    var filteredTabs: [MobileTabItem] {
        guard let selectedTag else { return tabs }
        return tabs.filter { $0.tag == selectedTag }
    }

    /// Returns the desktop's currently focused pane for explicit Handoff ("Follow Desktop Tab").
    var desktopFocusedPane: MobilePaneItem? {
        if let activeId = desktopActiveTabId,
           let tab = tabs.first(where: { $0.id == activeId }),
           let pane = tab.panes.first {
            return pane
        }
        return tabs.first(where: { $0.desktopFocused })?.panes.first
    }

    func parsePairingPreview(_ uri: String) -> FfiPairingInfo? {
        try? parsePairingUri(uri: uri)
    }

    func pair(uri: String) async {
        guard let client else { return }
        errorBanner = nil
        connectionStatus = "Pairing..."
        do {
            let host = try await Task.detached(priority: .userInitiated) {
                try client.pairWithUri(uri: uri)
            }.value
            pairedHosts = client.listPairedHosts()
            showPairingSheet = false
            await connect(host: host)
        } catch {
            connectionStatus = "Disconnected"
            errorBanner = error.localizedDescription
        }
    }

    func connect(host: FfiPairedHost) async {
        guard let client else { return }
        disconnect()
        errorBanner = nil
        connectionStatus = "Connecting to \(host.hostName)..."
        do {
            let session = try await Task.detached(priority: .userInitiated) {
                try client.connectHost(hostNodeId: host.hostNodeId)
            }.value
            self.hostSession = session
            self.activeHost = host
            self.connectionStatus = "Connected · E2EE QUIC"
            await refreshWorkspace()
            startWorkspaceEventLoop()
        } catch {
            self.connectionStatus = "Disconnected"
            self.errorBanner = error.localizedDescription
        }
    }

    func refreshWorkspace() async {
        guard let hostSession else { return }
        isRefreshingWorkspace = true
        defer { isRefreshingWorkspace = false }
        do {
            let snap = try await Task.detached(priority: .userInitiated) {
                try hostSession.fetchWorkspace()
            }.value
            applyWorkspaceSnapshot(snap)
        } catch {
            errorBanner = "Workspace sync failed: \(error.localizedDescription)"
        }
    }

    private func applyWorkspaceSnapshot(_ snap: FfiWorkspaceSnapshot) {
        desktopActiveTabId = snap.desktopActiveTabId
        routes = snap.routes
        tabs = snap.tabs.map { tab in
            MobileTabItem(
                id: tab.tabId,
                title: tab.title,
                tag: tab.tag,
                desktopFocused: tab.desktopFocused,
                hibernated: tab.hibernated,
                panes: tab.panes.map { p in
                    MobilePaneItem(
                        id: p.paneId,
                        title: p.title,
                        cwd: p.cwd,
                        agentName: p.agent,
                        agentStatus: p.agentStatus,
                        agentDetail: p.agentDetail,
                        hasTranscript: p.hasTranscript,
                        listeningPorts: p.listeningPorts
                    )
                }
            )
        }
        if let currentId = activePane?.id,
           let updated = tabs.flatMap(\.panes).first(where: { $0.id == currentId }) {
            activePane = updated
        }
    }

    /// Explicit Handoff: switch desktop's active tab to `tabId`.
    func handoffFocusOnDesktop(tabId: String) async {
        guard let hostSession else { return }
        do {
            try await Task.detached(priority: .userInitiated) {
                try hostSession.handoffFocusDesktopTab(tabId: tabId)
            }.value
            await refreshWorkspace()
        } catch {
            errorBanner = "Handoff failed: \(error.localizedDescription)"
        }
    }

    func updateDeviceName(_ newName: String) {
        guard let client else { return }
        do {
            let saved = try client.setDeviceName(name: newName)
            self.deviceName = saved
        } catch {
            self.errorBanner = error.localizedDescription
        }
    }

    func renameHost(_ host: FfiPairedHost, newName: String) {
        guard let client else { return }
        do {
            let updated = try client.renamePairedHost(
                hostNodeId: host.hostNodeId,
                newName: newName
            )
            pairedHosts = client.listPairedHosts()
            if activeHost?.hostNodeId == updated.hostNodeId {
                activeHost = updated
            }
        } catch {
            errorBanner = error.localizedDescription
        }
    }

    func removeHost(_ host: FfiPairedHost) {
        guard let client else { return }
        if activeHost?.hostNodeId == host.hostNodeId {
            disconnect()
        }
        try? client.removePairedHost(hostNodeId: host.hostNodeId)
        pairedHosts = client.listPairedHosts()
    }

    func disconnect() {
        workspaceEventTask?.cancel()
        workspaceEventTask = nil
        closeActivePane()
        stopPreview()
        hostSession?.disconnect()
        hostSession = nil
        activeHost = nil
        tabs = []
        routes = []
        desktopActiveTabId = nil
        connectionStatus = "Disconnected"
    }

    /// Opens a pane on iOS without altering the PC's active tab or PTY size.
    func openPane(_ pane: MobilePaneItem) async {
        closeActivePane()
        activePane = pane
        renderLayer = pane.hasTranscript ? .transcript : .reflow

        guard let hostSession else { return }
        do {
            let openedPane = try await Task.detached(priority: .userInitiated) {
                try hostSession.openPane(paneId: pane.id, resumeOffset: 0)
            }.value
            self.paneSession = openedPane

            if pane.hasTranscript {
                let openedTranscript = try? await Task.detached(priority: .userInitiated) {
                    try hostSession.openTranscript(paneId: pane.id, sinceSeq: 0)
                }.value
                if let openedTranscript {
                    self.transcriptSession = openedTranscript
                }
            }
            startPollingLoop()
        } catch {
            errorBanner = "Could not attach pane #\(pane.id): \(error.localizedDescription)"
        }
    }

    func closeActivePane() {
        pollTask?.cancel()
        pollTask = nil
        transcriptSession?.close()
        transcriptSession = nil
        paneSession?.close()
        paneSession = nil
        hasSizeLease = false
        transcriptEntries = []
        reflowLines = []
        metalFrameBytes = Data()
        activePane = nil
    }

    func sendRawText(_ text: String) {
        guard let paneSession else { return }
        Task.detached(priority: .userInitiated) {
            try? paneSession.sendText(text: text)
        }
    }

    func scrollActivePane(delta: Int32) {
        guard let paneSession else { return }
        Task.detached(priority: .userInitiated) {
            paneSession.scrollDisplay(delta: delta)
        }
    }

    func sendPrompt(_ text: String, attachments: [String] = []) {
        guard let paneSession else { return }
        let agent = activePane?.agentName ?? ""
        let chunks = buildAgentSubmitBytes(
            agent: agent,
            text: text,
            attachedPaths: attachments,
            bracketedPaste: true
        )
        Task.detached(priority: .userInitiated) {
            for chunk in chunks {
                try? paneSession.sendBytes(bytes: chunk)
                try? await Task.sleep(nanoseconds: 50_000_000)
            }
        }
    }

    func sendQuickApproval(for pane: MobilePaneItem? = nil, action: FfiQuickApproval) {
        let targetPane = pane ?? activePane
        if let paneSession, targetPane?.id == activePane?.id {
            let agent = targetPane?.agentName
            Task.detached(priority: .userInitiated) {
                try? paneSession.sendQuickApproval(agent: agent, action: action)
            }
            return
        }
        guard let hostSession, let targetPane else { return }
        Task.detached(priority: .userInitiated) {
            if let transient = try? hostSession.openPane(paneId: targetPane.id, resumeOffset: 0) {
                try? transient.sendQuickApproval(agent: targetPane.agentName, action: action)
                transient.close()
            }
        }
    }

    /// Layer 3 opt-in: explicitly lease PTY dimensions for fullscreen TUI apps.
    func toggleSizeLease(cols: UInt16 = 52, rows: UInt16 = 34) {
        guard let paneSession else { return }
        if hasSizeLease {
            try? paneSession.releaseSizeLease()
            hasSizeLease = false
        } else {
            try? paneSession.takeSizeLease(cols: cols, rows: rows)
            hasSizeLease = true
        }
    }

    // MARK: - Live Dev Workflows (Git Diff, File Browser, @file Search, Attachment Upload, Preview)

    func fetchGitDiff(cwd: String, filePath: String? = nil) async throws -> FfiGitDiffSnapshot {
        guard let hostSession else {
            throw NSError(domain: "Anterm", code: 1, userInfo: [NSLocalizedDescriptionKey: "Not connected"])
        }
        return try await Task.detached(priority: .userInitiated) {
            try hostSession.fetchGitDiff(cwd: cwd, filePath: filePath)
        }.value
    }

    func listDirectory(path: String) async throws -> [FfiFileEntry] {
        guard let hostSession else { return [] }
        return try await Task.detached(priority: .userInitiated) {
            try hostSession.listDirectory(dirPath: path)
        }.value
    }

    func readFileText(path: String, maxBytes: UInt32 = 256 * 1024) async throws -> String {
        guard let hostSession else { return "" }
        return try await Task.detached(priority: .userInitiated) {
            try hostSession.readFileText(filePath: path, maxBytes: maxBytes)
        }.value
    }

    func searchFiles(rootDir: String, query: String, limit: UInt32 = 12) async -> [FfiFileEntry] {
        guard let hostSession else { return [] }
        let hits = try? await Task.detached(priority: .utility) {
            try hostSession.searchFiles(rootDir: rootDir, query: query, limit: limit)
        }.value
        return hits ?? []
    }

    func uploadAttachment(filename: String, mime: String, data: Data) async throws -> String {
        guard let hostSession else {
            throw NSError(domain: "Anterm", code: 1, userInfo: [NSLocalizedDescriptionKey: "Not connected"])
        }
        return try await Task.detached(priority: .userInitiated) {
            try hostSession.uploadAttachment(filename: filename, mime: mime, data: data)
        }.value
    }

    func openWebPreview(port: UInt16) async {
        guard let hostSession else { return }
        stopPreview()
        let paneId = activePane?.id
        do {
            let proxy = try await Task.detached(priority: .userInitiated) {
                try hostSession.startPreviewProxy(remotePort: port, paneId: paneId)
            }.value
            self.previewProxy = proxy
            self.activePreviewURL = URL(string: proxy.localUrl())
        } catch {
            errorBanner = "Preview port \(port) unavailable: \(error.localizedDescription)"
        }
    }

    func stopPreview() {
        previewProxy?.stop()
        previewProxy = nil
        activePreviewURL = nil
    }

    private func startWorkspaceEventLoop() {
        guard let hostSession else { return }
        workspaceEventTask?.cancel()
        workspaceEventTask = Task { [weak self] in
            while !Task.isCancelled {
                let ev = await Task.detached(priority: .utility) {
                    hostSession.pollWorkspaceEvent(timeoutMs: 1500)
                }.value
                guard let self, !Task.isCancelled else { return }
                if ev != nil {
                    await self.refreshWorkspace()
                }
            }
        }
    }

    private func startPollingLoop() {
        let pane = paneSession
        let transcript = transcriptSession
        pollTask = Task { [weak self] in
            while !Task.isCancelled {
                guard let self else { return }
                if let transcript {
                    let batch = await Task.detached(priority: .utility) {
                        transcript.pollBatch(timeoutMs: 60)
                    }.value
                    if let batch {
                        if batch.reset {
                            self.transcriptEntries = batch.entries
                        } else {
                            self.transcriptEntries.append(contentsOf: batch.entries)
                        }
                    }
                }
                if let pane {
                    let currentLayer = self.renderLayer
                    if currentLayer == .metal {
                        let bytes = await Task.detached(priority: .userInitiated) {
                            pane.packMetalFrame()
                        }.value
                        if !bytes.isEmpty {
                            self.metalFrameBytes = Data(bytes)
                        }
                    } else if currentLayer == .reflow {
                        let lines = await Task.detached(priority: .utility) {
                            pane.reflowLines(targetCols: 44, maxLines: 400)
                        }.value
                        self.reflowLines = lines
                    }
                    self.hasSizeLease = pane.hasSizeLease()
                }
                try? await Task.sleep(nanoseconds: 50_000_000)
            }
        }
    }
}
