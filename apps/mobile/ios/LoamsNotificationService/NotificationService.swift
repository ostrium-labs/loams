import LoamsCore
import LoamsData
import UserNotifications

/// Unseals a push and replaces the generic alert (design §37 §7.4). Fails closed: anything that
/// does not open keeps "New activity in Loams", and the app syncs its inbox when opened.
final class NotificationService: UNNotificationServiceExtension {
    private var handler: ((UNNotificationContent) -> Void)?
    private var content: UNMutableNotificationContent?

    override func didReceive(_ request: UNNotificationRequest, withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void) {
        handler = contentHandler
        let content = (request.content.mutableCopy() as? UNMutableNotificationContent) ?? UNMutableNotificationContent()
        self.content = content
        let keys = PushKeys(store: KeychainStore.shared)
        let opener = PushOpener(keyFor: { keys.privateKey(instanceID: $0) }, decode: NotificationDecoding.shown)
        let shown = opener.open(request.content.userInfo)
        content.title = shown.title
        content.body = shown.body
        if let approval = shown.approvalID {
            content.categoryIdentifier = Loams.Category.approval.rawValue
            content.userInfo["approval_id"] = approval
        }
        contentHandler(content)
    }

    override func serviceExtensionTimeWillExpire() {
        guard let handler, let content else { return }
        content.title = Shown.generic.title
        content.body = Shown.generic.body
        handler(content)
    }
}
