import SwiftUI

enum DiffReviewMode {
    case transcriptEntry(FfiTranscriptEntry)
    case repository(cwd: String)
}

/// Mobile-first single-column Git Diff reviewer supporting both inline Agent
/// Edit cards and full live repository diffs via `ControlStream`. Tap any diff
/// line to quote it straight into the Agent Composer.
struct DiffReviewSheet: View {
    let mode: DiffReviewMode
    var onCommentToAgent: ((String) -> Void)?

    @EnvironmentObject private var store: MobileWorkbenchStore
    @Environment(\.dismiss) private var dismiss

    @State private var repoDiff: FfiGitDiffSnapshot?
    @State private var selectedFile: String?
    @State private var isLoading: Bool = false
    @State private var loadError: String?

    var body: some View {
        NavigationStack {
            VStack(spacing: 0) {
                if let snap = repoDiff, !snap.files.isEmpty {
                    changedFilesBar(snap: snap)
                }

                ScrollView {
                    VStack(alignment: .leading, spacing: 8) {
                        if isLoading {
                            ProgressView("Loading Git diff...")
                                .frame(maxWidth: .infinity)
                                .padding(.top, 48)
                        } else if let err = loadError {
                            Text(err)
                                .font(.footnote)
                                .foregroundStyle(.orange)
                                .padding()
                        } else {
                            switch mode {
                            case .transcriptEntry(let entry):
                                if let path = entry.filePath {
                                    Text(path)
                                        .font(.system(.footnote, design: .monospaced).weight(.semibold))
                                        .foregroundStyle(.secondary)
                                }
                                unifiedDiffLinesView(
                                    text: entry.body,
                                    defaultPath: entry.filePath ?? entry.title
                                )
                            case .repository:
                                if let snap = repoDiff {
                                    if snap.unifiedDiff.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
                                        Text("Working tree clean (no uncommitted diff).")
                                            .font(.subheadline)
                                            .foregroundStyle(.secondary)
                                            .frame(maxWidth: .infinity)
                                            .padding(.top, 48)
                                    } else {
                                        unifiedDiffLinesView(
                                            text: snap.unifiedDiff,
                                            defaultPath: selectedFile ?? snap.repoRoot
                                        )
                                    }
                                }
                            }
                        }
                    }
                    .padding(14)
                }
            }
            .background(Color(red: 0.06, green: 0.07, blue: 0.09).ignoresSafeArea())
            .navigationTitle(navigationTitle)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            .task {
                await loadIfRepository(file: nil)
            }
        }
    }

    private var navigationTitle: String {
        if let branch = repoDiff?.branch, !branch.isEmpty {
            return "Diff · \(branch)"
        }
        return "Diff Review"
    }

    private func loadIfRepository(file: String?) async {
        guard case .repository(let cwd) = mode else { return }
        isLoading = true
        loadError = nil
        do {
            repoDiff = try await store.fetchGitDiff(cwd: cwd, filePath: file)
        } catch {
            loadError = error.localizedDescription
        }
        isLoading = false
    }

    private func changedFilesBar(snap: FfiGitDiffSnapshot) -> some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                fileChip(title: "All (\(snap.files.count))", active: selectedFile == nil) {
                    selectedFile = nil
                    Task { await loadIfRepository(file: nil) }
                }
                ForEach(snap.files, id: \.path) { file in
                    fileChip(
                        title: "\(file.status) \((file.path as NSString).lastPathComponent)",
                        active: selectedFile == file.path
                    ) {
                        selectedFile = file.path
                        Task { await loadIfRepository(file: file.path) }
                    }
                }
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 8)
        }
        .background(Color.white.opacity(0.04))
    }

    private func fileChip(title: String, active: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(.system(.caption, design: .monospaced).weight(.medium))
                .padding(.horizontal, 10)
                .padding(.vertical, 5)
                .background(active ? Color.accentColor : Color.white.opacity(0.08), in: Capsule())
                .foregroundStyle(active ? Color.black : Color.primary)
        }
        .buttonStyle(.plain)
    }

    private func unifiedDiffLinesView(text: String, defaultPath: String) -> some View {
        let lines = text.split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        return LazyVStack(alignment: .leading, spacing: 1) {
            ForEach(Array(lines.enumerated()), id: \.offset) { idx, line in
                let style = lineStyle(for: line)
                Button {
                    if let onCommentToAgent {
                        onCommentToAgent("Regarding `\(defaultPath)` (diff line \(idx + 1): `\(line.trimmingCharacters(in: .whitespaces))`): ")
                        dismiss()
                    }
                } label: {
                    Text(line.isEmpty ? " " : line)
                        .font(.system(size: 12, design: .monospaced))
                        .foregroundStyle(style.fg)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .padding(.horizontal, 8)
                        .padding(.vertical, 2)
                        .background(style.bg)
                }
                .buttonStyle(.plain)
            }
        }
        .clipShape(RoundedRectangle(cornerRadius: 8))
    }

    private func lineStyle(for line: String) -> (fg: Color, bg: Color) {
        if line.hasPrefix("+++") || line.hasPrefix("---") || line.hasPrefix("diff ") {
            return (.secondary, Color.white.opacity(0.06))
        }
        if line.hasPrefix("@@") {
            return (.cyan, Color.cyan.opacity(0.12))
        }
        if line.hasPrefix("+") {
            return (.green, Color.green.opacity(0.12))
        }
        if line.hasPrefix("-") {
            return (.red, Color.red.opacity(0.12))
        }
        return (.primary.opacity(0.88), .clear)
    }
}
