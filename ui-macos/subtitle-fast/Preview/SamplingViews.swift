import SwiftUI
import AppKit

struct SamplingCaptureView: NSViewRepresentable {
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

final class SamplingTrackingView: NSView {
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

struct MagnifierOverlay: View {
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

struct CenterPointer: View {
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

struct SamplingHintView: View {
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
