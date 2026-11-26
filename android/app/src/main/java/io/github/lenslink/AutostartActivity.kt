package io.github.lenslink

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import io.github.lenslink.stream.StreamService

/**
 * Invisible activity the PC launches over adb (`am start -n io.github.lenslink/.AutostartActivity`)
 * when it finds the phone but no streaming service: after a reboot, after the service was stopped.
 *
 * The camera foreground service may only start while LensLink is in the foreground (Android 14+),
 * hence an activity rather than starting the service directly. It shows over the lock screen and
 * closes itself right away. When the user stopped streaming on the phone it does nothing (and does not
 * wake the screen): only a Start button (in the app or in the "stopped" notification) resumes. The
 * notification's button comes here with [ACTION_USER_START].
 *
 * The manifest restricts it to callers holding DUMP, which adb's shell has and ordinary apps cannot
 * get, so other apps cannot start the service through it.
 */
class AutostartActivity : Activity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (intent?.action != ACTION_USER_START && StreamService.userStopped(this)) {
            finish()
            return
        }
        if (Build.VERSION.SDK_INT >= 27) {
            setShowWhenLocked(true)
            setTurnScreenOn(true)
        }
    }

    override fun onResume() {
        super.onResume()
        if (isFinishing) return
        if (checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
            if (intent?.action == ACTION_USER_START) StreamService.start(this) else StreamService.autostart(this)
            // Stay resumed a moment so the service is started while LensLink counts as visible.
            window.decorView.postDelayed({ finish() }, 500)
        } else {
            // Permissions are asked in the main screen.
            startActivity(Intent(this, MainActivity::class.java))
            finish()
        }
    }

    companion object {
        const val ACTION_USER_START = "io.github.lenslink.USER_START"
    }

    override fun finish() {
        super.finish()
        if (Build.VERSION.SDK_INT >= 34) {
            overrideActivityTransition(OVERRIDE_TRANSITION_CLOSE, 0, 0)
        } else {
            @Suppress("DEPRECATION")
            overridePendingTransition(0, 0)
        }
    }
}
