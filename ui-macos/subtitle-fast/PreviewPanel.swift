import SwiftUI
import AVFoundation
import AVKit
import AppKit

private let highlightPalette: [NSColor] = [
    .controlAccentColor,
    NSColor.systemRed,
    NSColor.systemOrange,
    NSColor.systemYellow,
    NSColor.systemGreen,
    NSColor.systemTeal,
    NSColor.systemBlue,
    NSColor.systemPurple
]

struct PreviewPanel: View {
    @ObservedObject var session: DetectionSession

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Label {
                    Text("ui.preview")
                        .font(.headline)
                } icon: {
                    Image(systemName: "sparkles.tv.fill")
                        .font(.headline.weight(.semibold))
                }
                Spacer()
                HStack(spacing: 6) {
                    HeaderIconButton(
                        systemName: session.selectionVisible ? "eye.fill" : "eye.slash",
                        active: session.selectionVisible,
                        disabled: session.selectedFile == nil
                    ) {
                        session.selectionVisible.toggle()
                    }
                    .help(LocalizedStringKey("ui.toggle_selection"))

                    HeaderIconButton(
                        systemName: "arrow.counterclockwise",
                        active: false,
                        disabled: session.selectedFile == nil,
                        action: { session.resetSelection() }
                    )
                    .help(LocalizedStringKey("ui.reset_selection"))

                    Divider().frame(height: 22)

                    HeaderIconButton(
                        systemName: "light.max",
                        active: session.isHighlightActive,
                        disabled: session.selection == nil || session.selectedFile == nil,
                        action: { session.isHighlightActive.toggle() }
                    )
                    .help(LocalizedStringKey("ui.highlight"))

                    HeaderIconButton(
                        systemName: "scope",
                        active: session.isSamplingThreshold,
                        disabled: session.selectedFile == nil,
                        action: {
                            if session.isSamplingThreshold {
                                session.cancelThresholdSampling()
                            } else {
                                session.beginThresholdSampling()
                            }
                        }
                    )
                    .help(LocalizedStringKey("ui.sample_threshold"))

                    ColorMenuButton(
                        current: session.highlightTint,
                        disabled: session.selection == nil || session.selectedFile == nil || !session.isHighlightActive
                    ) { swatch in
                        session.highlightTint = swatch
                    }

                    Divider().frame(height: 22)

                    PreviewModeToggle(selection: $session.previewMode)
                }
            }

            VideoPreviewView(session: session)
                .frame(minHeight: 340)
                .onChange(of: session.previewMode) { _, _ in
                    session.applyPreviewMode()
                }
        }
        .padding(.vertical, 8)
        .padding(.horizontal, 10)
    }
}

struct VideoPreviewView: View {
    @ObservedObject var session: DetectionSession

    var body: some View {
        VStack(spacing: 10) {
            VideoCanvas(session: session)
                .frame(minHeight: 280)
                .frame(maxWidth: .infinity)
                .background(
                    RoundedRectangle(cornerRadius: 12, style: .continuous)
                        .fill(Color(nsColor: .underPageBackgroundColor))
                )
                .overlay(
                    RoundedRectangle(cornerRadius: 12, style: .continuous)
                        .stroke(Color.primary.opacity(0.06))
                )
                .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))

            PlaybackControls(session: session)
                .frame(maxWidth: .infinity)
                .padding(.top, 4)
                .padding(.bottom, 6)
        }
    }
}

private struct HeaderIconButton: View {
    let systemName: String
    var active: Bool = false
    var disabled: Bool = false
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            Image(systemName: systemName)
                .font(.system(size: 13, weight: .semibold))
                .frame(width: 28, height: 28)
                .foregroundStyle(active ? Color.accentColor : Color.primary)
                .overlay(
                    RoundedRectangle(cornerRadius: 8, style: .continuous)
                        .stroke(active ? Color.accentColor.opacity(0.7) : Color.primary.opacity(0.2), lineWidth: 1.2)
                )
        }
        .buttonStyle(.plain)
        .disabled(disabled)
        .opacity(disabled ? 0.35 : 1)
    }
}

private struct ColorMenuButton: View {
    let current: NSColor
    let disabled: Bool
    let onSelect: (NSColor) -> Void
    @State private var isOpen = false

    var body: some View {
        let resolvedCurrent = normalizedColor(current)
        Button {
            guard !disabled else { return }
            isOpen = false
            // Open on next run loop to avoid stale state keeping the popover closed.
            DispatchQueue.main.async {
                isOpen = true
            }
        } label: {
            HStack(spacing: 6) {
                Circle()
                    .fill(swiftUIColor(from: resolvedCurrent))
                    .frame(width: 18, height: 18)
                    .overlay(
                        Circle()
                            .stroke(Color.primary.opacity(0.18), lineWidth: 1)
                    )
                Image(systemName: "chevron.down")
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(Color.primary.opacity(0.7))
            }
            .padding(.horizontal, 8)
            .frame(height: 28)
            .overlay(
                RoundedRectangle(cornerRadius: 8, style: .continuous)
                    .stroke(Color.primary.opacity(0.12), lineWidth: 1)
            )
        }
        .buttonStyle(.plain)
        .disabled(disabled)
        .opacity(disabled ? 0.35 : 1)
        .onChange(of: disabled) { value in
            if value {
                isOpen = false
            }
        }
        .popover(isPresented: $isOpen, arrowEdge: .bottom) {
            VStack(spacing: 0) {
                ForEach(Array(highlightPalette.enumerated()), id: \.offset) { index, swatch in
                    let resolvedSwatch = normalizedColor(swatch)
                    Button {
                        onSelect(resolvedSwatch)
                        isOpen = false
                    } label: {
                        HStack(spacing: 8) {
                            Circle()
                                .fill(swiftUIColor(from: resolvedSwatch))
                                .frame(width: 16, height: 16)
                                .overlay(Circle().stroke(Color.primary.opacity(0.2), lineWidth: 1))
                            Text(colorName(for: swatch))
                            Spacer()
                            if isSameColor(resolvedCurrent, resolvedSwatch) {
                                Image(systemName: "checkmark")
                                    .font(.system(size: 10, weight: .semibold))
                            }
                        }
                        .padding(.vertical, 6)
                        .padding(.horizontal, 10)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)
                    if index != highlightPalette.indices.last {
                        Divider()
                    }
                }
            }
            .padding(6)
            .frame(minWidth: 180)
        }
    }

    private func normalizedColor(_ color: NSColor) -> NSColor {
        if let sRGB = color.usingColorSpace(.sRGB) { return sRGB }
        if let device = color.usingColorSpace(.deviceRGB) { return device }
        if let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) {
            let cgColor = color.cgColor
            if let converted = cgColor.converted(to: colorSpace, intent: .defaultIntent, options: nil),
               let nsColor = NSColor(cgColor: converted) {
                return nsColor
            }
        }
        return color
    }

    private func swiftUIColor(from color: NSColor) -> Color {
        let normalized = normalizedColor(color)
        return Color(
            .sRGB,
            red: normalized.redComponent,
            green: normalized.greenComponent,
            blue: normalized.blueComponent,
            opacity: normalized.alphaComponent
        )
    }

    private func isSameColor(_ lhs: NSColor, _ rhs: NSColor) -> Bool {
        let epsilon: CGFloat = 0.001
        return abs(lhs.redComponent - rhs.redComponent) < epsilon
            && abs(lhs.greenComponent - rhs.greenComponent) < epsilon
            && abs(lhs.blueComponent - rhs.blueComponent) < epsilon
            && abs(lhs.alphaComponent - rhs.alphaComponent) < epsilon
    }

    private func colorName(for swatch: NSColor) -> String {
        switch swatch {
        case NSColor.systemRed: return "Red"
        case NSColor.systemOrange: return "Orange"
        case NSColor.systemYellow: return "Yellow"
        case NSColor.systemGreen: return "Green"
        case NSColor.systemTeal: return "Teal"
        case NSColor.systemBlue: return "Blue"
        case NSColor.systemPurple: return "Purple"
        default: return "Accent"
        }
    }
}

private struct PreviewModeToggle: View {
    @Binding var selection: PreviewMode

    var body: some View {
        HStack(spacing: 1) {
            ForEach(PreviewMode.allCases) { mode in
                let isSelected = selection == mode
                Button {
                    selection = mode
                } label: {
                    Text(label(for: mode))
                        .font(.caption.weight(.semibold))
                        .frame(width: 74, height: 24)
                        .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .foregroundStyle(isSelected ? Color.primary : Color.secondary)
                .background(
                    RoundedRectangle(cornerRadius: 7, style: .continuous)
                        .fill(isSelected ? Color.accentColor.opacity(0.18) : Color.clear)
                )
            }
        }
        .frame(height: 28)
        .padding(.horizontal, 4)
        .background(
            RoundedRectangle(cornerRadius: 9, style: .continuous)
                .fill(Color.primary.opacity(0.05))
        )
        .overlay(
            RoundedRectangle(cornerRadius: 9, style: .continuous)
                .stroke(Color.primary.opacity(0.12), lineWidth: 1)
        )
    }

    private func label(for mode: PreviewMode) -> LocalizedStringKey {
        switch mode {
        case .color:
            return "ui.preview_mode_color"
        case .luma:
            return "ui.preview_mode_luma"
        }
    }
}

struct VideoCanvas: View {
    @ObservedObject var session: DetectionSession
    @State private var dragOrigin: CGPoint?
    @State private var activeHandle: SelectionHandle?
    @State private var handleStartRect: CGRect?
    @State private var highlightImage: CGImage?
    @State private var isComputingHighlight = false
    @State private var samplingFrame: CGImage?
    @State private var samplingHoverLocation: CGPoint?
    @State private var samplingBrightness: Double?
    @State private var magnifierImage: CGImage?

    var body: some View {
        GeometryReader { proxy in
            let currentVideoRect = videoRect(in: proxy.size)
            ZStack {
                Color(nsColor: .underPageBackgroundColor)

                if let player = session.player {
                    PlayerContainerView(player: player)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                        .allowsHitTesting(false)

                    if let videoRect = videoRect(in: proxy.size), let selection = session.selection {
                        let rectInVideo = selection.denormalized(in: videoRect.size)
                        let rectInCanvas = rectInVideo.offsetBy(dx: videoRect.minX, dy: videoRect.minY)

                        if let image = highlightImage, session.isHighlightActive {
                            Image(decorative: image, scale: 1.0)
                                .resizable()
                                .interpolation(.none)
                                .frame(width: rectInCanvas.width, height: rectInCanvas.height)
                                .position(x: rectInCanvas.midX, y: rectInCanvas.midY)
                        }

                        if session.selectionVisible {
                            SelectionOverlay(
                                rect: rectInCanvas,
                                color: .accentColor,
                                isHighlightActive: session.isHighlightActive
                            ) { handle, value in
                                beginHandleDrag(handle: handle, rectInVideo: rectInVideo)
                                guard let startRect = handleStartRect else { return }
                                let updated = updatedRect(
                                    for: handle,
                                    translation: value.translation,
                                    startRect: startRect,
                                    videoSize: videoRect.size
                                )
                                session.updateSelection(normalized: updated.normalized(in: videoRect.size))
                            } onDragEnd: {
                                activeHandle = nil
                                handleStartRect = nil
                            }
                        }
                    }
                } else {
                    ContentUnavailableView("ui.placeholder_no_video", systemImage: "film", description: Text("ui.no_file"))
                }

                if session.isSamplingThreshold, let videoRect = currentVideoRect {
                    SamplingCaptureView(
                        onMove: { location in
                            handleSamplingHover(location: location, videoRect: videoRect)
                        },
                        onClick: { location in
                            handleSamplingClick(location: location, videoRect: videoRect)
                        },
                        onExit: { resetSamplingHover() }
                    )
                    .frame(maxWidth: .infinity, maxHeight: .infinity)

                    if let location = samplingHoverLocation, let brightness = samplingBrightness {
                        MagnifierOverlay(
                            image: magnifierImage,
                            brightness: brightness
                        )
                        .position(magnifierPosition(for: location, in: proxy.size))
                        .allowsHitTesting(false)
                    }

                    SamplingHintView()
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                        .padding(10)
                        .allowsHitTesting(false)
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
            .onChange(of: session.isHighlightActive) { _, isOn in
                if isOn {
                    refreshHighlightMask()
                } else {
                    highlightImage = nil
                }
            }
            .onChange(of: session.highlightTint) { _, _ in
                refreshHighlightMask(clearFirst: true)
            }
            .onChange(of: session.threshold) { _, _ in
                refreshHighlightMask()
            }
            .onChange(of: session.tolerance) { _, _ in
                refreshHighlightMask()
            }
            .onChange(of: session.selection) { _, _ in
                refreshHighlightMask()
            }
            .onChange(of: session.isSamplingThreshold) { _, isSampling in
                if isSampling {
                    samplingFrame = session.snapshotCurrentFrame(lumaOnly: false)
                    resetSamplingHover()
                } else {
                    samplingFrame = nil
                    resetSamplingHover()
                }
            }
            .onChange(of: session.currentTime) { _, _ in
                refreshHighlightMask()
                if session.isSamplingThreshold {
                    samplingFrame = nil
                }
            }
            .onChange(of: session.isPlaying) { _, _ in refreshHighlightMask() }
            .onChange(of: session.previewMode) { _, _ in
                refreshHighlightMask(clearFirst: true)
                if session.isSamplingThreshold {
                    samplingFrame = nil
                }
            }
            .onAppear {
                if session.isHighlightActive {
                    refreshHighlightMask()
                }
            }
            .onExitCommand {
                if session.isSamplingThreshold {
                    session.cancelThresholdSampling()
                }
            }
        }
    }

    private func handleDragChanged(value: DragGesture.Value, in containerSize: CGSize) {
        guard !session.isSamplingThreshold else { return }
        guard session.selectionVisible, let videoRect = videoRect(in: containerSize) else { return }
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
        guard !session.isSamplingThreshold else { return }
        dragOrigin = nil
        handleStartRect = nil
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

    private func beginHandleDrag(handle: SelectionHandle, rectInVideo: CGRect) {
        if activeHandle == nil {
            activeHandle = handle
            handleStartRect = rectInVideo
        }
    }

    private func updatedRect(for handle: SelectionHandle, translation: CGSize, startRect: CGRect, videoSize: CGSize) -> CGRect {
        var rect = startRect
        switch handle {
        case .topLeft:
            rect.origin.x += translation.width
            rect.origin.y += translation.height
            rect.size.width -= translation.width
            rect.size.height -= translation.height
        case .topRight:
            rect.origin.y += translation.height
            rect.size.width += translation.width
            rect.size.height -= translation.height
        case .bottomLeft:
            rect.origin.x += translation.width
            rect.size.width -= translation.width
            rect.size.height += translation.height
        case .bottomRight:
            rect.size.width += translation.width
            rect.size.height += translation.height
        case .move:
            rect.origin.x += translation.width
            rect.origin.y += translation.height
        }
        return clampRect(rect, in: videoSize)
    }

    private func clampRect(_ rect: CGRect, in size: CGSize) -> CGRect {
        let minSide: CGFloat = 4
        var clamped = rect
        clamped.size.width = max(minSide, min(clamped.width, size.width))
        clamped.size.height = max(minSide, min(clamped.height, size.height))
        clamped.origin.x = min(max(clamped.origin.x, 0), size.width - clamped.width)
        clamped.origin.y = min(max(clamped.origin.y, 0), size.height - clamped.height)
        return clamped
    }

    private func refreshHighlightMask(clearFirst: Bool = false) {
        guard session.isHighlightActive else {
            highlightImage = nil
            return
        }
        guard let selection = session.selection else {
            highlightImage = nil
            return
        }
        if clearFirst {
            highlightImage = nil
        }
        guard !isComputingHighlight else { return }
        guard let frame = session.snapshotCurrentFrame(lumaOnly: session.previewMode == .luma) else {
            highlightImage = nil
            return
        }

        let imageSize = CGSize(width: CGFloat(frame.width), height: CGFloat(frame.height))
        let regionInPixels = selection.denormalized(in: imageSize)
        let clamped = regionInPixels.intersection(CGRect(origin: .zero, size: imageSize))
        guard clamped.width >= 1, clamped.height >= 1 else {
            highlightImage = nil
            return
        }

        isComputingHighlight = true
        let threshold = session.threshold
        let tolerance = session.tolerance
        let tint = session.highlightTint

        DispatchQueue.global(qos: .userInitiated).async {
            let mask = HighlightMaskRenderer.makeMask(
                frame: frame,
                selection: clamped,
                threshold: threshold,
                tolerance: tolerance,
                tint: tint
            )
            DispatchQueue.main.async {
                self.isComputingHighlight = false
                if self.session.isHighlightActive, self.session.selection != nil {
                    self.highlightImage = mask
                } else {
                    self.highlightImage = nil
                }
            }
        }
    }

    private func handleSamplingHover(location: CGPoint, videoRect: CGRect) {
        guard session.isSamplingThreshold else { return }
        guard videoRect.contains(location) else {
            resetSamplingHover()
            return
        }
        guard let frame = ensureSamplingFrame() else {
            resetSamplingHover()
            return
        }
        let pixel = pixelPoint(for: location, in: videoRect, frame: frame)
        samplingBrightness = brightness(at: pixel, in: frame)
        magnifierImage = makeMagnifierImage(from: frame, around: pixel)
        samplingHoverLocation = location
    }

    private func handleSamplingClick(location: CGPoint, videoRect: CGRect) {
        guard session.isSamplingThreshold else { return }
        guard videoRect.contains(location) else {
            session.cancelThresholdSampling()
            resetSamplingHover()
            return
        }
        guard let frame = ensureSamplingFrame() else {
            session.cancelThresholdSampling()
            resetSamplingHover()
            return
        }
        let pixel = pixelPoint(for: location, in: videoRect, frame: frame)
        if let value = brightness(at: pixel, in: frame) {
            session.applySampledThreshold(value)
        } else {
            session.cancelThresholdSampling()
        }
        resetSamplingHover()
    }

    private func resetSamplingHover() {
        samplingHoverLocation = nil
        samplingBrightness = nil
        magnifierImage = nil
    }

    private func ensureSamplingFrame() -> CGImage? {
        if let frame = samplingFrame { return frame }
        guard let snapshot = session.snapshotCurrentFrame(lumaOnly: false) else { return nil }
        samplingFrame = snapshot
        return snapshot
    }

    private func pixelPoint(for location: CGPoint, in videoRect: CGRect, frame: CGImage) -> CGPoint {
        let normalized = CGPoint(
            x: (location.x - videoRect.minX) / videoRect.width,
            y: (location.y - videoRect.minY) / videoRect.height
        )
        let clampedX = min(max(normalized.x, 0), 1)
        let clampedY = min(max(normalized.y, 0), 1)
        return CGPoint(
            x: clampedX * CGFloat(frame.width - 1),
            y: clampedY * CGFloat(frame.height - 1)
        )
    }

    private func brightness(at pixel: CGPoint, in frame: CGImage) -> Double? {
        let x = Int(pixel.x.rounded())
        let y = Int(pixel.y.rounded())
        guard x >= 0, x < frame.width, y >= 0, y < frame.height else { return nil }
        guard let cropped = frame.cropping(to: CGRect(x: x, y: y, width: 1, height: 1)) else { return nil }
        var data = [UInt8](repeating: 0, count: 4)
        guard let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) else { return nil }
        guard let context = CGContext(
            data: &data,
            width: 1,
            height: 1,
            bitsPerComponent: 8,
            bytesPerRow: 4,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue
        ) else { return nil }
        context.draw(cropped, in: CGRect(x: 0, y: 0, width: 1, height: 1))
        let r = Double(data[0])
        let g = Double(data[1])
        let b = Double(data[2])
        return 0.299 * r + 0.587 * g + 0.114 * b
    }

    private func makeMagnifierImage(from frame: CGImage, around pixel: CGPoint) -> CGImage? {
        let sampleSize = 18
        let half = sampleSize / 2
        let originX = max(0, Int(pixel.x) - half)
        let originY = max(0, Int(pixel.y) - half)
        let width = min(sampleSize, frame.width - originX)
        let height = min(sampleSize, frame.height - originY)
        guard width > 0, height > 0 else { return nil }
        let rect = CGRect(x: originX, y: originY, width: width, height: height)
        return frame.cropping(to: rect)
    }

    private func magnifierPosition(for location: CGPoint, in containerSize: CGSize) -> CGPoint {
        let offset: CGFloat = 80
        var x = location.x + offset
        var y = location.y - offset
        let padding: CGFloat = 60
        if x + padding > containerSize.width {
            x = location.x - offset
        }
        if y - padding < 0 {
            y = location.y + offset
        }
        x = min(max(x, 24), containerSize.width - 24)
        y = min(max(y, 24), containerSize.height - 24)
        return CGPoint(x: x, y: y)
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
    
    private var totalFrames: Int? {
        guard let fps = session.videoFrameRate, fps > 0, session.duration > 0 else { return nil }
        let count = Int((session.duration * fps).rounded(.down))
        return max(count, 1)
    }

    private var currentFrame: Int? {
        guard let fps = session.videoFrameRate, fps > 0 else { return nil }
        let frameIndex = Int((session.currentTime * fps).rounded(.down)) + 1
        guard let total = totalFrames else { return max(frameIndex, 1) }
        return min(total, max(frameIndex, 1))
    }

    private var frameDisplay: String {
        guard let current = currentFrame else { return "Frame --" }
        if let total = totalFrames {
            return "Frame \(current)/\(total)"
        }
        return "Frame \(current)"
    }

    var body: some View {
        let controlHeight: CGFloat = 70

        ZStack {
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .fill(.thinMaterial)

            VStack(spacing: 6) {
                HStack(alignment: .center, spacing: 8) {
                    Button {
                        session.togglePlayPause()
                    } label: {
                        Image(systemName: session.isPlaying ? "pause.fill" : "play.fill")
                            .font(.title3.weight(.semibold))
                            .frame(width: 28, height: 28)
                    }
                    .buttonStyle(.plain)
                    .background(.ultraThinMaterial, in: Circle())
                    .overlay(Circle().stroke(Color.primary.opacity(0.08)))

                    Slider(
                        value: Binding(
                            get: { session.currentTime },
                            set: { session.seek(to: $0) }
                        ),
                        in: 0...(max(session.duration, 1))
                    )
                    .controlSize(.small)
                }

                HStack(alignment: .center, spacing: 10) {
                    Text(formattedTime(session.currentTime))
                        .font(.caption2.monospacedDigit())

                    Text(frameDisplay)
                        .font(.caption2.monospacedDigit())
                        .foregroundStyle(.secondary)

                    Spacer()

                    HStack(spacing: 6) {
                        pillButton(title: "-1f") { session.stepFrame(forward: false) }
                        pillButton(title: "+1f") { session.stepFrame(forward: true) }
                    }

                    HStack(spacing: 6) {
                        pillButton(title: "-1s") { session.jumpBy(seconds: -1) }
                        pillButton(title: "+1s") { session.jumpBy(seconds: 1) }
                    }

                    Text(formattedTime(session.duration))
                        .font(.caption2.monospacedDigit())
                        .foregroundStyle(.secondary)
                }
            }
            .padding(.top, 14)
            .padding([.horizontal, .bottom], 8)
        }
        .frame(height: controlHeight)
        .overlay(
            RoundedRectangle(cornerRadius: 12, style: .continuous)
                .stroke(Color.primary.opacity(0.08))
        )
        .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
    }

    private func pillButton(title: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(title)
                .font(.caption2.weight(.semibold))
                .padding(.horizontal, 6)
                .frame(height: 22)
        }
        .buttonStyle(.plain)
        .background(.ultraThinMaterial, in: Capsule())
        .overlay(
            Capsule()
                .stroke(Color.primary.opacity(0.08))
        )
    }
}

enum SelectionHandle: CaseIterable {
    case topLeft
    case topRight
    case bottomLeft
    case bottomRight
    case move

    static var cornerCases: [SelectionHandle] {
        [.topLeft, .topRight, .bottomLeft, .bottomRight]
    }

    func position(in rect: CGRect) -> CGPoint {
        switch self {
        case .topLeft:
            return CGPoint(x: rect.minX, y: rect.minY)
        case .topRight:
            return CGPoint(x: rect.maxX, y: rect.minY)
        case .bottomLeft:
            return CGPoint(x: rect.minX, y: rect.maxY)
        case .bottomRight:
            return CGPoint(x: rect.maxX, y: rect.maxY)
        case .move:
            return CGPoint(x: rect.midX, y: rect.midY)
        }
    }
}

struct SelectionOverlay: View {
    let rect: CGRect
    let color: Color
    let isHighlightActive: Bool
    let onDrag: (SelectionHandle, DragGesture.Value) -> Void
    let onDragEnd: () -> Void

    var body: some View {
        ZStack(alignment: .topLeading) {
            Path(rect)
                .stroke(color, style: StrokeStyle(lineWidth: 2, dash: [6, 4]))
            
            Path(rect)
                .fill(color.opacity(isHighlightActive ? 0.08 : 0.1))

            Rectangle()
                .fill(Color.clear)
                .frame(width: rect.width, height: rect.height)
                .position(x: rect.midX, y: rect.midY)
                .contentShape(Rectangle())
                .highPriorityGesture(dragGesture(for: .move))

            ForEach(SelectionHandle.cornerCases, id: \.self) { handle in
                handleView(for: handle)
                    .position(handle.position(in: rect))
                    .highPriorityGesture(dragGesture(for: handle))
            }
        }
    }

    private func dragGesture(for handle: SelectionHandle) -> some Gesture {
        DragGesture()
            .onChanged { value in
                onDrag(handle, value)
            }
            .onEnded { _ in
                onDragEnd()
            }
    }

    private func handleView(for handle: SelectionHandle) -> some View {
        let size: CGFloat = 12
        return RoundedRectangle(cornerRadius: 3, style: .continuous)
            .fill(.thinMaterial)
            .frame(width: size, height: size)
            .overlay(
                RoundedRectangle(cornerRadius: 3, style: .continuous)
                    .stroke(color, lineWidth: 1.5)
            )
    }
}

private struct SamplingCaptureView: NSViewRepresentable {
    let onMove: (CGPoint) -> Void
    let onClick: (CGPoint) -> Void
    let onExit: () -> Void

    func makeNSView(context: Context) -> SamplingTrackingView {
        let view = SamplingTrackingView()
        view.onMove = onMove
        view.onClick = onClick
        view.onExit = onExit
        return view
    }

    func updateNSView(_ nsView: SamplingTrackingView, context: Context) {
        nsView.onMove = onMove
        nsView.onClick = onClick
        nsView.onExit = onExit
    }
}

private final class SamplingTrackingView: NSView {
    var onMove: ((CGPoint) -> Void)?
    var onClick: ((CGPoint) -> Void)?
    var onExit: (() -> Void)?
    private var trackingArea: NSTrackingArea?
    private var cursorPushed = false

    override var acceptsFirstResponder: Bool { true }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let trackingArea {
            removeTrackingArea(trackingArea)
        }
        let options: NSTrackingArea.Options = [
            .mouseMoved,
            .mouseEnteredAndExited,
            .activeInKeyWindow,
            .inVisibleRect,
            .enabledDuringMouseDrag
        ]
        let area = NSTrackingArea(rect: bounds, options: options, owner: self, userInfo: nil)
        addTrackingArea(area)
        trackingArea = area
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        window?.acceptsMouseMovedEvents = true
    }

    override func mouseEntered(with event: NSEvent) {
        super.mouseEntered(with: event)
        pushCursor()
    }

    override func mouseMoved(with event: NSEvent) {
        super.mouseMoved(with: event)
        pushCursor()
        onMove?(flipped(event))
    }

    override func mouseExited(with event: NSEvent) {
        super.mouseExited(with: event)
        popCursor()
        onExit?()
    }

    override func mouseDown(with event: NSEvent) {
        super.mouseDown(with: event)
        onClick?(flipped(event))
    }

    override func resetCursorRects() {
        super.resetCursorRects()
        addCursorRect(bounds, cursor: .crosshair)
    }

    deinit {
        popCursor()
    }

    private func pushCursor() {
        guard !cursorPushed else { return }
        NSCursor.crosshair.push()
        cursorPushed = true
    }

    private func popCursor() {
        guard cursorPushed else { return }
        NSCursor.pop()
        cursorPushed = false
    }

    private func flipped(_ event: NSEvent) -> CGPoint {
        let point = convert(event.locationInWindow, from: nil)
        return CGPoint(x: point.x, y: bounds.height - point.y)
    }
}

private struct MagnifierOverlay: View {
    let image: CGImage?
    let brightness: Double

    var body: some View {
        VStack(spacing: 8) {
            if let image {
                ZStack {
                    Image(decorative: image, scale: 1.0)
                        .resizable()
                        .interpolation(.none)
                        .frame(width: 96, height: 96)
                        .background(.thinMaterial)
                        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
                        .overlay(
                            RoundedRectangle(cornerRadius: 10, style: .continuous)
                                .stroke(Color.primary.opacity(0.15), lineWidth: 1)
                        )

                    CenterPointer()
                }
            }

            HStack(spacing: 6) {
                Image(systemName: "sun.max")
                Text(LocalizedStringKey("ui.threshold"))
                Text(String(format: "%.0f", brightness))
                    .font(.caption.monospacedDigit())
            }
            .font(.caption)
            .padding(.horizontal, 10)
            .padding(.vertical, 6)
            .background(.ultraThinMaterial, in: Capsule())
            .shadow(color: Color.black.opacity(0.12), radius: 6, x: 0, y: 4)
        }
        .padding(6)
    }
}

private struct CenterPointer: View {
    var body: some View {
        ZStack {
            Rectangle()
                .fill(Color.white.opacity(0.85))
                .frame(width: 1, height: 36)
            Rectangle()
                .fill(Color.white.opacity(0.85))
                .frame(width: 36, height: 1)
            Circle()
                .stroke(Color.white.opacity(0.9), lineWidth: 1.5)
                .frame(width: 20, height: 20)
            Circle()
                .fill(Color.black.opacity(0.45))
                .frame(width: 6, height: 6)
        }
    }
}

private struct SamplingHintView: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Label("ui.sample_threshold", systemImage: "scope")
                .font(.footnote.weight(.semibold))
            Text("ui.sampling_hint")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
        .padding(10)
        .background(.thinMaterial, in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .stroke(Color.primary.opacity(0.12), lineWidth: 1)
        )
    }
}

private enum HighlightMaskRenderer {
    static func makeMask(
        frame: CGImage,
        selection: CGRect,
        threshold: Double,
        tolerance: Double,
        tint: NSColor
    ) -> CGImage? {
        let frameSize = CGSize(width: CGFloat(frame.width), height: CGFloat(frame.height))
        let bounded = selection
            .integral
            .intersection(CGRect(origin: .zero, size: frameSize))
        guard bounded.width >= 1, bounded.height >= 1 else { return nil }

        guard let cropped = frame.cropping(to: bounded) else { return nil }
        let width = cropped.width
        let height = cropped.height
        let bytesPerPixel = 4
        let bytesPerRow = bytesPerPixel * width
        let bufferSize = bytesPerRow * height

        guard let data = malloc(bufferSize) else { return nil }
        defer { free(data) }

        guard let colorSpace = CGColorSpace(name: CGColorSpace.sRGB) else { return nil }
        guard let context = CGContext(
            data: data,
            width: width,
            height: height,
            bitsPerComponent: 8,
            bytesPerRow: bytesPerRow,
            space: colorSpace,
            bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue | CGBitmapInfo.byteOrder32Big.rawValue
        ) else {
            return nil
        }

        context.draw(cropped, in: CGRect(x: 0, y: 0, width: width, height: height))

        guard let sRGBTint = tint.usingColorSpace(.sRGB) else { return nil }
        let alpha: Double = 0.8
        let alphaByte = UInt8(clamping: Int(alpha * 255))
        let tintR = UInt8(clamping: Int(sRGBTint.redComponent * 255 * alpha))
        let tintG = UInt8(clamping: Int(sRGBTint.greenComponent * 255 * alpha))
        let tintB = UInt8(clamping: Int(sRGBTint.blueComponent * 255 * alpha))

        let minThreshold = max(0, threshold - tolerance)
        let maxThreshold = min(255, threshold + tolerance)
        let buffer = data.bindMemory(to: UInt8.self, capacity: bufferSize)

        for y in 0..<height {
            let row = buffer.advanced(by: y * bytesPerRow)
            for x in 0..<width {
                let idx = x * bytesPerPixel
                let r = Double(row[idx])
                let g = Double(row[idx + 1])
                let b = Double(row[idx + 2])
                let luma = 0.299 * r + 0.587 * g + 0.114 * b
                if luma >= minThreshold && luma <= maxThreshold {
                    row[idx] = tintR
                    row[idx + 1] = tintG
                    row[idx + 2] = tintB
                    row[idx + 3] = alphaByte
                } else {
                    row[idx] = 0
                    row[idx + 1] = 0
                    row[idx + 2] = 0
                    row[idx + 3] = 0
                }
            }
        }

        return context.makeImage()
    }
}
