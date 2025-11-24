import SwiftUI

struct ControlPanel: View {
    @ObservedObject var session: DetectionSession

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Label("ui.selection_summary", systemImage: "cursorarrow.motionlines")
                .font(.headline)

            SelectionSummary(session: session)

            Divider()

            Grid(alignment: .leading, horizontalSpacing: 12, verticalSpacing: 10) {
                GridRow {
                    sliderLabel(text: "ui.threshold", value: session.threshold)
                    Slider(value: $session.threshold, in: 0...255, step: 1)
                }
                GridRow {
                    sliderLabel(text: "ui.tolerance", value: session.tolerance)
                    Slider(value: $session.tolerance, in: 0...50, step: 1)
                }
            }

            HStack(spacing: 10) {
                if !session.subtitles.isEmpty {
                    Button {
                        session.exportSubtitles()
                    } label: {
                        Label("ui.export", systemImage: "square.and.arrow.up")
                    }
                    .controlSize(.small)
                }

                Spacer()

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
                    .font(.title3.weight(.semibold))
                    .padding(.horizontal, 18)
                    .padding(.vertical, 10)
                }
                .buttonStyle(.borderedProminent)
                .tint(session.activeStatus == .detecting ? .yellow : .accentColor)
                .controlSize(.large)
                .disabled(session.selection == nil || session.selectedFile == nil)
                
                if session.activeStatus == .detecting || session.activeStatus == .paused {
                    Button {
                        session.cancelDetection()
                    } label: {
                        Label("ui.cancel", systemImage: "stop.fill")
                    }
                    .buttonStyle(.bordered)
                    .tint(.red)
                }
            }
        }
        .padding(.vertical, 8)
        .padding(.horizontal, 10)
    }

    @ViewBuilder
    private func sliderLabel(text: String, value: Double) -> some View {
        HStack {
            Text(LocalizedStringKey(text))
                .font(.caption)
                .foregroundStyle(.secondary)
            Spacer()
            Text(String(format: "%.0f", value))
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
        }
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

struct SelectionSummary: View {
    @ObservedObject var session: DetectionSession

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Label("ui.region", systemImage: "square.dashed")
                Spacer()
                Text(regionText)
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
            HStack {
                Label("ui.threshold", systemImage: "sun.max")
                Spacer()
                Text(String(format: "%.0f / ±%.0f", session.threshold, session.tolerance))
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(.secondary)
            }

            if session.selectedFile == nil {
                Text("ui.placeholder_no_video")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
    }

    private var regionText: String {
        if let selection = session.selection {
            return String(
                format: "x:%.2f y:%.2f w:%.2f h:%.2f",
                selection.origin.x,
                selection.origin.y,
                selection.width,
                selection.height
            )
        }
        return NSLocalizedString("ui.region_none", comment: "no region")
    }
}
