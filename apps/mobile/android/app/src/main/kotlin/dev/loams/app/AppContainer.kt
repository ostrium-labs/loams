package dev.loams.app

import android.content.Context
import android.content.Intent
import dev.loams.app.session.MockPushPoller
import dev.loams.app.session.SessionManager
import dev.loams.app.session.Settings
import dev.loams.data.keys.DeviceKeys
import dev.loams.data.session.SessionStore
import dev.loams.push.FcmStubRegistrar
import dev.loams.push.Notifier
import dev.loams.push.PushHandler
import dev.loams.push.PushKeys

/** Manual dependency wiring: a handful of singletons does not need a DI framework yet. */
class AppContainer(context: Context) {
    private val app = context.applicationContext
    val settings = Settings(app)
    val keys = DeviceKeys(app)
    val pushKeys = PushKeys(app)
    val notifier = Notifier(app) { Intent(app, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP) }
    val pushHandler = PushHandler(pushKeys, notifier)
    val mockPushPoller = MockPushPoller(pushHandler)
    val session = SessionManager(SessionStore(app), keys, pushKeys, FcmStubRegistrar(app))
}
