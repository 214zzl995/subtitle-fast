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
}

struct MetricsGrid: View {
    let metrics: DetectionMetrics
    let subtitles: Int
    
    var body: some View {
        ViewThatFits(in: .horizontal) {
            Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 8) {
                GridRow {
                    label("ui.metrics_fps", systemImage: "speedometer")
                    Text(String(format: "%.1f", metrics.fps)).fontWeight(.semibold)
                    Text("fps").foregroundStyle(.secondary)
                    
                    label("ui.metrics_detection", systemImage: "waveform.path.ecg")
                    Text(String(format: "%.1f", metrics.det)).fontWeight(.semibold)
                    Text("ms").foregroundStyle(.secondary)
                }
                GridRow {
                    label("ui.metrics_ocr", systemImage: "text.viewfinder")
                    Text(String(format: "%.1f", metrics.ocr)).fontWeight(.semibold)
                    Text("ms").foregroundStyle(.secondary)
                    
                    label("ui.metrics_cues", systemImage: "text.bubble")
                    Text("\(subtitles)").fontWeight(.semibold)
                    Text(LocalizedStringKey("ui.metrics_cues_unit")).foregroundStyle(.secondary)
                }
                GridRow {
                    label("ui.metrics_empty", systemImage: "eye.slash")
                    Text("\(metrics.ocrEmpty)").fontWeight(.semibold)
                    Text(LocalizedStringKey("ui.metrics_empty_unit")).foregroundStyle(.secondary)
                }
            }
            VStack(alignment: .leading, spacing: 6) {
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
