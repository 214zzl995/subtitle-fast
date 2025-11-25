import SwiftUI

struct StatusPanel: View {
    @ObservedObject var session: DetectionSession

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Label("ui.detection_progress", systemImage: "speedometer")
                    .font(.headline)
                Spacer()
                if session.activeStatus == .detecting {
                    ProgressView()
                        .controlSize(.small)
                }
            }

            Text(statusTitle)
                .font(.subheadline)
                .foregroundStyle(.secondary)

            ProgressView(value: session.progress, total: 1.0) {
                Text(String(format: "%.0f%%", session.progress * 100))
                    .font(.caption.monospacedDigit())
            }
            .progressViewStyle(.linear)

            MetricsGrid(metrics: session.metrics, subtitles: session.subtitles.count)

            actionButtons

            if let message = session.errorMessage {
                Text(message)
                    .font(.caption)
                    .foregroundStyle(.red)
            }
        }
        .padding(.vertical, 10)
        .padding(.horizontal, 10)
    }

    private var statusTitle: String {
        switch session.activeStatus {
        case .idle:
            return session.selection == nil
                ? NSLocalizedString("ui.select_region", comment: "select region")
                : NSLocalizedString("ui.status_ready", comment: "ready")
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

    private var actionButtons: some View {
        actionButtonsRow
            .frame(maxWidth: .infinity, alignment: .center)
            .animation(.easeInOut(duration: 0.2), value: isCompactActions)
    }

    private var startDisabled: Bool {
        session.selection == nil || session.selectedFile == nil
    }

    private var cancelDisabled: Bool {
        session.selectedFile == nil
    }

    private var isCompactActions: Bool {
        session.activeStatus == .detecting || session.activeStatus == .paused
    }

    private var actionButtonWidth: CGFloat {
        isCompactActions ? 96 : 120
    }

    private var actionControlSize: ControlSize {
        .regular
    }

    private var actionButtonsRow: some View {
        HStack(spacing: 10) {
            primaryActionButton

            if isCompactActions {
                cancelButton
                    .transition(.move(edge: .trailing).combined(with: .opacity))
            }
        }
        .frame(maxWidth: .infinity, alignment: .center)
    }

    private var primaryActionButton: some View {
        Button {
            switch session.activeStatus {
            case .detecting:
                session.pauseDetection()
            case .paused:
                session.resumeDetection()
            default:
                session.startDetection()
            }
        } label: {
            Label(
                primaryButtonTitle(for: session.activeStatus),
                systemImage: primaryButtonIcon(for: session.activeStatus)
            )
            .font(.headline.weight(.semibold))
        }
        .buttonStyle(.borderedProminent)
        .tint(session.activeStatus == .detecting ? .yellow : .accentColor)
        .controlSize(actionControlSize)
        .frame(width: actionButtonWidth)
        .disabled(startDisabled)
    }

    private var cancelButton: some View {
        Button {
            session.cancelDetection()
        } label: {
            Label("ui.cancel", systemImage: "stop.fill")
                .font(.headline.weight(.semibold))
        }
        .buttonStyle(.bordered)
        .tint(.red)
        .controlSize(actionControlSize)
        .frame(width: actionButtonWidth)
        .disabled(cancelDisabled)
    }

    private func primaryButtonTitle(for status: DetectionStatus) -> LocalizedStringKey {
        switch status {
        case .detecting:
            return "ui.pause"
        case .paused:
            return "ui.resume"
        default:
            return "ui.start_detection"
        }
    }

    private func primaryButtonIcon(for status: DetectionStatus) -> String {
        switch status {
        case .detecting:
            return "pause.fill"
        default:
            return "play.fill"
        }
    }

}

struct MetricsGrid: View {
    let metrics: DetectionMetrics
    let subtitles: Int
    
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            compactRow(
                key: "ui.metrics_fps",
                value: String(format: "%.1f", metrics.fps),
                unit: "fps",
                systemImage: "speedometer"
            )
            compactRow(
                key: "ui.metrics_detection",
                value: String(format: "%.1f", metrics.det),
                unit: "ms",
                systemImage: "waveform.path.ecg"
            )
            compactRow(
                key: "ui.metrics_ocr",
                value: String(format: "%.1f", metrics.ocr),
                unit: "ms",
                systemImage: "text.viewfinder"
            )
            compactRow(
                key: "ui.metrics_cues",
                value: "\(subtitles)",
                unitKey: "ui.metrics_cues_unit",
                systemImage: "text.bubble"
            )
            compactRow(
                key: "ui.metrics_empty",
                value: "\(metrics.ocrEmpty)",
                unitKey: "ui.metrics_empty_unit",
                systemImage: "eye.slash"
            )
        }
        .font(.caption.monospacedDigit())
    }
    
    private func label(_ key: String, systemImage: String) -> some View {
        Label(LocalizedStringKey(key), systemImage: systemImage)
            .foregroundStyle(.secondary)
    }
    
    private func compactRow(
        key: String,
        value: String,
        unit: String? = nil,
        unitKey: String? = nil,
        systemImage: String
    ) -> some View {
        HStack(spacing: 6) {
            Label(LocalizedStringKey(key), systemImage: systemImage)
                .foregroundStyle(.secondary)
            Spacer()
            Text(value)
                .fontWeight(.semibold)
            if let unitKey {
                Text(LocalizedStringKey(unitKey))
                    .foregroundStyle(.secondary)
            } else if let unit {
                Text(unit)
                    .foregroundStyle(.secondary)
            }
        }
    }
}
