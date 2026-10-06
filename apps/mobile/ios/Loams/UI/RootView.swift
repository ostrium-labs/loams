import LoamsCore
import LoamsData
import SwiftUI

struct RootView: View {
    @Environment(AppModel.self) private var model
    @Environment(\.scenePhase) private var scenePhase

    var body: some View {
        ZStack {
            switch model.phase {
            case .loading:
                ProgressView()
            case .signedOut:
                WelcomeView()
            case .active:
                MainTabs()
            }
            // Hide approvals and pairing codes in the app switcher snapshot (AP3 constraints).
            if scenePhase != .active {
                Color(uiColor: .systemBackground).ignoresSafeArea().overlay(Text("Loams").font(.largeTitle))
            }
        }
    }
}

struct MainTabs: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        TabView {
            NavigationStack { ApprovalsView() }
                .tabItem { Label("Approvals", systemImage: "checkmark.shield") }
            NavigationStack { StatusView() }
                .tabItem { Label("Status", systemImage: "waveform.path.ecg") }
            NavigationStack { SettingsView() }
                .tabItem { Label("Settings", systemImage: "gearshape") }
        }
    }
}

/// A thin bar when the live stream is not up; cached data stays visible, read-only.
struct StreamBanner: View {
    let status: StreamStatus

    var body: some View {
        if let text {
            Text(text)
                .font(.footnote)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal)
                .padding(.vertical, 6)
                .background(Color.red.opacity(0.15))
                .accessibilityIdentifier("stream-banner")
        }
    }

    private var text: String? {
        switch status {
        case .live: return nil
        case .connecting: return "Connecting…"
        case .reconnecting(let retry, _): return "Offline: showing cached data. Reconnecting in \(Int(retry.rounded(.up))) s"
        case .stopped(let cause): return "Disconnected: \(cause ?? "sign in again")"
        }
    }
}
