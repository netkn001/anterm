import SwiftUI

struct WorkbenchView: View {
    @EnvironmentObject private var store: MobileWorkbenchStore
    @State private var selectedPaneForNav: MobilePaneItem?
    @State private var hostBeingRenamed: FfiPairedHost?
    @State private var renameDraft: String = ""

    var body: some View {
        ZStack(alignment: .bottom) {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    connectionHeader

                    if let err = store.errorBanner {
                        HStack(spacing: 8) {
                            Image(systemName: "exclamationmark.triangle.fill")
                                .foregroundStyle(.orange)
                            Text(err)
                                .font(.footnote)
                            Spacer()
                            Button("Dismiss") { store.errorBanner = nil }
                                .font(.caption)
                        }
                        .padding(12)
                        .background(.orange.opacity(0.14), in: RoundedRectangle(cornerRadius: 12))
                    }

                    if store.activeHost == nil {
                        pairedHostsSection
                    } else {
                        if !store.routes.isEmpty {
                            routesBar
                        }

                        if !store.waitingPanes.isEmpty {
                            waitingQueueSection
                        }

                        if !store.availableTags.isEmpty {
                            tagFilterBar
                        }

                        hybridTabsSection
                    }
                }
                .padding(.horizontal, 16)
                .padding(.top, 12)
                .padding(.bottom, 76)
            }
            .refreshable {
                await store.refreshWorkspace()
            }

            if let focusedPane = store.desktopFocusedPane, store.activeHost != nil {
                handoffCapsule(pane: focusedPane)
                    .padding(.bottom, 12)
            }
        }
        .background(Color(red: 0.06, green: 0.07, blue: 0.09).ignoresSafeArea())
        .navigationTitle("Anterm")
        .toolbar {
            ToolbarItemGroup(placement: .topBarTrailing) {
                if store.activeHost != nil {
                    Button {
                        Task { await store.refreshWorkspace() }
                    } label: {
                        Image(systemName: "arrow.clockwise")
                    }
                    .disabled(store.isRefreshingWorkspace)
                }
                Button {
                    store.showPairingSheet = true
                } label: {
                    Image(systemName: "qrcode.viewfinder")
                }
                .accessibilityLabel("Pair Desktop")
            }
        }
        .sheet(isPresented: $store.showPairingSheet) {
            PairingView()
                .environmentObject(store)
        }
        .navigationDestination(item: $selectedPaneForNav) { pane in
            ActivePaneView(pane: pane)
                .environmentObject(store)
        }
        .alert(
            "Rename Host",
            isPresented: Binding(
                get: { hostBeingRenamed != nil },
                set: { if !$0 { hostBeingRenamed = nil } }
            )
        ) {
            TextField("e.g. Office MacBook Pro", text: $renameDraft)
            Button("Cancel", role: .cancel) {
                hostBeingRenamed = nil
            }
            Button("Save") {
                if let target = hostBeingRenamed {
                    let trimmed = renameDraft.trimmingCharacters(in: .whitespacesAndNewlines)
                    if !trimmed.isEmpty {
                        store.renameHost(target, newName: trimmed)
                    }
                }
                hostBeingRenamed = nil
            }
        } message: {
            Text("Give this computer a recognizable name across your paired hosts.")
        }
    }

    private func handoffCapsule(pane: MobilePaneItem) -> some View {
        Button {
            selectedPaneForNav = pane
        } label: {
            HStack(spacing: 8) {
                Image(systemName: "arrow.triangle.2.circlepath")
                    .font(.caption.weight(.semibold))
                Text("Follow Desktop: \(pane.title)")
                    .font(.caption.weight(.semibold))
                    .lineLimit(1)
            }
            .padding(.horizontal, 14)
            .padding(.vertical, 9)
            .background(.ultraThinMaterial, in: Capsule())
            .overlay(Capsule().stroke(Color.accentColor.opacity(0.45), lineWidth: 1))
            .shadow(color: .black.opacity(0.35), radius: 10, y: 4)
        }
        .buttonStyle(.plain)
    }

    private var connectionHeader: some View {
        HStack(spacing: 10) {
            Circle()
                .fill(store.activeHost != nil ? Color.green : Color.secondary)
                .frame(width: 8, height: 8)
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(store.activeHost?.hostName ?? "No Host Connected")
                        .font(.subheadline.weight(.semibold))
                    if let active = store.activeHost {
                        Button {
                            renameDraft = active.hostName
                            hostBeingRenamed = active
                        } label: {
                            Image(systemName: "pencil")
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Rename Host")
                    }
                }
                Text(store.connectionStatus)
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            Spacer()
            if store.activeHost != nil {
                Button("Disconnect") {
                    store.disconnect()
                }
                .font(.caption.weight(.medium))
                .buttonStyle(.bordered)
            }
        }
        .padding(12)
        .background(Color.white.opacity(0.05), in: RoundedRectangle(cornerRadius: 12))
    }

    private var routesBar: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                Label(store.activeHost?.hostName ?? "Local", systemImage: "desktopcomputer")
                    .font(.caption.weight(.semibold))
                    .padding(.horizontal, 10)
                    .padding(.vertical, 5)
                    .background(Color.accentColor.opacity(0.2), in: Capsule())
                    .foregroundStyle(Color.accentColor)

                ForEach(store.routes, id: \.key) { route in
                    HStack(spacing: 5) {
                        Circle()
                            .fill(route.connected ? Color.green : Color.secondary)
                            .frame(width: 6, height: 6)
                        Text(route.hostLabel)
                            .font(.caption)
                    }
                    .padding(.horizontal, 10)
                    .padding(.vertical, 5)
                    .background(Color.white.opacity(0.06), in: Capsule())
                }
            }
        }
    }

    private var pairedHostsSection: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("Paired Hosts")
                .font(.headline)

            if store.pairedHosts.isEmpty {
                VStack(spacing: 12) {
                    Image(systemName: "desktopcomputer")
                        .font(.largeTitle)
                        .foregroundStyle(.secondary)
                    Text("Pair with Anterm Desktop (Settings → Mobile) or `anterm-server pair`")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .multilineTextAlignment(.center)
                    Button {
                        store.showPairingSheet = true
                    } label: {
                        Label("Pair New Host", systemImage: "qrcode.viewfinder")
                    }
                    .buttonStyle(.borderedProminent)
                }
                .frame(maxWidth: .infinity)
                .padding(.vertical, 32)
                .background(Color.white.opacity(0.04), in: RoundedRectangle(cornerRadius: 14))
            } else {
                ForEach(store.pairedHosts, id: \.hostNodeId) { host in
                    HStack(spacing: 10) {
                        VStack(alignment: .leading, spacing: 4) {
                            Text(host.hostName)
                                .font(.body.weight(.semibold))
                            Text(String(host.hostNodeId.prefix(16)) + "...")
                                .font(.system(.caption, design: .monospaced))
                                .foregroundStyle(.secondary)
                        }
                        Spacer()
                        Button {
                            renameDraft = host.hostName
                            hostBeingRenamed = host
                        } label: {
                            Image(systemName: "pencil")
                                .font(.subheadline)
                                .foregroundStyle(.secondary)
                                .padding(6)
                        }
                        .buttonStyle(.plain)
                        .accessibilityLabel("Rename \(host.hostName)")

                        Button("Connect") {
                            Task { await store.connect(host: host) }
                        }
                        .buttonStyle(.borderedProminent)
                    }
                    .padding(14)
                    .background(Color.white.opacity(0.05), in: RoundedRectangle(cornerRadius: 12))
                    .contextMenu {
                        Button {
                            renameDraft = host.hostName
                            hostBeingRenamed = host
                        } label: {
                            Label("Rename Host", systemImage: "pencil")
                        }
                        Button("Forget Host", role: .destructive) {
                            store.removeHost(host)
                        }
                    }
                }
            }
        }
    }

    /// Top-pinned Approval Queue for every waiting coding agent across all tabs.
    private var waitingQueueSection: some View {
        VStack(alignment: .leading, spacing: 10) {
            Label("Needs Attention (\(store.waitingPanes.count))", systemImage: "bell.badge.fill")
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(.orange)

            ForEach(store.waitingPanes) { pane in
                VStack(alignment: .leading, spacing: 10) {
                    HStack {
                        Text(pane.agentName?.capitalized ?? "Agent")
                            .font(.caption.weight(.bold))
                            .padding(.horizontal, 8)
                            .padding(.vertical, 3)
                            .background(.orange.opacity(0.2), in: Capsule())
                        Text(pane.title)
                            .font(.subheadline.weight(.medium))
                        Spacer()
                        Button("Inspect") {
                            selectedPaneForNav = pane
                        }
                        .font(.caption)
                    }
                    if let detail = pane.agentDetail {
                        Text(detail)
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                            .lineLimit(2)
                    }
                    HStack(spacing: 8) {
                        Button("Allow Once") {
                            store.sendQuickApproval(for: pane, action: .approveOnce)
                        }
                        .buttonStyle(.borderedProminent)
                        .controlSize(.small)

                        Button("Always") {
                            store.sendQuickApproval(for: pane, action: .approveAlways)
                        }
                        .buttonStyle(.bordered)
                        .controlSize(.small)

                        Button("Deny", role: .destructive) {
                            store.sendQuickApproval(for: pane, action: .deny)
                        }
                        .buttonStyle(.bordered)
                        .controlSize(.small)
                    }
                }
                .padding(12)
                .background(.orange.opacity(0.08), in: RoundedRectangle(cornerRadius: 12))
                .overlay(
                    RoundedRectangle(cornerRadius: 12)
                        .stroke(.orange.opacity(0.35), lineWidth: 1)
                )
            }
        }
    }

    private var tagFilterBar: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                tagPill(label: "All", active: store.selectedTag == nil) {
                    store.selectedTag = nil
                }
                ForEach(store.availableTags, id: \.self) { tag in
                    tagPill(label: tag, active: store.selectedTag == tag) {
                        store.selectedTag = tag
                    }
                }
            }
        }
    }

    private func tagPill(label: String, active: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(label)
                .font(.caption.weight(.medium))
                .padding(.horizontal, 12)
                .padding(.vertical, 6)
                .background(active ? Color.accentColor : Color.white.opacity(0.08), in: Capsule())
                .foregroundStyle(active ? Color.black : Color.primary)
        }
    }

    /// Hybrid Multi-Pane Tab Cards: one master card per desktop Tab with live
    /// sub-rows for active/waiting split panes. Independent of PC active focus.
    private var hybridTabsSection: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Text("Tabs")
                    .font(.headline)
                Spacer()
                Text("\(store.filteredTabs.count)")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }

            if store.filteredTabs.isEmpty {
                VStack(spacing: 8) {
                    Image(systemName: "rectangle.stack")
                        .font(.title2)
                        .foregroundStyle(.secondary)
                    Text("No active tabs reported by host.")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity)
                .padding(.vertical, 28)
                .background(Color.white.opacity(0.04), in: RoundedRectangle(cornerRadius: 12))
            } else {
                ForEach(store.filteredTabs) { tab in
                    VStack(alignment: .leading, spacing: 10) {
                        HStack {
                            Text(tab.title)
                                .font(.subheadline.weight(.semibold))
                            if tab.panes.count > 1 {
                                Text("⊞ \(tab.panes.count)")
                                    .font(.caption2.weight(.medium))
                                    .foregroundStyle(.secondary)
                            }
                            if tab.desktopFocused {
                                Text("Desktop Active")
                                    .font(.caption2.weight(.medium))
                                    .padding(.horizontal, 7)
                                    .padding(.vertical, 2)
                                    .background(Color.accentColor.opacity(0.2), in: Capsule())
                                    .foregroundStyle(Color.accentColor)
                            }
                            if tab.hibernated {
                                Text("Sleeping")
                                    .font(.caption2)
                                    .foregroundStyle(.secondary)
                            }
                            Spacer()
                            if let tag = tab.tag {
                                Text(tag)
                                    .font(.caption2)
                                    .foregroundStyle(.secondary)
                            }
                        }

                        ForEach(tab.panes) { pane in
                            Button {
                                selectedPaneForNav = pane
                            } label: {
                                HStack(spacing: 10) {
                                    statusDot(for: pane.agentStatus)
                                    VStack(alignment: .leading, spacing: 2) {
                                        Text(pane.title)
                                            .font(.subheadline)
                                            .foregroundStyle(.primary)
                                        if let cwd = pane.cwd {
                                            Text(cwd)
                                                .font(.system(.caption2, design: .monospaced))
                                                .foregroundStyle(.secondary)
                                                .lineLimit(1)
                                        }
                                    }
                                    Spacer()
                                    if !pane.listeningPorts.isEmpty {
                                        Text(":\(pane.listeningPorts[0])")
                                            .font(.system(.caption2, design: .monospaced))
                                            .padding(.horizontal, 6)
                                            .padding(.vertical, 2)
                                            .background(.green.opacity(0.16), in: Capsule())
                                            .foregroundStyle(.green)
                                    }
                                    if let agent = pane.agentName {
                                        Text(agent)
                                            .font(.caption2.weight(.medium))
                                            .padding(.horizontal, 6)
                                            .padding(.vertical, 2)
                                            .background(Color.white.opacity(0.08), in: Capsule())
                                    }
                                    Image(systemName: "chevron.right")
                                        .font(.caption2)
                                        .foregroundStyle(.tertiary)
                                }
                                .padding(10)
                                .background(Color.white.opacity(0.04), in: RoundedRectangle(cornerRadius: 10))
                            }
                            .buttonStyle(.plain)
                        }
                    }
                    .padding(14)
                    .background(Color.white.opacity(0.05), in: RoundedRectangle(cornerRadius: 14))
                    .contextMenu {
                        Button {
                            Task { await store.handoffFocusOnDesktop(tabId: tab.id) }
                        } label: {
                            Label("Focus This Tab on Desktop", systemImage: "macwindow")
                        }
                    }
                }
            }
        }
    }

    private func statusDot(for status: FfiAgentStatus) -> some View {
        let color: Color = switch status {
        case .working: .blue
        case .waiting: .orange
        case .error: .red
        case .idle: .green.opacity(0.75)
        }
        return Circle().fill(color).frame(width: 8, height: 8)
    }
}
