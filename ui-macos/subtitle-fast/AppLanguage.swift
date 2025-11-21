import Foundation

enum AppLanguage: String, CaseIterable, Identifiable {
    case system
    case english
    case chinese

    var id: String { rawValue }

    static func systemDefault() -> AppLanguage { .system }

    var label: String {
        switch self {
        case .system:
            return NSLocalizedString("System", comment: "System language")
        case .english:
            return "English"
        case .chinese:
            return "简体中文"
        }
    }

    var locale: Locale {
        switch self {
        case .system:
            return .autoupdatingCurrent
        case .english:
            return Locale(identifier: "en")
        case .chinese:
            return Locale(identifier: "zh-Hans")
        }
    }
}
