package dev.loams.core

/** Identifiers fixed by AP2 Ruling 1. */
object Loams {
    const val APPLICATION_ID = "dev.loams.app"
    const val CLIENT_ID = "loams-android"
    const val PAIRING_GRANT = "urn:loams:params:oauth:grant-type:pairing"
    const val TOKEN_EXCHANGE_GRANT = "urn:ietf:params:oauth:grant-type:token-exchange"

    /** Notification channels (AP2 Ruling 1); ids are permanent once released. */
    object Channels {
        const val APPROVALS = "approvals"
        const val OPERATIONS = "operations"
        const val JOBS = "jobs"
        const val SECURITY = "security"
        const val CONNECTION = "connection"
    }
}
