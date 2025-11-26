import SwiftUI

@main
struct SubtitleFastApp: App {
    var body: some Scene {
        WindowGroup {
            ContentView()
                .onAppear {
                    NotificationManager.shared.requestAuthorization()
                }
        }
        .defaultSize(width: 1280, height: 820)
        .windowResizability(.contentSize)
    }
}
