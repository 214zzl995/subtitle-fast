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
            .padding(.bottom, 45)
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
