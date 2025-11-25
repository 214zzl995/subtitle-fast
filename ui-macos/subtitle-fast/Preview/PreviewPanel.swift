import SwiftUI

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
