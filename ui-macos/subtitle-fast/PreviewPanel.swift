import SwiftUI
import AVFoundation
import AVKit

struct PreviewPanel: View {
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

struct VideoCanvas: View {
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
