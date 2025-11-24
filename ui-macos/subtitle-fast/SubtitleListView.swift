import SwiftUI

struct SubtitleListPanel: View {
    @ObservedObject var session: DetectionSession

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("ui.subtitles", systemImage: "text.bubble")
                .font(.headline)

            SubtitleListView(session: session)
                .frame(minHeight: 240, maxHeight: .infinity)
        }
        .padding(.vertical, 10)
        .padding(.horizontal, 10)
        .frame(maxHeight: .infinity, alignment: .top)
    }
}

struct SubtitleListView: View {
    @ObservedObject var session: DetectionSession
    private let subtitleTapOffset: TimeInterval = 0.1

    var body: some View {
        List {
            Section(header: Text("ui.subtitles").font(.caption).foregroundStyle(.secondary)) {
                ForEach(session.subtitles) { item in
                    subtitleRow(item: item)
                        .onTapGesture {
                            session.seek(to: item.timecode + subtitleTapOffset)
                        }
                }
            }
        }
        .listStyle(.plain)
        .listRowInsets(EdgeInsets(top: 6, leading: 0, bottom: 6, trailing: 0))
        .scrollContentBackground(.hidden)
        .background(Color.clear)
        .overlay {
            if session.subtitles.isEmpty {
                VStack(spacing: 10) {
                    Image(systemName: "text.bubble")
                        .font(.system(size: 24, weight: .light))
                        .foregroundStyle(.secondary)
                    Text("ui.no_subtitles_hint")
                        .font(.system(size: 12, weight: .light))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                .padding(.vertical, 16)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .center)
            }
        }
    }

    @ViewBuilder
    private func subtitleRow(item: SubtitleItem) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(formattedRange(start: item.timecode, end: item.endTime))
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
                .lineLimit(1)

            Text(item.text)
                .font(.body)
                .textSelection(.enabled)

            if let confidence = item.confidence {
                Text(String(format: "%.0f%%", confidence * 100))
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
        }
    }
}
