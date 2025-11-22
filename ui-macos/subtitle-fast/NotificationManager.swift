import Foundation
import UserNotifications

final class NotificationManager: NSObject, UNUserNotificationCenterDelegate {
    static let shared = NotificationManager()
    
    private var permissionGranted = false
    
    private override init() {
        super.init()
        UNUserNotificationCenter.current().delegate = self
    }
    
    func requestAuthorization() {
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { [weak self] granted, _ in
            DispatchQueue.main.async {
                self?.permissionGranted = granted
            }
        }
    }
    
    func notifyDetectionFinished(fileName: String, success: Bool, message: String?) {
        guard permissionGranted else { return }
        
        let content = UNMutableNotificationContent()
        if success {
            content.title = NSLocalizedString("ui.notification_done_title", comment: "detection finished")
            content.body = String(format: NSLocalizedString("ui.notification_done_body", comment: "detection finished body"), fileName)
        } else {
            content.title = NSLocalizedString("ui.notification_failed_title", comment: "detection failed")
            let fallback = NSLocalizedString("ui.notification_failed_body", comment: "detection failed body")
            content.body = message ?? String(format: fallback, fileName)
        }
        
        let request = UNNotificationRequest(
            identifier: UUID().uuidString,
            content: content,
            trigger: nil
        )
        UNUserNotificationCenter.current().add(request)
    }
    
    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        completionHandler([.banner, .sound])
    }
}
