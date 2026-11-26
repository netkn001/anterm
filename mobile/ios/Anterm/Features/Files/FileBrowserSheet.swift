import SwiftUI

/// Remote host file tree browser and read-only code viewer powered by `ControlStream`,
/// with 1-tap `@file` mention insertion into the active Agent Composer.
struct FileBrowserSheet: View {
    let rootPath: String
    let onMentionPath: (String) -> Void

    @EnvironmentObject private var store: MobileWorkbenchStore
    @Environment(\.dismiss) private var dismiss

    @State private var currentPath: String = ""
    @State private var entries: [FfiFileEntry] = []
    @State private var previewFile: FfiFileEntry?
    @State private var previewText: String = ""
    @State private var isLoading: Bool = false
    @State private var errorText: String?

    var body: some View {
        NavigationStack {
            Group {
                if let file = previewFile {
                    filePreviewView(file: file)
                } else {
                    directoryListView
                }
            }
            .background(Color(red: 0.06, green: 0.07, blue: 0.09).ignoresSafeArea())
            .navigationTitle(
                previewFile?.name ?? ((currentPath as NSString).lastPathComponent.isEmpty ? rootPath : (currentPath as NSString).lastPathComponent)
            )
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    if previewFile != nil {
                        Button("Back") { previewFile = nil }
                    } else if currentPath != rootPath {
                        Button("Up") {
                            let parent = (currentPath as NSString).deletingLastPathComponent
                            if !parent.isEmpty {
                                Task { await loadDir(parent) }
                            }
                        }
                    } else {
                        Button("Close") { dismiss() }
                    }
                }
                ToolbarItem(placement: .confirmationAction) {
                    if let file = previewFile {
                        Button("@Mention") {
                            onMentionPath(file.path)
                            dismiss()
                        }
                    } else {
                        Button("Done") { dismiss() }
                    }
                }
            }
            .task {
                await loadDir(rootPath)
            }
        }
    }

    private var directoryListView: some View {
        List {
            if let errorText {
                Text(errorText)
                    .font(.footnote)
                    .foregroundStyle(.orange)
            }
            if isLoading {
                ProgressView()
            } else {
                ForEach(entries, id: \.path) { entry in
                    Button {
                        if entry.isDir {
                            Task { await loadDir(entry.path) }
                        } else {
                            Task { await openFile(entry) }
                        }
                    } label: {
                        HStack(spacing: 10) {
                            Image(systemName: entry.isDir ? "folder.fill" : "doc.text")
                                .foregroundStyle(entry.isDir ? Color.accentColor : Color.secondary)
                            Text(entry.name)
                                .font(.system(.subheadline, design: .monospaced))
                                .foregroundStyle(entry.ignored ? .secondary : .primary)
                            Spacer()
                            if !entry.isDir {
                                Button("@") {
                                    onMentionPath(entry.path)
                                    dismiss()
                                }
                                .font(.caption.weight(.bold))
                                .buttonStyle(.bordered)
                                .controlSize(.mini)
                            }
                        }
                    }
                    .buttonStyle(.plain)
                }
            }
        }
        .scrollContentBackground(.hidden)
    }

    private func filePreviewView(file: FfiFileEntry) -> some View {
        ScrollView([.vertical, .horizontal]) {
            Text(previewText)
                .font(.system(size: 12, design: .monospaced))
                .textSelection(.enabled)
                .padding(12)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func loadDir(_ path: String) async {
        isLoading = true
        errorText = nil
        currentPath = path
        do {
            entries = try await store.listDirectory(path: path)
        } catch {
            errorText = error.localizedDescription
        }
        isLoading = false
    }

    private func openFile(_ entry: FfiFileEntry) async {
        isLoading = true
        errorText = nil
        do {
            previewText = try await store.readFileText(path: entry.path)
            previewFile = entry
        } catch {
            errorText = error.localizedDescription
        }
        isLoading = false
    }
}
