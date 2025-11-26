import SwiftUI

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
        let shape = RoundedRectangle(cornerRadius: 12, style: .continuous)

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
        .frame(height: controlHeight)
        .background {
            if #available(macOS 15.0, *) {
                shape.fill(.ultraThinMaterial)
            } else {
                shape
                    .fill(
                        LinearGradient(
                            colors: [
                                Color.white.opacity(0.08),
                                Color.white.opacity(0.04)
                            ],
                            startPoint: .topLeading,
                            endPoint: .bottomTrailing
                        )
                    )
            }
        }
        .overlay {
            shape.stroke(Color.white.opacity(0.08))
        }
        .clipShape(shape)
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
