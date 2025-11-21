import SwiftUI

@main
struct SubtitleFastApp: App {
    var body: some Scene {
        WindowGroup {
            ContentView()
        }
        
        Settings {
            SettingsView()
        }
    }
}

struct SettingsView: View {
    @AppStorage("appLanguage") private var appLanguage: AppLanguage = .systemDefault()
    
    var body: some View {
        Form {
            Picker("ui.language", selection: $appLanguage) {
                ForEach(AppLanguage.allCases) { lang in
                    Text(lang.label).tag(lang)
                }
            }
            .pickerStyle(.inline)
        }
        .frame(width: 300, height: 100)
        .padding()
    }
}
