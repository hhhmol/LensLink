package io.github.lenslink.stream

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.LocalServerSocket
import android.net.LocalSocket
import android.net.LocalSocketAddress
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.util.Log
import io.github.lenslink.AutostartActivity
import io.github.lenslink.MainActivity
import java.io.IOException
import kotlin.concurrent.thread

/**
 * Camera-type foreground service. Listens on the abstract socket `lenslink`, which the PC reaches with
 * `adb forward tcp:27183 localabstract:lenslink`. Must be started while the activity is visible:
 * Android 14+ only grants background camera access to services started from the foreground.
 */
class StreamService : Service() {

    private var server: LocalServerSocket? = null
    private var serverThread: Thread? = null
    private var wakeLock: PowerManager.WakeLock? = null

    @Volatile
    private var closing = false

    @Volatile
    private var client: LocalSocket? = null

    @Volatile
    private var session: Session? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_STOP) {
            setUserStopped(this, true)
            stopSelf()
            return START_NOT_STICKY
        }
        getSystemService(NotificationManager::class.java).cancel(STOPPED_NOTIFICATION_ID)
        startInForeground("Waiting for the PC")
        if (serverThread == null) startServer()
        running = true
        return START_NOT_STICKY
    }

    override fun onDestroy() {
        running = false
        closing = true
        // Tell the PC this was deliberate, so it does not start the service again.
        if (userStopped(this)) runCatching { session?.stoppedByUser() }
        runCatching { client?.shutdownInput() }
        // LocalServerSocket.close() does not interrupt accept(); a throwaway connection does.
        runCatching { LocalSocket().connect(LocalSocketAddress(Protocol.SOCKET_NAME)) }
        runCatching { server?.close() }
        releaseWakeLock()
        if (userStopped(this)) showStopped()
        super.onDestroy()
    }

    private fun startServer() {
        val socket = try {
            LocalServerSocket(Protocol.SOCKET_NAME)
        } catch (e: IOException) {
            Log.e(TAG, "cannot listen on @${Protocol.SOCKET_NAME}", e)
            stopSelf()
            return
        }
        server = socket
        serverThread = thread(name = "lenslink-server") {
            while (!closing) {
                val accepted = try {
                    socket.accept()
                } catch (e: IOException) {
                    break
                }
                if (closing) {
                    runCatching { accepted.close() }
                    break
                }
                client = accepted
                acquireWakeLock()
                updateNotification("PC connected")
                try {
                    Session(this, accepted).also { session = it }.run()
                } catch (e: Exception) {
                    Log.w(TAG, "session ended: ${e.message}")
                }
                session = null
                client = null
                releaseWakeLock()
                if (!closing) updateNotification("Waiting for the PC")
            }
        }
    }

    private fun startInForeground(text: String) {
        val nm = getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL, "Streaming", NotificationManager.IMPORTANCE_LOW))
        val notification = buildNotification(text)
        if (Build.VERSION.SDK_INT >= 30) {
            startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_CAMERA)
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
    }

    private fun updateNotification(text: String) {
        getSystemService(NotificationManager::class.java).notify(NOTIFICATION_ID, buildNotification(text))
    }

    private fun buildNotification(text: String): Notification {
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE)
        val stop = PendingIntent.getService(
            this, 1, Intent(this, StreamService::class.java).setAction(ACTION_STOP), PendingIntent.FLAG_IMMUTABLE)
        return Notification.Builder(this, CHANNEL)
            .setSmallIcon(android.R.drawable.ic_menu_camera)
            .setContentTitle("LensLink")
            .setContentText(text)
            .setContentIntent(open)
            .setOngoing(true)
            .addAction(Notification.Action.Builder(null, "Stop", stop).build())
            .build()
    }

    /** Dismissible note with a Start button, so resuming does not need the app. */
    private fun showStopped() {
        val open = PendingIntent.getActivity(
            this, 0, Intent(this, MainActivity::class.java), PendingIntent.FLAG_IMMUTABLE)
        // Through the activity: the camera service may only start while LensLink is in the foreground.
        val start = PendingIntent.getActivity(
            this, 2,
            Intent(this, AutostartActivity::class.java).setAction(AutostartActivity.ACTION_USER_START),
            PendingIntent.FLAG_IMMUTABLE)
        val notification = Notification.Builder(this, CHANNEL)
            .setSmallIcon(android.R.drawable.ic_menu_camera)
            .setContentTitle("LensLink")
            .setContentText("Streaming stopped")
            .setContentIntent(open)
            .addAction(Notification.Action.Builder(null, "Start", start).build())
            .build()
        getSystemService(NotificationManager::class.java).notify(STOPPED_NOTIFICATION_ID, notification)
    }

    private fun acquireWakeLock() {
        if (wakeLock?.isHeld == true) return
        wakeLock = getSystemService(PowerManager::class.java)
            .newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "LensLink:stream")
            .apply { acquire(12 * 60 * 60 * 1000L) }
    }

    private fun releaseWakeLock() {
        runCatching { wakeLock?.takeIf { it.isHeld }?.release() }
        wakeLock = null
    }

    companion object {
        private const val TAG = "LensLink"
        private const val CHANNEL = "stream"
        private const val NOTIFICATION_ID = 1
        private const val STOPPED_NOTIFICATION_ID = 2
        private const val ACTION_STOP = "io.github.lenslink.STOP"

        @Volatile
        var running = false
            private set

        private const val PREFS = "stream"
        private const val KEY_USER_STOPPED = "user_stopped"

        /** Started by the user: clears an earlier stop, so the PC may restart the service again. */
        fun start(context: Context) {
            setUserStopped(context, false)
            startForegroundService(context)
        }

        /** Started by the PC; does nothing while the user has the service stopped. */
        fun autostart(context: Context): Boolean {
            if (userStopped(context)) return false
            if (!running) startForegroundService(context)
            return true
        }

        /** Stopped by the user (app button or notification): the PC leaves it stopped. */
        fun stop(context: Context) {
            setUserStopped(context, true)
            context.stopService(Intent(context, StreamService::class.java))
        }

        private fun startForegroundService(context: Context) {
            context.startForegroundService(Intent(context, StreamService::class.java))
        }

        fun userStopped(context: Context) =
            context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getBoolean(KEY_USER_STOPPED, false)

        private fun setUserStopped(context: Context, value: Boolean) {
            context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
                .putBoolean(KEY_USER_STOPPED, value).apply()
        }
    }
}
