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
                Label("ui.preview", systemImage: "sparkles.tv.fill")
                    .font(.headline)
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
            .onChange(of: session.currentTime) { _, _ in refreshHighlightMask() }
            .onChange(of: session.isPlaying) { _, _ in refreshHighlightMask() }
            .onChange(of: session.previewMode) { _, _ in refreshHighlightMask(clearFirst: true) }
            .onAppear {
                if session.isHighlightActive {
                    refreshHighlightMask()
                }
            }
        }
    }

    private func handleDragChanged(value: DragGesture.Value, in containerSize: CGSize) {
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
