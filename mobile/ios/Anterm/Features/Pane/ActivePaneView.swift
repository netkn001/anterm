import PhotosUI
import SwiftUI

/// 3-Layer Zero-PC-Interference Pane Screen:
/// - Layer 1 (`.transcript`): Native SwiftUI Markdown + collapsible tool/diff cards (0% PC impact)
/// - Layer 2 (`.reflow`): Client-side logical-line reflow via `Flags::WRAPLINE` (0% PC impact)
/// - Layer 3 (`.metal`): Native Metal GPU grid with pinch-to-zoom Peek + explicit opt-in `SizeLease`
struct ActivePaneView: View {
    let pane: MobilePaneItem

    @EnvironmentObject private var store: MobileWorkbenchStore
    @State private var promptDraft: String = ""
    @State private var attachedHostPaths: [String] = []
    @State private var selectedPhotoItem: PhotosPickerItem?
    @State private var isUploadingAttachment: Bool = false

    @State private var mentionResults: [FfiFileEntry] = []
    @State private var showTranscriptDiffSheet: Bool = false
    @State private var selectedDiffEntry: FfiTranscriptEntry?
    @State private var showRepoDiffSheet: Bool = false
    @State private var showFileBrowserSheet: Bool = false

    private var livePane: MobilePaneItem {
        store.activePane ?? pane
    }

    var body: some View {
        VStack(spacing: 0) {
            layerSwitcherBar

            ZStack {
                switch store.renderLayer {
                case .transcript:
                    transcriptStreamView
                case .reflow:
                    reflowTerminalView
                case .metal:
                    metalLayerView
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)

            if livePane.agentStatus == .waiting {
                quickApprovalBar
            }

            if !mentionResults.isEmpty {
                mentionSuggestionStrip
            }

            if !attachedHostPaths.isEmpty {
                attachmentChipStrip
            }

            composerBar
            adaptiveKeyboardAccessoryBar
        }
        .background(Color(red: 0.06, green: 0.07, blue: 0.09).ignoresSafeArea())
        .navigationTitle(livePane.title)
        .navigationBarTitleDisplayMode(.inline)
        .toolbar {
            ToolbarItemGroup(placement: .topBarTrailing) {
                if livePane.cwd != nil {
                    Button {
                        showRepoDiffSheet = true
                    } label: {
                        Image(systemName: "plus.forwardslash.minus")
                    }
                    .accessibilityLabel("Git Diff")

                    Button {
                        showFileBrowserSheet = true
                    } label: {
                        Image(systemName: "folder")
                    }
                    .accessibilityLabel("Files")
                }

                if let firstPort = livePane.listeningPorts.first {
                    Button {
                        Task { await store.openWebPreview(port: firstPort) }
                    } label: {
                        Image(systemName: "safari")
                    }
                    .accessibilityLabel("Preview Port \(firstPort)")
                }

                if store.renderLayer == .metal {
                    Button {
                        store.toggleSizeLease()
                    } label: {
                        Image(
                            systemName: store.hasSizeLease
                                ? "arrow.up.left.and.arrow.down.right"
                                : "rectangle.compress.vertical"
                        )
                    }
                    .tint(store.hasSizeLease ? .orange : .accentColor)
                    .accessibilityLabel(store.hasSizeLease ? "Release Width" : "Fit Phone")
                }
            }
        }
        .task(id: pane.id) {
            await store.openPane(pane)
        }
        .onDisappear {
            store.closeActivePane()
        }
        .onChange(of: promptDraft) { _, newValue in
            updateMentionSuggestions(for: newValue)
        }
        .onChange(of: selectedPhotoItem) { _, newItem in
            guard let newItem else { return }
            Task { await handlePhotoSelection(newItem) }
        }
        .sheet(isPresented: $showTranscriptDiffSheet) {
            if let entry = selectedDiffEntry {
                DiffReviewSheet(
                    mode: .transcriptEntry(entry),
                    onCommentToAgent: { snippet in
                        appendCommentDraft(snippet)
                    }
                )
            }
        }
        .sheet(isPresented: $showRepoDiffSheet) {
            if let cwd = livePane.cwd {
                DiffReviewSheet(
                    mode: .repository(cwd: cwd),
                    onCommentToAgent: { snippet in
                        appendCommentDraft(snippet)
                    }
                )
                .environmentObject(store)
            }
        }
        .sheet(isPresented: $showFileBrowserSheet) {
            if let cwd = livePane.cwd {
                FileBrowserSheet(rootPath: cwd) { path in
                    let formatted = formatAgentMention(
                        agent: livePane.agentName ?? "",
                        path: path
                    )
                    if !promptDraft.isEmpty && !promptDraft.hasSuffix(" ") {
                        promptDraft.append(" ")
                    }
                    promptDraft.append(formatted + " ")
                }
                .environmentObject(store)
            }
        }
        .sheet(isPresented: Binding(
            get: { store.activePreviewURL != nil },
            set: { if !$0 { store.stopPreview() } }
        )) {
            if let url = store.activePreviewURL {
                WebPreviewSheet(url: url) {
                    store.stopPreview()
                }
            }
        }
    }

    private func appendCommentDraft(_ snippet: String) {
        if !promptDraft.isEmpty && !promptDraft.hasSuffix("\n") {
            promptDraft.append("\n")
        }
        promptDraft.append(snippet)
    }

    private func updateMentionSuggestions(for draft: String) {
        guard let cwd = livePane.cwd,
              let atIndex = draft.lastIndex(of: "@") else {
            mentionResults = []
            return
        }
        let afterAt = draft[draft.index(after: atIndex)...]
        if afterAt.contains(" ") || afterAt.contains("\n") {
            mentionResults = []
            return
        }
        let query = String(afterAt)
        guard !query.isEmpty else {
            mentionResults = []
            return
        }
        Task {
            let hits = await store.searchFiles(rootDir: cwd, query: query, limit: 8)
            mentionResults = hits
        }
    }

    private func applyMention(_ entry: FfiFileEntry) {
        let mentionText = formatAgentMention(agent: livePane.agentName ?? "", path: entry.path)
        if let atIndex = promptDraft.lastIndex(of: "@") {
            promptDraft.replaceSubrange(atIndex..<promptDraft.endIndex, with: mentionText + " ")
        } else {
            promptDraft.append(mentionText + " ")
        }
        mentionResults = []
    }

    private func handlePhotoSelection(_ item: PhotosPickerItem) async {
        isUploadingAttachment = true
        defer {
            isUploadingAttachment = false
            selectedPhotoItem = nil
        }
        do {
            guard let data = try await item.loadTransferable(type: Data.self) else { return }
            let uploadedPath = try await store.uploadAttachment(
                filename: "ios-\(Int(Date().timeIntervalSince1970)).jpg",
                mime: "image/jpeg",
                data: data
            )
            attachedHostPaths.append(uploadedPath)
        } catch {
            store.errorBanner = "Upload failed: \(error.localizedDescription)"
        }
    }

    private var layerSwitcherBar: some View {
        HStack(spacing: 8) {
            Picker("Mode", selection: $store.renderLayer) {
                ForEach(PaneRenderLayer.allCases) { layer in
                    Text(layer.rawValue).tag(layer)
                }
            }
            .pickerStyle(.segmented)

            if store.hasSizeLease {
                Text("LEASE")
                    .font(.caption2.weight(.bold))
                    .padding(.horizontal, 6)
                    .padding(.vertical, 3)
                    .background(.orange.opacity(0.22), in: Capsule())
                    .foregroundStyle(.orange)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(Color.white.opacity(0.04))
    }

    // MARK: - Layer 1: Native Agent Transcript Stream (0% PC PTY impact)

    private var transcriptStreamView: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 12) {
                    if store.transcriptEntries.isEmpty {
                        VStack(spacing: 8) {
                            Image(systemName: "sparkles")
                                .font(.title2)
                                .foregroundStyle(.secondary)
                            Text("Waiting for agent transcript...")
                                .font(.subheadline)
                                .foregroundStyle(.secondary)
                            Text("Zero interference with desktop PTY layout.")
                                .font(.caption)
                                .foregroundStyle(.tertiary)
                        }
                        .frame(maxWidth: .infinity)
                        .padding(.top, 48)
                    } else {
                        ForEach(store.transcriptEntries, id: \.seq) { entry in
                            TranscriptEntryCard(entry: entry) {
                                selectedDiffEntry = entry
                                showTranscriptDiffSheet = true
                            }
                            .id(entry.seq)
                        }
                    }
                }
                .padding(14)
            }
            .onChange(of: store.transcriptEntries.last?.seq) { _, lastSeq in
                if let lastSeq {
                    withAnimation(.easeOut(duration: 0.15)) {
                        proxy.scrollTo(lastSeq, anchor: .bottom)
                    }
                }
            }
        }
    }

    // MARK: - Layer 2: Zero-PC-Impact Reflow Terminal

    private var reflowTerminalView: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 2) {
                ForEach(store.reflowLines, id: \.rowIndex) { line in
                    Text(line.text.isEmpty ? " " : line.text)
                        .font(.system(size: 13, weight: .regular, design: .monospaced))
                        .foregroundStyle(line.isWrappedContinuation ? Color.white.opacity(0.86) : Color.white)
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
            }
            .padding(12)
        }
    }

    // MARK: - Layer 3: Native Metal GPU Grid (Peek + Opt-in SizeLease)

    private var metalLayerView: some View {
        VStack(spacing: 0) {
            if !store.hasSizeLease {
                HStack(spacing: 6) {
                    Image(systemName: "eye")
                        .font(.caption2)
                    Text("Peek mode (pinch to zoom, desktop width untouched).")
                        .font(.caption2)
                    Spacer()
                }
                .foregroundStyle(.secondary)
                .padding(.horizontal, 12)
                .padding(.vertical, 5)
                .background(Color.white.opacity(0.03))
            }
            MetalTerminalView(
                frameBytes: store.metalFrameBytes,
                onScroll: { delta in
                    store.scrollActivePane(delta: delta)
                }
            )
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
    }

    // MARK: - Quick Approval + @file Suggestions + Attachments + Composer

    private var quickApprovalBar: some View {
        HStack(spacing: 8) {
            Button("Allow Once") {
                store.sendQuickApproval(action: .approveOnce)
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.small)

            Button("Always") {
                store.sendQuickApproval(action: .approveAlways)
            }
            .buttonStyle(.bordered)
            .controlSize(.small)

            Button("Deny", role: .destructive) {
                store.sendQuickApproval(action: .deny)
            }
            .buttonStyle(.bordered)
            .controlSize(.small)

            Spacer()

            Button("Stop") {
                store.sendQuickApproval(action: .interrupt)
            }
            .buttonStyle(.bordered)
            .controlSize(.small)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(.orange.opacity(0.10))
    }

    private var mentionSuggestionStrip: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                ForEach(mentionResults, id: \.path) { hit in
                    Button {
                        applyMention(hit)
                    } label: {
                        Label(hit.name, systemImage: hit.isDir ? "folder" : "doc.text")
                            .font(.caption)
                            .padding(.horizontal, 9)
                            .padding(.vertical, 5)
                            .background(Color.accentColor.opacity(0.18), in: Capsule())
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
        }
        .background(Color.white.opacity(0.03))
    }

    private var attachmentChipStrip: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                ForEach(attachedHostPaths, id: \.self) { path in
                    HStack(spacing: 4) {
                        Image(systemName: "photo")
                            .font(.caption2)
                        Text((path as NSString).lastPathComponent)
                            .font(.caption2)
                        Button {
                            attachedHostPaths.removeAll { $0 == path }
                        } label: {
                            Image(systemName: "xmark.circle.fill")
                                .font(.caption2)
                        }
                    }
                    .padding(.horizontal, 8)
                    .padding(.vertical, 4)
                    .background(Color.white.opacity(0.10), in: Capsule())
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 5)
        }
        .background(Color.white.opacity(0.03))
    }

    private var composerBar: some View {
        HStack(alignment: .bottom, spacing: 8) {
            PhotosPicker(selection: $selectedPhotoItem, matching: .images) {
                if isUploadingAttachment {
                    ProgressView()
                        .controlSize(.small)
                        .frame(width: 28, height: 28)
                } else {
                    Image(systemName: "plus.circle.fill")
                        .font(.system(size: 24))
                        .foregroundStyle(.secondary)
                }
            }
            .disabled(isUploadingAttachment)

            TextField("Message agent or command...", text: $promptDraft, axis: .vertical)
                .lineLimit(1...5)
                .padding(.horizontal, 12)
                .padding(.vertical, 8)
                .background(Color.white.opacity(0.07), in: RoundedRectangle(cornerRadius: 16))
                .font(.subheadline)

            Button {
                let text = promptDraft.trimmingCharacters(in: .whitespacesAndNewlines)
                guard !text.isEmpty || !attachedHostPaths.isEmpty else { return }
                if livePane.agentName != nil {
                    store.sendPrompt(text, attachments: attachedHostPaths)
                } else {
                    let combined = ([text] + attachedHostPaths).filter { !$0.isEmpty }.joined(separator: " ")
                    store.sendRawText(combined + "\r")
                }
                promptDraft = ""
                attachedHostPaths = []
                mentionResults = []
            } label: {
                Image(systemName: "arrow.up.circle.fill")
                    .font(.system(size: 28))
            }
            .disabled(
                promptDraft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                    && attachedHostPaths.isEmpty
            )
        }
        .padding(.horizontal, 12)
        .padding(.top, 8)
        .padding(.bottom, 6)
        .background(Color(red: 0.08, green: 0.09, blue: 0.12))
    }

    private var adaptiveKeyboardAccessoryBar: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                keyChip("Esc") { store.sendRawText("\u{1B}") }
                keyChip("Tab") { store.sendRawText("\t") }
                keyChip("Ctrl+C") { store.sendRawText("\u{03}") }
                keyChip("↑") { store.sendRawText("\u{1B}[A") }
                keyChip("↓") { store.sendRawText("\u{1B}[B") }
                keyChip("/") { promptDraft.append("/") }
                keyChip("@") { promptDraft.append("@") }
                keyChip("|") { store.sendRawText("|") }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
        }
        .background(Color(red: 0.07, green: 0.08, blue: 0.10))
    }

    private func keyChip(_ label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(label)
                .font(.system(.caption, design: .monospaced).weight(.medium))
                .padding(.horizontal, 10)
                .padding(.vertical, 5)
                .background(Color.white.opacity(0.09), in: RoundedRectangle(cornerRadius: 7))
                .foregroundStyle(.primary)
        }
        .buttonStyle(.plain)
    }
}

private struct TranscriptEntryCard: View {
    let entry: FfiTranscriptEntry
    let onOpenDiff: () -> Void

    @State private var expanded: Bool = false

    var body: some View {
        switch entry.kind {
        case .userPrompt:
            HStack {
                Spacer(minLength: 32)
                Text(entry.body)
                    .font(.subheadline)
                    .padding(12)
                    .background(Color.accentColor.opacity(0.22), in: RoundedRectangle(cornerRadius: 14))
            }

        case .assistantMarkdown:
            VStack(alignment: .leading, spacing: 6) {
                if let attributed = try? AttributedString(markdown: entry.body) {
                    Text(attributed)
                        .font(.subheadline)
                        .textSelection(.enabled)
                } else {
                    Text(entry.body)
                        .font(.subheadline)
                        .textSelection(.enabled)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(12)
            .background(Color.white.opacity(0.05), in: RoundedRectangle(cornerRadius: 12))

        case .thinking:
            DisclosureGroup(isExpanded: $expanded) {
                Text(entry.body)
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .padding(.top, 4)
            } label: {
                Label("Thinking", systemImage: "brain")
                    .font(.caption.weight(.medium))
                    .foregroundStyle(.secondary)
            }
            .padding(10)
            .background(Color.white.opacity(0.03), in: RoundedRectangle(cornerRadius: 10))

        case .toolCall, .toolResult:
            DisclosureGroup(isExpanded: $expanded) {
                Text(entry.body)
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(entry.isError ? .red : .secondary)
                    .padding(.top, 4)
                    .textSelection(.enabled)
            } label: {
                HStack(spacing: 6) {
                    Image(systemName: entry.isError ? "xmark.octagon.fill" : "wrench.and.screwdriver")
                        .foregroundStyle(entry.isError ? .red : .accentColor)
                    Text(entry.title.isEmpty ? (entry.toolName ?? "Tool") : entry.title)
                        .font(.caption.weight(.semibold))
                }
            }
            .padding(10)
            .background(Color.white.opacity(0.04), in: RoundedRectangle(cornerRadius: 10))

        case .editDiff:
            Button(action: onOpenDiff) {
                HStack {
                    Image(systemName: "doc.text.magnifyingglass")
                        .foregroundStyle(.green)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(entry.filePath ?? entry.title)
                            .font(.system(.caption, design: .monospaced).weight(.semibold))
                            .foregroundStyle(.primary)
                        Text("Tap to inspect inline diff")
                            .font(.caption2)
                            .foregroundStyle(.secondary)
                    }
                    Spacer()
                    Image(systemName: "chevron.right")
                        .font(.caption2)
                        .foregroundStyle(.tertiary)
                }
                .padding(11)
                .background(.green.opacity(0.09), in: RoundedRectangle(cornerRadius: 10))
                .overlay(
                    RoundedRectangle(cornerRadius: 10)
                        .stroke(.green.opacity(0.28), lineWidth: 1)
                )
            }
            .buttonStyle(.plain)

        case .systemNotice:
            Text(entry.body)
                .font(.caption)
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, alignment: .center)
                .padding(.vertical, 4)
        }
    }
}
