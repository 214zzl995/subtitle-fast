import SwiftUI
import UniformTypeIdentifiers
import AppKit

struct ContentView: View {
    @StateObject private var session = DetectionSession()
    @State private var showingFilePicker = false
    private let minLeftWidth: CGFloat = 540
    private let minRightWidth: CGFloat = 240

    var body: some View {
        NavigationSplitView {
            SidebarView(session: session)
                .navigationSplitViewColumnWidth(min: 240, ideal: 280, max: 340)
        } detail: {
            NativeSplitView(
                minLeftWidth: minLeftWidth,
                minRightWidth: minRightWidth,
                initialRightWidth: minRightWidth
            ) {
                leftColumn
                    .frame(maxHeight: .infinity, alignment: .top)
            } right: {
                rightColumn
                    .frame(maxHeight: .infinity, alignment: .top)
            }
            .padding(.horizontal, 0)
            .padding(.vertical, 0)
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
        .frame(minWidth: minLeftWidth, maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
    }

    @ViewBuilder
    private var rightColumn: some View {
        VStack(spacing: 10) {
            StatusPanel(session: session)
            SubtitleListPanel(session: session)
        }
        .frame(minWidth: minRightWidth, maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
    }
}
