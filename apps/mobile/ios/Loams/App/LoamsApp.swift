import Combine
import LoamsCore
import SwiftUI
import UIKit
import UserNotifications

@main
struct LoamsApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @State private var model = AppModel()

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
                .task { await model.restore() }
                .onReceive(NotificationCenter.default.publisher(for: AppDelegate.openApproval)) { note in
                    model.pendingApprovalID = note.object as? String
                }
        }
    }
}

/// Notification categories and the Review action (AP3 Ruling 1, Ruling 5). APNs registration is
/// a stub until the app is signed with the aps-environment entitlement (TODO(Q420)).
final class AppDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    static let openApproval = Notification.Name("dev.loams.app.openApproval")

    func application(_ application: UIApplication, didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]? = nil) -> Bool {
        let center = UNUserNotificationCenter.current()
        center.delegate = self
        // REVIEW opens the app on the approval; deciding needs Face ID there. No background approve.
        let review = UNNotificationAction(identifier: Loams.reviewAction, title: "Review", options: [.foreground, .authenticationRequired])
        center.setNotificationCategories([
            UNNotificationCategory(identifier: Loams.Category.approval.rawValue, actions: [review], intentIdentifiers: []),
            UNNotificationCategory(identifier: Loams.Category.operation.rawValue, actions: [], intentIdentifiers: []),
            UNNotificationCategory(identifier: Loams.Category.job.rawValue, actions: [], intentIdentifiers: []),
            UNNotificationCategory(identifier: Loams.Category.security.rawValue, actions: [], intentIdentifiers: []),
        ])
        return true
    }

    func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        // TODO(Q420): send this token in RegisterPushTarget instead of ApnsStub.token().
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
        let id = response.notification.request.content.userInfo["approval_id"] as? String
        await MainActor.run { NotificationCenter.default.post(name: Self.openApproval, object: id) }
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification) async -> UNNotificationPresentationOptions {
        [.banner, .list, .sound]
    }
}
