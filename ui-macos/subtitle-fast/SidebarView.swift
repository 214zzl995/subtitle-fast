import SwiftUI

struct SidebarView: View {
    @ObservedObject var session: DetectionSession
    @Binding var showingFilePicker: Bool

    var body: some View {
        List {
            Section {
                Button {
                    showingFilePicker = true
                } label: {
                    Label("ui.open_file", systemImage: "folder.badge.plus")
                }
                .buttonStyle(.borderless)
            }

            Section(header: Text("ui.files")) {
                SidebarFiles(session: session)
            }
        }
        .listStyle(.sidebar)
        .scrollContentBackground(.hidden)
    }
}

struct SidebarFiles: View {
    @ObservedObject var session: DetectionSession

    var body: some View {
        if session.files.isEmpty {
            ContentUnavailableView("ui.no_file", systemImage: "film", description: Text("ui.placeholder_no_video"))
                .frame(maxWidth: .infinity, alignment: .leading)
        } else {
            ForEach(session.files) { file in
                Button {
                    session.activateFile(id: file.id)
                } label: {
                    fileRow(for: file)
                }
                .buttonStyle(.plain)
            }
        }
    }

    @ViewBuilder
    private func fileRow(for file: TrackedFile) -> some View {
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
