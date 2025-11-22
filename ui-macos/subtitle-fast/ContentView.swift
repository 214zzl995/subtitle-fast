import AVFoundation
import AVKit
import CoreImage
import SwiftUI
import UniformTypeIdentifiers

struct ContentView: View {
    @StateObject private var session = DetectionSession()
    @State private var showingFilePicker = false

    var body: some View {
        NavigationSplitView {
            SidebarView(session: session, showingFilePicker: $showingFilePicker)
                .navigationSplitViewColumnWidth(min: 240, ideal: 280, max: 340)
        } detail: {
            HSplitView {
                leftColumn
                    .frame(maxHeight: .infinity, alignment: .top)
                rightColumn
                    .frame(maxHeight: .infinity, alignment: .top)
            }
            .padding(.horizontal, 12)
            .padding(.vertical, 10)
            .background(
                LinearGradient(
                    colors: [
                        Color(nsColor: .windowBackgroundColor).opacity(0.9),
                        Color(nsColor: .textBackgroundColor).opacity(0.85)
                    ],
                    startPoint: .topLeading,
                    endPoint: .bottomTrailing
                )
            )
        }
        .frame(minWidth: 1050, minHeight: 720)
        .toolbar {
            ToolbarItem(placement: .navigation) {
                Button {
                    showingFilePicker = true
                } label: {
                    Label("ui.open_file", systemImage: "folder.badge.plus")
                }
            }
        }
        .fileImporter(
            isPresented: $showingFilePicker,
            allowedContentTypes: [.movie, .mpeg4Movie, .quickTimeMovie],
            allowsMultipleSelection: true
        ) { result in
            guard case .success(let urls) = result else { return }
            session.load(from: urls)
        }
    }

    @ViewBuilder
    private var leftColumn: some View {
        VStack(spacing: 10) {
            PreviewPanel(session: session)
            ControlPanel(session: session)
        }
        .frame(minWidth: 540, maxHeight: .infinity, alignment: .top)
    }

    @ViewBuilder
    private var rightColumn: some View {
        VStack(spacing: 10) {
            StatusPanel(session: session)
            SubtitleListPanel(session: session)
        }
        .frame(minWidth: 320, maxWidth: 420, maxHeight: .infinity, alignment: .top)
    }
}

// MARK: - Sidebar

private struct SidebarView: View {
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

private struct SidebarFiles: View {
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
            if case .detecting = file.status { return true }
            return false
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
        }
    }
}

// MARK: - Primary layout

private struct PreviewPanel: View {
    @ObservedObject var session: DetectionSession

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Label("ui.preview", systemImage: "sparkles.tv.fill")
                    .font(.headline)
                Spacer()
                Picker("", selection: $session.previewMode) {
                    Text("ui.preview_mode_color").tag(PreviewMode.color)
                    Text("ui.preview_mode_luma").tag(PreviewMode.luma)
                }
                .pickerStyle(.segmented)
                .frame(width: 220)
            }

            VideoPreviewView(session: session)
                .frame(minHeight: 340)
                .onChange(of: session.previewMode) { _, _ in
                    session.applyPreviewMode()
                }
        }
        .padding(.vertical, 8)
        .padding(.horizontal, 8)
    }
}

private struct ControlPanel: View {
    @ObservedObject var session: DetectionSession

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
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
                    .font(.title3.weight(.semibold))
                    .padding(.horizontal, 18)
                    .padding(.vertical, 10)
                }
                .buttonStyle(.borderedProminent)
                .tint(session.isDetecting ? .red : .accentColor)
                .controlSize(.large)
                .disabled(session.selection == nil || session.selectedFile == nil)
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
        VStack(alignment: .leading, spacing: 10) {
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
        .padding(.vertical, 10)
        .padding(.horizontal, 10)
    }

    private var statusTitle: String {
        if session.isDetecting {
            return NSLocalizedString("ui.detecting", comment: "detecting")
        }
        guard let activeID = session.activeFileID,
              let status = session.files.first(where: { $0.id == activeID })?.status else {
            return NSLocalizedString("ui.status_idle", comment: "idle")
        }
        switch status {
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
        }
    }
}

private struct SubtitleListPanel: View {
    @ObservedObject var session: DetectionSession

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
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
                .frame(minHeight: 240, maxHeight: .infinity)
        }
        .padding(.vertical, 10)
        .padding(.horizontal, 10)
        .frame(maxHeight: .infinity, alignment: .top)
    }
}

private struct MetricsGrid: View {
    let metrics: DetectionMetrics
    let subtitles: Int
    
    var body: some View {
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
        .font(.caption.monospacedDigit())
    }
    
    private func label(_ key: String, systemImage: String) -> some View {
        Label(LocalizedStringKey(key), systemImage: systemImage)
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
                    PlayerContainerView(player: player)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .allowsHitTesting(false)

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

struct PlayerContainerView: NSViewRepresentable {
    let player: AVPlayer
    
    func makeNSView(context: Context) -> AVPlayerView {
        let view = AVPlayerView()
        view.controlsStyle = .none
        view.showsFrameSteppingButtons = false
        view.showsSharingServiceButton = false
        view.updatesNowPlayingInfoCenter = false
        view.allowsPictureInPicturePlayback = false
        view.videoGravity = .resizeAspect
        return view
    }
    
    func updateNSView(_ nsView: AVPlayerView, context: Context) {
        nsView.player = player
    }
}

struct PlaybackControls: View {
    @ObservedObject var session: DetectionSession
    
    var body: some View {
        HStack(alignment: .center, spacing: 14) {
            Button {
                session.togglePlayPause()
            } label: {
                Image(systemName: session.isPlaying ? "pause.fill" : "play.fill")
                    .font(.title3.weight(.semibold))
                    .frame(width: 34, height: 34)
            }
            .buttonStyle(.plain)
            .background(.ultraThinMaterial, in: Circle())
            .overlay(Circle().stroke(Color.primary.opacity(0.06)))
            
            VStack(alignment: .leading, spacing: 6) {
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
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(.thinMaterial, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
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

// MARK: - Glass helper

private struct GlassBackground: ViewModifier {
    func body(content: Content) -> some View {
        content
            .background(
                RoundedRectangle(cornerRadius: 14, style: .continuous)
                    .fill(.ultraThinMaterial)
            )
            .overlay(
                RoundedRectangle(cornerRadius: 14, style: .continuous)
                    .strokeBorder(Color.white.opacity(0.08))
            )
            .shadow(color: .black.opacity(0.08), radius: 10, x: 0, y: 8)
    }
}

private extension View {
    func glassSurface() -> some View {
        modifier(GlassBackground())
    }
}
