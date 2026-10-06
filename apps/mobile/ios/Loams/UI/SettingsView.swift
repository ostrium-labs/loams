import LoamsCore
import SwiftUI
import UserNotifications

struct SettingsView: View {
    @Environment(AppModel.self) private var model
    @State private var confirmSignOut = false

    var body: some View {
        Form {
            if let message = model.message { Section { Text(message) } }
            Section("This phone") {
                if let r = model.record {
                    Text("Instance: \(r.issuer)")
                    Text("Instance id: \(r.instanceID)").font(.caption)
                    Text("Device id: \(r.deviceID) (paired by \(r.method))").font(.caption)
                    Text("Instance key: \(r.jkt.prefix(16))…").font(.caption)
                } else {
                    Text("Demo mode: in-memory data, nothing leaves the phone.")
                }
            }
            Section("Notifications") {
                Text("Pushes are sealed to this phone's key, so Apple and the push gateway see no content. APNs is a stub in this build: no Apple Developer account yet.")
                    .font(.footnote)
                Button("Allow notifications") {
                    Task { _ = try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge]) }
                }
                Button("Register for push") { Task { await model.registerPush() } }
                Button("Send a test notification") { Task { await model.sendTestNotification() } }
                    .disabled(model.record == nil)
            }
            Section {
                Button(model.record == nil ? "Leave demo mode" : "Sign out and forget this instance", role: .destructive) { confirmSignOut = true }
            }
            Section {
                Text("Loams \(Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "") · Apache License 2.0").font(.caption)
            }
        }
        .navigationTitle("Settings")
        .confirmationDialog("Sign out? This deletes this phone's keys and tokens for the instance.", isPresented: $confirmSignOut, titleVisibility: .visible) {
            Button("Sign out", role: .destructive) { Task { await model.signOut() } }
        }
    }
}
