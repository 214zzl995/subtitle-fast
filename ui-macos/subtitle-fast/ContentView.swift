import AVFoundation
import AVKit
import SwiftUI
import UniformTypeIdentifiers

struct ContentView: View {
    @StateObject private var session = DetectionSession()
    @State private var showingFilePicker = false
    @AppStorage("appLanguage") private var appLanguage: AppLanguage = .systemDefault()

    var body: some View {
        NavigationStack {
            HSplitView {
                VStack(spacing: 12) {
                    PreviewPanel(session: session)
                    ControlPanel(session: session)
                }
                .frame(minWidth: 640)

                VStack(spacing: 12) {
                    StatusPanel(session: session)
                    SubtitleListPanel(session: session)
                }
                .frame(minWidth: 360)
            }
            .padding(16)
            .background(Color(nsColor: .windowBackgroundColor))
            .toolbar {
                ToolbarItem(placement: .navigation) {
                    Button {
                        showingFilePicker = true
                    } label: {
                        Label("ui.open_file", systemImage: "folder.badge.plus")
                    }
                }

                ToolbarItem(placement: .principal) {
                    VStack(spacing: 2) {
                        Text(session.selectedFile?.lastPathComponent ?? NSLocalizedString("ui.no_file", comment: "no file"))
                            .font(.headline)
                        if session.isDetecting {
                            Text(NSLocalizedString("ui.detecting", comment: "detecting"))
                                .font(.caption)
                                .foregroundStyle(.secondary)
                        }
                    }
                }

                ToolbarItem {
                    Menu {
                        Picker("ui.language", selection: $appLanguage) {
                            ForEach(AppLanguage.allCases) { lang in
                                Text(lang.label).tag(lang)
                            }
                        }
                    } label: {
                        Label(appLanguage.label, systemImage: "globe")
                    }
                    .help("ui.locale_toggle_hint")
                }
            }
            .environment(\.locale, appLanguage.locale)
            .fileImporter(
                isPresented: $showingFilePicker,
                allowedContentTypes: [.movie, .mpeg4Movie, .quickTimeMovie],
                allowsMultipleSelection: false
            ) { result in
                guard case .success(let urls) = result, let url = urls.first else { return }
                session.load(from: url)
            }
        }
    }
}

// MARK: - Primary layout

private struct PreviewPanel: View {
    @ObservedObject var session: DetectionSession

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Label("ui.preview", systemImage: "sparkles.tv.fill")
                .font(.headline)

            VideoPreviewView(session: session)
                .frame(minHeight: 340)
        }
        .padding(14)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
    }
}

private struct ControlPanel: View {
    @ObservedObject var session: DetectionSession

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            Label("ui.selection_summary", systemImage: "cursorarrow.motionlines")
                .font(.headline)

            SelectionSummary(session: session)

            Divider()

            HStack(spacing: 8) {
                Button {
                    session.startRegionSelection()
                } label: {
                    Label(
                        session.isSelectingRegion ? "ui.finish_region" : "ui.select_region",
                        systemImage: session.isSelectingRegion ? "viewfinder.circle.fill" : "viewfinder.circle"
                    )
                }
                .buttonStyle(.bordered)
                .tint(session.isSelectingRegion ? .green : .primary)

                Toggle(isOn: $session.isHighlightActive) {
                    Label("ui.highlight", systemImage: "light.max")
                }
                .toggleStyle(.button)

                if session.selection != nil {
                    Button {
                        session.resetSelection()
                    } label: {
                        Label("ui.reset_selection", systemImage: "arrow.counterclockwise")
                    }
                    .buttonStyle(.bordered)
                }

                Spacer()
            }

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
                    if session.isDetecting {
                        session.cancelDetection()
                    } else {
                        session.startDetection()
                    }
                } label: {
                    Label(
                        session.isDetecting ? "ui.cancel" : "ui.start_detection",
                        systemImage: session.isDetecting ? "stop.fill" : "play.fill"
                    )
                    .frame(maxWidth: 200)
                }
                .buttonStyle(.borderedProminent)
                .tint(session.isDetecting ? .red : .accentColor)
                .controlSize(.large)
                .disabled(session.selection == nil || session.selectedFile == nil)
            }
        }
        .padding(14)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
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

private struct SelectionSummary: View {
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

private struct StatusPanel: View {
    @ObservedObject var session: DetectionSession

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Label("ui.detection_progress", systemImage: "speedometer")
                    .font(.headline)
                Spacer()
                if session.isDetecting {
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
        .padding(14)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
    }

    private var statusTitle: String {
        if session.isDetecting {
            return NSLocalizedString("ui.detecting", comment: "detecting")
        }
        if session.selectedFile == nil {
            return NSLocalizedString("ui.status_idle", comment: "idle")
        }
        if session.selection == nil {
            return NSLocalizedString("ui.select_region", comment: "select region")
        }
        return NSLocalizedString("ui.status_ready", comment: "ready")
    }
}

private struct SubtitleListPanel: View {
    @ObservedObject var session: DetectionSession

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Label("ui.subtitles", systemImage: "text.bubble")
                    .font(.headline)
                Spacer()
                if !session.subtitles.isEmpty {
                    Button {
                        session.exportSubtitles()
                    } label: {
                        Label("ui.export", systemImage: "square.and.arrow.up")
                    }
                    .controlSize(.small)
                }
            }

            SubtitleListView(session: session)
                .frame(minHeight: 260)
        }
        .padding(14)
        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 14, style: .continuous))
    }
}

private struct MetricsGrid: View {
    let metrics: DetectionMetrics
    let subtitles: Int
    
    var body: some View {
        Grid(alignment: .leading, horizontalSpacing: 16, verticalSpacing: 6) {
            GridRow {
                label("ui.metrics_fps", systemImage: "speedometer")
                Text(String(format: "%.1f", metrics.fps)).bold()
                
                label("ui.metrics_detection", systemImage: "waveform.path.ecg")
                Text(String(format: "%.1f ms", metrics.det)).bold()
            }
            GridRow {
                label("ui.metrics_ocr", systemImage: "text.viewfinder")
                Text(String(format: "%.1f ms", metrics.ocr)).bold()
                
                label("ui.metrics_cues", systemImage: "text.bubble")
                Text("\(subtitles)").bold()
            }
            GridRow {
                label("ui.metrics_empty", systemImage: "eye.slash")
                Text("\(metrics.ocrEmpty)").bold()
            }
        }
        .font(.caption.monospacedDigit())
    }
    
    private func label(_ key: String, systemImage: String) -> some View {
        Label(key, systemImage: systemImage)
            .foregroundStyle(.secondary)
    }
}

// MARK: - Preview + Playback

private struct VideoPreviewView: View {
    @ObservedObject var session: DetectionSession

    var body: some View {
        VStack(spacing: 0) {
            VideoCanvas(session: session)
                .frame(minHeight: 280)

            PlaybackControls(session: session)
                .padding(.horizontal, 12)
                .padding(.vertical, 10)
                .background(.ultraThinMaterial)
        }
        .background(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .strokeBorder(Color.primary.opacity(0.06))
        )
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
    }
}

private struct VideoCanvas: View {
    @ObservedObject var session: DetectionSession
    @State private var dragOrigin: CGPoint?

    var body: some View {
        GeometryReader { proxy in
            ZStack {
                Color.black

                if let player = session.player {
                    VideoPlayerLayer(player: player)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)

                    if let videoRect = videoRect(in: proxy.size), let selection = session.selection {
                        let rectInVideo = selection.denormalized(in: videoRect.size)
                        let rectInCanvas = rectInVideo.offsetBy(dx: videoRect.minX, dy: videoRect.minY)
                        SelectionOverlay(
                            rect: rectInCanvas,
                            color: .accentColor,
                            isHighlightActive: session.isHighlightActive
                        )
                    }
                } else {
                    ContentUnavailableView("ui.placeholder_no_video", systemImage: "film", description: Text("ui.no_file"))
                }
            }
            .contentShape(Rectangle())
            .gesture(
                DragGesture(minimumDistance: 2)
                    .onChanged { value in
                        handleDragChanged(value: value, in: proxy.size)
                    }
                    .onEnded { _ in
                        handleDragEnded()
                    }
            )
        }
    }

    private func handleDragChanged(value: DragGesture.Value, in containerSize: CGSize) {
        guard session.isSelectingRegion, let videoRect = videoRect(in: containerSize) else { return }
        if dragOrigin == nil {
            guard videoRect.contains(value.startLocation) else { return }
            dragOrigin = clamp(value.startLocation.offsetBy(dx: -videoRect.minX, dy: -videoRect.minY), in: videoRect.size)
        }

        let origin = dragOrigin ?? clamp(value.startLocation.offsetBy(dx: -videoRect.minX, dy: -videoRect.minY), in: videoRect.size)
        let current = clamp(value.location.offsetBy(dx: -videoRect.minX, dy: -videoRect.minY), in: videoRect.size)
        let rect = makeRect(origin: origin, current: current)
        session.updateSelection(normalized: rect.normalized(in: videoRect.size))
    }

    private func handleDragEnded() {
        if session.isSelectingRegion {
            session.finishRegionSelection()
        }
        dragOrigin = nil
    }

    private func videoRect(in containerSize: CGSize) -> CGRect? {
        guard let videoSize = session.videoSize else { return nil }
        return AVMakeRect(aspectRatio: videoSize, insideRect: CGRect(origin: .zero, size: containerSize))
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

struct VideoPlayerLayer: NSViewRepresentable {
    let player: AVPlayer
    
    func makeNSView(context: Context) -> NSView {
        let view = NSView()
        view.wantsLayer = true
        let layer = AVPlayerLayer(player: player)
        layer.videoGravity = .resizeAspect
        view.layer = layer
        return view
    }
    
    func updateNSView(_ nsView: NSView, context: Context) {
        if let layer = nsView.layer as? AVPlayerLayer {
            layer.player = player
        }
    }
}

struct PlaybackControls: View {
    @ObservedObject var session: DetectionSession
    
    var body: some View {
        HStack(spacing: 12) {
            Button {
                session.togglePlayPause()
            } label: {
                Image(systemName: session.isPlaying ? "pause.fill" : "play.fill")
                    .font(.title3)
            }
            .buttonStyle(.plain)
            .padding(8)
            .background(.thinMaterial, in: Circle())
            
            VStack(spacing: 2) {
                Slider(
                    value: Binding(
                        get: { session.currentTime },
                        set: { session.seek(to: $0) }
                    ),
                    in: 0...(max(session.duration, 1))
                )
                .controlSize(.small)
                
                HStack {
                    Text(formattedTime(session.currentTime))
                        .font(.caption2.monospacedDigit())
                    Spacer()
                    Text(formattedTime(session.duration))
                        .font(.caption2.monospacedDigit())
                }
                .foregroundStyle(.secondary)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 8)
    }
}

struct SelectionOverlay: View {
    let rect: CGRect
    let color: Color
    let isHighlightActive: Bool

    var body: some View {
        ZStack(alignment: .topLeading) {
            Path(rect)
                .stroke(color, style: StrokeStyle(lineWidth: 2, dash: [6, 4]))
            
            Path(rect)
                .fill(color.opacity(0.1))

            if isHighlightActive {
                Path(rect.insetBy(dx: 2, dy: 2))
                    .fill(Color.cyan.opacity(0.2))
            }
        }
        .allowsHitTesting(false)
    }
}

struct SubtitleListView: View {
    @ObservedObject var session: DetectionSession

    var body: some View {
        List {
            Section(header: Text("ui.subtitles").font(.caption).foregroundStyle(.secondary)) {
                ForEach(session.subtitles) { item in
                    HStack(alignment: .top, spacing: 12) {
                        Text(formattedTime(item.timecode))
                            .font(.caption.monospacedDigit())
                            .foregroundStyle(.secondary)
                            .frame(width: 70, alignment: .leading)
                        
                        VStack(alignment: .leading, spacing: 4) {
                            Text(item.text)
                                .font(.body)
                                .textSelection(.enabled)
                            
                            if let confidence = item.confidence {
                                Text(String(format: "%.0f%%", confidence * 100))
                                    .font(.caption2)
                                    .foregroundStyle(.secondary)
                            }
                        }
                        Spacer(minLength: 0)
                    }
                    .onTapGesture {
                        session.seek(to: item.timecode)
                    }
                }
            }
        }
        .listStyle(.plain)
        .scrollContentBackground(.hidden)
        .background(Color.clear)
        .overlay {
            if session.subtitles.isEmpty {
                ContentUnavailableView("ui.no_subtitles", systemImage: "text.bubble", description: Text("ui.no_subtitles_hint"))
            }
        }
    }
}

// MARK: - Helpers

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

extension CGRect {
    func normalized(in size: CGSize) -> CGRect {
        guard size.width > 0 && size.height > 0 else { return .zero }
        return CGRect(
            x: origin.x / size.width,
            y: origin.y / size.height,
            width: width / size.width,
            height: height / size.height
        )
    }

    func denormalized(in size: CGSize) -> CGRect {
        CGRect(
            x: origin.x * size.width,
            y: origin.y * size.height,
            width: width * size.width,
            height: height * size.height
        )
    }
}

extension CGPoint {
    func offsetBy(dx: CGFloat, dy: CGFloat) -> CGPoint {
        CGPoint(x: x + dx, y: y + dy)
    }
}
