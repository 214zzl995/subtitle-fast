import AVKit
import SwiftUI

struct ContentView: View {
    @StateObject private var session = DetectionSession()
    @State private var language = AppLanguage.systemDefault()

    var body: some View {
        NavigationStack {
            HStack(spacing: 16) {
                VStack(alignment: .leading, spacing: 12) {
                    FrameCanvasView(session: session)
                    ControlStackView(session: session)
                }
                .frame(minWidth: 520)

                Divider()

                SubtitlesListView(session: session)
                    .frame(minWidth: 280, idealWidth: 320, maxWidth: 360)
            }
            .padding()
            .toolbar {
                ToolbarItemGroup(placement: .navigation) {
                    Button {
                        session.pickFile()
                    } label: {
                        Label("ui.open_file", systemImage: "folder")
                    }
                    Button {
                        session.resetSelection()
                    } label: {
                        Label("ui.reset_selection", systemImage: "crop")
                    }
                    .disabled(session.selection == nil)
                }

                ToolbarItemGroup(placement: .automatic) {
                    Text(session.fileDisplayName)
                        .font(.callout)
                        .foregroundStyle(.secondary)
                }

                ToolbarItemGroup(placement: .primaryAction) {
                    Picker("", selection: $language) {
                        ForEach(AppLanguage.allCases) { lang in
                            Text(lang.label).tag(lang)
                        }
                    }
                    .onChange(of: language) { _, newValue in
                        session.focusLanguage(newValue)
                    }
                    .pickerStyle(.segmented)
                    .frame(width: 160)
                    .help(Text("ui.locale_toggle_hint"))
                }
            }
        }
        .environment(\.locale, session.locale)
    }
}

struct FrameCanvasView: View {
    @ObservedObject var session: DetectionSession
    @State private var dragOrigin: CGPoint?

    var body: some View {
        GeometryReader { proxy in
            let videoRect = calculateVideoRect(containerSize: proxy.size, videoSize: session.videoSize)
            
            ZStack(alignment: .topLeading) {
                // Background for the video area
                Color.black
                
                if let player = session.player {
                    VideoPlayer(player: player)
                        .disabled(true)
                        .aspectRatio(contentMode: .fit)
                        .frame(width: proxy.size.width, height: proxy.size.height)
                        .overlay(alignment: .topTrailing) {
                            TimecodeLabel(text: formattedTime(session.currentTime))
                                .padding(8)
                        }
                } else {
                    PlaceholderView(text: "ui.placeholder_no_video")
                }

                // Selection Overlay
                if let selection = session.selection {
                    // Denormalize relative to video rect, then offset to container coordinates
                    let rectInVideo = selection.denormalized(in: videoRect.size)
                    let rectInContainer = rectInVideo.offsetBy(dx: videoRect.minX, dy: videoRect.minY)
                    
                    SelectionOverlay(
                        rect: rectInContainer,
                        color: session.subtitleColor,
                        isHighlightActive: session.isHighlightActive,
                        threshold: session.threshold,
                        tolerance: session.tolerance
                    )
                }
            }
            .contentShape(Rectangle())
            .gesture(
                DragGesture(minimumDistance: 2)
                    .onChanged { value in
                        guard session.player != nil else { return }
                        
                        // Convert container coordinates to video coordinates
                        let startInVideo = value.startLocation.offsetBy(dx: -videoRect.minX, dy: -videoRect.minY)
                        let locationInVideo = value.location.offsetBy(dx: -videoRect.minX, dy: -videoRect.minY)
                        
                        if dragOrigin == nil {
                            dragOrigin = clamp(startInVideo, in: videoRect.size)
                            session.isSelectingRegion = true
                        }
                        
                        let origin = dragOrigin ?? clamp(startInVideo, in: videoRect.size)
                        let current = clamp(locationInVideo, in: videoRect.size)
                        let rect = makeRect(origin: origin, current: current)
                        
                        session.updateSelection(normalized: rect.normalized(in: videoRect.size))
                    }
                    .onEnded { _ in
                        dragOrigin = nil
                        session.isSelectingRegion = false
                    }
            )
            .background(Color(nsColor: .textBackgroundColor))
            .clipShape(RoundedRectangle(cornerRadius: 10))
            .overlay(
                RoundedRectangle(cornerRadius: 10)
                    .strokeBorder(Color.gray.opacity(0.2), lineWidth: 1)
            )
        }
        .frame(maxWidth: .infinity, minHeight: 320, maxHeight: 420)
    }

    private func calculateVideoRect(containerSize: CGSize, videoSize: CGSize?) -> CGRect {
        guard let videoSize = videoSize, videoSize.width > 0, videoSize.height > 0 else {
            return CGRect(origin: .zero, size: containerSize)
        }
        
        let widthRatio = containerSize.width / videoSize.width
        let heightRatio = containerSize.height / videoSize.height
        let scale = min(widthRatio, heightRatio)
        
        let newWidth = videoSize.width * scale
        let newHeight = videoSize.height * scale
        
        let x = (containerSize.width - newWidth) / 2
        let y = (containerSize.height - newHeight) / 2
        
        return CGRect(x: x, y: y, width: newWidth, height: newHeight)
    }

    private func clamp(_ point: CGPoint, in size: CGSize) -> CGPoint {
        CGPoint(
            x: min(max(point.x, 0), size.width),
            y: min(max(point.y, 0), size.height)
        )
    }

    private func makeRect(origin: CGPoint, current: CGPoint) -> CGRect {
        CGRect(
            x: min(origin.x, current.x),
            y: min(origin.y, current.y),
            width: abs(current.x - origin.x),
            height: abs(current.y - origin.y)
        )
    }
}

struct SelectionOverlay: View {
    let rect: CGRect
    let color: Color
    let isHighlightActive: Bool
    let threshold: Double
    let tolerance: Double

    var body: some View {
        ZStack(alignment: .topLeading) {
            // Ensure the overlay takes up the full space
            Color.clear
            
            Path(rect)
                .fill(color.opacity(0.2))

            Path(rect)
                .stroke(color, style: StrokeStyle(lineWidth: 2, dash: [6, 4]))

            if isHighlightActive {
                Path(rect.insetBy(dx: 2, dy: 2))
                    .fill(Color.cyan.opacity(0.2))
                
                VStack(alignment: .leading, spacing: 2) {
                    Text(String(format: NSLocalizedString("ui.highlight_values", comment: ""), Int(threshold), Int(tolerance)))
                        .font(.footnote)
                        .padding(6)
                        .background(Color.black.opacity(0.6).blur(radius: 0.5))
                        .foregroundStyle(.white)
                        .clipShape(RoundedRectangle(cornerRadius: 6))
                        .padding(6)
                }
                .fixedSize()
                .offset(x: rect.minX, y: rect.minY)
            }
        }
        // Allow touches to pass through the clear background, but capture touches on the selection?
        // Actually FrameCanvasView handles gestures on the container.
        // So we should allow hit testing to pass through if we want to drag a new selection over an old one?
        // But the drag gesture is on the parent. So allowsHitTesting(false) might be good for the overlay
        // to prevent it from blocking the drag gesture if it were interactive (but here gesture is on parent).
        .allowsHitTesting(false)
    }
}

struct ControlStackView: View {
    @ObservedObject var session: DetectionSession

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            TimelineView(session: session)
            SelectionSummaryView(session: session)
            ColorAndBrightnessControls(session: session)
            ActionButtons(session: session)
            ProgressPanel(session: session)
        }
    }
}

struct TimelineView: View {
    @ObservedObject var session: DetectionSession

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 12) {
                Button {
                    session.togglePlayPause()
                } label: {
                    Image(systemName: session.isPlaying ? "pause.fill" : "play.fill")
                        .font(.title3)
                }
                .buttonStyle(.plain)
                .disabled(session.player == nil)

                Slider(
                    value: Binding(
                        get: { session.currentTime },
                        set: { newValue in
                            session.seek(to: newValue)
                        }
                    ),
                    in: 0...(max(session.duration, 1))
                )
            }
            HStack {
                Text(formattedTime(session.currentTime))
                    .font(.footnote.monospacedDigit())
                Spacer()
                Text(formattedTime(session.duration))
                    .font(.footnote.monospacedDigit())
                    .foregroundStyle(.secondary)
            }
        }
    }
}

struct SelectionSummaryView: View {
    @ObservedObject var session: DetectionSession

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("ui.selection_summary")
                .font(.headline)
            HStack {
                VStack(alignment: .leading, spacing: 4) {
                    Text("ui.region")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    if let selection = session.selection {
                        let x = selection.origin.x
                        let y = selection.origin.y
                        let w = selection.size.width
                        let h = selection.size.height
                        Text(String(format: "x: %.2f  y: %.2f  w: %.2f  h: %.2f", x, y, w, h))
                            .font(.footnote.monospacedDigit())
                    } else {
                        Text("ui.region_none")
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                    }
                }
                Spacer()
                VStack(alignment: .leading, spacing: 4) {
                    Text("ui.subtitle_color")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    HStack(spacing: 6) {
                        RoundedRectangle(cornerRadius: 4)
                            .fill(session.subtitleColor)
                            .frame(width: 36, height: 18)
                            .overlay(
                                RoundedRectangle(cornerRadius: 4)
                                    .stroke(Color.gray.opacity(0.4), lineWidth: 1)
                            )
                        Text(session.subtitleColor.description)
                            .font(.footnote)
                            .lineLimit(1)
                    }
                }
            }
        }
        .padding(10)
        .background(Color(nsColor: .controlBackgroundColor))
        .clipShape(RoundedRectangle(cornerRadius: 8))
    }
}

struct ColorAndBrightnessControls: View {
    @ObservedObject var session: DetectionSession

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 4) {
                    Text("ui.subtitle_color")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    ColorPicker("ui.subtitle_color", selection: $session.subtitleColor)
                        .labelsHidden()
                }
                VStack(alignment: .leading, spacing: 4) {
                    Text("ui.threshold")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    HStack {
                        Slider(value: $session.threshold, in: 0...255, step: 1)
                        Text("\(Int(session.threshold))")
                            .font(.footnote.monospacedDigit())
                            .frame(width: 44, alignment: .trailing)
                    }
                }
                VStack(alignment: .leading, spacing: 4) {
                    Text("ui.tolerance")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    HStack {
                        Slider(value: $session.tolerance, in: 0...64, step: 1)
                        Text("\(Int(session.tolerance))")
                            .font(.footnote.monospacedDigit())
                            .frame(width: 44, alignment: .trailing)
                    }
                }
            }
        }
    }
}

struct ActionButtons: View {
    @ObservedObject var session: DetectionSession

    var body: some View {
        HStack(spacing: 10) {
            Button {
                session.isSelectingColor.toggle()
            } label: {
                Label("ui.start_color", systemImage: "eyedropper.halffull")
            }

            Button {
                session.isSelectingRegion.toggle()
            } label: {
                Label("ui.start_region", systemImage: "selection.pin.in.out")
            }

            Button {
                session.toggleHighlight()
            } label: {
                Label("ui.highlight", systemImage: session.isHighlightActive ? "highlighter" : "highlighter")
            }

            Spacer()

            Button {
                session.startDetectionDemo()
            } label: {
                Label("ui.start_detection", systemImage: "play.circle.fill")
            }
            .buttonStyle(.borderedProminent)
            .disabled(session.isDetecting)
        }
    }
}

struct ProgressPanel: View {
    @ObservedObject var session: DetectionSession

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack {
                Text("ui.detection_progress")
                Spacer()
                if session.isDetecting {
                    ProgressView()
                        .progressViewStyle(.circular)
                        .scaleEffect(0.75)
                }
            }
            ProgressView(value: session.progress, total: 1.0)
                .progressViewStyle(.linear)
            MetricsView(metrics: session.metrics)
            if let message = session.errorMessage {
                Text(message)
                    .font(.footnote)
                    .foregroundStyle(.red)
            }
        }
        .padding(10)
        .background(Color(nsColor: .controlBackgroundColor))
        .clipShape(RoundedRectangle(cornerRadius: 8))
    }
}

struct MetricsView: View {
    let metrics: ProgressMetrics

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 12) {
                metric("FPS", metrics.fps)
                metric("det", metrics.det, suffix: " ms")
                metric("seg", metrics.seg, suffix: " ms")
                metric("pf", metrics.pf, suffix: " ms")
                metric("ocr", metrics.ocr, suffix: " ms")
                metric("wr", metrics.wr, suffix: " ms")
            }
            Text(String(format: NSLocalizedString("ui.metric_counts", comment: ""), metrics.cues, metrics.ocrEmpty))
                .font(.footnote)
                .foregroundStyle(.secondary)
        }
    }

    private func metric(_ label: String, _ value: Double?, suffix: String = "") -> some View {
        HStack(spacing: 4) {
            Text(label)
                .font(.caption2)
                .foregroundStyle(.secondary)
            Text(valueText(value, suffix: suffix))
                .font(.footnote.monospacedDigit())
        }
    }

    private func valueText(_ value: Double?, suffix: String) -> String {
        guard let value else { return "--" }
        let formatted = String(format: "%.1f", value)
        return suffix.isEmpty ? formatted : "\(formatted)\(suffix)"
    }
}

struct SubtitlesListView: View {
    @ObservedObject var session: DetectionSession

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Text("ui.subtitles")
                    .font(.headline)
                Spacer()
                if session.isDetecting {
                    Text("ui.detecting")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
            if session.subtitles.isEmpty {
                VStack(alignment: .leading, spacing: 6) {
                    Text("ui.no_subtitles")
                        .foregroundStyle(.secondary)
                    Text("ui.no_subtitles_hint")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding()
                .background(Color(nsColor: .controlBackgroundColor))
                .clipShape(RoundedRectangle(cornerRadius: 8))
            } else {
                List(session.subtitles) { item in
                    Button {
                        session.seek(to: item.timecode)
                    } label: {
                        VStack(alignment: .leading, spacing: 4) {
                            HStack {
                                Text(formattedTime(item.timecode))
                                    .font(.footnote.monospacedDigit())
                                    .foregroundStyle(.secondary)
                                if let confidence = item.confidence {
                                    Text(String(format: "· %.0f%%", confidence * 100))
                                        .font(.footnote)
                                        .foregroundStyle(.secondary)
                                }
                                Spacer()
                            }
                            Text(item.text)
                                .font(.body)
                                .lineLimit(3)
                        }
                    }
                    .buttonStyle(.plain)
                }
                .listStyle(.plain)
            }
        }
    }
}

struct PlaceholderView: View {
    let text: LocalizedStringKey

    var body: some View {
        ZStack {
            Color(nsColor: .textBackgroundColor)
            VStack(spacing: 8) {
                Image(systemName: "film")
                    .font(.largeTitle)
                    .foregroundStyle(.secondary)
                Text(text)
                    .foregroundStyle(.secondary)
            }
        }
    }
}

struct TimecodeLabel: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.caption.monospacedDigit())
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(.ultraThinMaterial, in: Capsule())
    }
}

func formattedTime(_ time: TimeInterval) -> String {
    guard time.isFinite else { return "--:--" }
    let totalSeconds = Int(time.rounded())
    let seconds = totalSeconds % 60
    let minutes = (totalSeconds / 60) % 60
    let hours = totalSeconds / 3600
    if hours > 0 {
        return String(format: "%02d:%02d:%02d", hours, minutes, seconds)
    }
    return String(format: "%02d:%02d", minutes, seconds)
}
