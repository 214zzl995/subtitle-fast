import SwiftUI

struct SidebarView: View {
    @ObservedObject var session: DetectionSession

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label {
                Text("ui.files")
                    .font(.headline)
            } icon: {
                Image(systemName: "film.stack")
                    .font(.headline.weight(.semibold))
            }
            .padding(.leading, 6)
            SidebarFiles(session: session)
        }
        .padding(.vertical, 6)
        .padding(.horizontal, 4)
    }
}

struct SidebarFiles: View {
    @ObservedObject var session: DetectionSession
    @State private var filePendingRemovalID: UUID?
    @State private var filePendingRemovalName: String = ""
    @State private var showingRemoveConfirm = false

    var body: some View {
        ScrollView {
            if session.files.isEmpty {
                Color.clear
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else {
                LazyVStack(spacing: 8) {
                    ForEach(session.files) { file in
                        fileRow(for: file)
                    }
                }
            }
        }
        .overlay(alignment: .center) {
            if session.files.isEmpty {
                VStack(spacing: 10) {
                    Image(systemName: "film")
                        .font(.system(size: 30, weight: .light))
                        .foregroundStyle(.secondary)
                    Text("ui.placeholder_no_video")
                        .font(.system(size: 15, weight: .light))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                .padding(.vertical, 16)
                .padding(.horizontal, 8)
            }
        }
        .alert(isPresented: $showingRemoveConfirm) {
            Alert(
                title: Text(NSLocalizedString("ui.remove_confirm_title", comment: "Remove job title")),
                message: Text(
                    String(
                        format: NSLocalizedString("ui.remove_confirm_message", comment: "Remove job message"),
                        filePendingRemovalName
                    )
                ),
                primaryButton: .destructive(Text(NSLocalizedString("ui.remove", comment: "Remove job"))) {
                    if let id = filePendingRemovalID {
                        session.removeFile(id: id)
                    }
                    filePendingRemovalID = nil
                    showingRemoveConfirm = false
                },
                secondaryButton: .cancel {
                    filePendingRemovalID = nil
                    showingRemoveConfirm = false
                }
            )
        }
    }

    @ViewBuilder
    private func fileRow(for file: TrackedFile) -> some View {
        let isDetecting = isDetecting(file)
        let isPaused = isPaused(file)
        HStack(alignment: .center, spacing: 8) {
            fileCard(for: file)
                .frame(maxWidth: .infinity, alignment: .leading)
                .contentShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                .onTapGesture {
                    session.activateFile(id: file.id)
                }
        }
        .padding(.horizontal, 4)
        .contextMenu {
            Button(role: .destructive) {
                filePendingRemovalID = file.id
                filePendingRemovalName = file.url.lastPathComponent
                showingRemoveConfirm = true
            } label: {
                Label(NSLocalizedString("ui.remove", comment: "Remove job"), systemImage: "trash")
            }
            .disabled(isDetecting)

            Button {
                session.pauseFile(id: file.id)
            } label: {
                Label(NSLocalizedString("ui.pause", comment: "Pause job"), systemImage: "pause.fill")
            }
            .disabled(!isDetecting)

            Button {
                session.resumeFile(id: file.id)
            } label: {
                Label(NSLocalizedString("ui.resume", comment: "Resume job"), systemImage: "play.fill")
            }
            .disabled(!isPaused)
        }
    }

    @ViewBuilder
    private func fileCard(for file: TrackedFile) -> some View {
        let isActive = session.activeFileID == file.id
        let isDone: Bool = {
            if case .completed = file.status { return true }
            return false
        }()
        let isDetecting: Bool = {
            switch file.status {
            case .detecting, .paused:
                return true
            default:
                return false
            }
        }()
        VStack(alignment: .leading, spacing: 6) {
            HStack(alignment: .center, spacing: 10) {
                Image(systemName: isActive ? "play.rectangle.fill" : "film")
                    .foregroundStyle(isActive ? Color.accentColor : Color.secondary)

                VStack(alignment: .leading, spacing: 2) {
                    Text(file.url.lastPathComponent)
                        .font(.subheadline.weight(.semibold))
                        .lineLimit(1)

                    Text(statusLabel(for: file))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Spacer()
                if file.progress > 0 || isDone {
                    Text(String(format: "%.0f%%", min(max(file.progress, 0), 1) * 100))
                        .font(.caption.monospacedDigit())
                        .foregroundStyle(.secondary)
                }
            }

            ProgressView(value: file.progress, total: 1.0)
                .progressViewStyle(.linear)
                .opacity(isDetecting || file.progress > 0 ? 1 : 0.25)
        }
        .padding(10)
        .background(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .fill(isActive ? Color.accentColor.opacity(0.08) : Color.clear)
        )
        .contentShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
    }

    private func isDetecting(_ file: TrackedFile) -> Bool {
        if case .detecting = file.status { return true }
        return false
    }

    private func isPaused(_ file: TrackedFile) -> Bool {
        if case .paused = file.status { return true }
        return false
    }

    private func statusLabel(for file: TrackedFile) -> String {
        switch file.status {
        case .idle:
            return NSLocalizedString("ui.status_idle", comment: "idle")
        case .detecting:
            return NSLocalizedString("ui.detecting", comment: "detecting")
        case .completed:
            return NSLocalizedString("ui.status_completed", comment: "completed")
        case .failed:
            return NSLocalizedString("ui.status_failed", comment: "failed")
        case .canceled:
            return NSLocalizedString("ui.status_canceled", comment: "canceled")
        case .paused:
            return NSLocalizedString("ui.status_paused", comment: "paused")
        }
    }
}
