import SwiftUI
import AVFoundation
import AVKit
import AppKit

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
                    Button {
                        session.pickFile()
                    } label: {
                        VStack(spacing: 10) {
                            Image(systemName: "film")
                                .font(.system(size: 30, weight: .light))
                            Text("ui.placeholder_no_video")
                                .font(.headline)
                            Text("ui.no_file")
                                .font(.subheadline)
                                .foregroundStyle(.secondary)
                        }
                        .foregroundStyle(Color.white.opacity(0.9))
                        .padding(20)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                    }
                    .buttonStyle(.plain)
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
            .simultaneousGesture(
                TapGesture().onEnded {
                    if session.player == nil {
                        session.pickFile()
                    }
                }
            )
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
