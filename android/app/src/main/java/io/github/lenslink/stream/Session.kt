package io.github.lenslink.stream

import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.LocalSocket
import android.os.BatteryManager
import android.os.Build
import android.util.Log
import io.github.lenslink.probe.CameraProbe
import io.github.lenslink.probe.EncoderProbe
import org.json.JSONObject

/** One connected PC. Blocks the calling thread until the PC disconnects. */
class Session(
    private val context: Context,
    private val socket: LocalSocket,
    /** Called with true when the camera starts streaming and false when it stops. */
    private val onStreaming: (Boolean) -> Unit = {},
) {

    private val writer = PacketWriter(socket.outputStream)
    private var streamer: CameraStreamer? = null
    // Survive mode switches: a new `start` keeps the settings made with `set`.
    private var controls = Controls()
    private var reporter: Thread? = null

    fun run() {
        try {
            writer.handshake()
            val reader = PacketReader(socket.inputStream)
            reader.handshake()
            writer.control(deviceInfo())
            Log.i(TAG, "PC connected")
            reporter = Thread(::reportStatus, "status").apply { isDaemon = true; start() }

            while (true) {
                val packet = reader.next() ?: break
                if (packet.kind != Protocol.KIND_CONTROL) continue
                handle(JSONObject(String(packet.payload, Charsets.UTF_8)))
            }
        } finally {
            reporter?.interrupt()
            streamer?.stop()
            if (streamer != null) onStreaming(false)
            runCatching { socket.close() }
            Log.i(TAG, "PC disconnected")
        }
    }

    private fun handle(msg: JSONObject) {
        when (msg.optString("type")) {
            "start" -> {
                streamer?.stop()
                val config = StreamConfig.from(msg)
                streamer = CameraStreamer(context, config, writer, controls, ::onStreamFailed).also { it.start() }
                onStreaming(true)
            }
            "set" -> {
                controls = controls.merge(msg)
                streamer?.update(controls)
            }
            "stop" -> {
                streamer?.stop()
                streamer = null
                onStreaming(false)
            }
            "get_capabilities" -> writer.control(JSONObject()
                .put("type", "capabilities")
                .put("cameras", CameraProbe.run(context))
                .put("encoders", EncoderProbe.run()))
            else -> Log.w(TAG, "ignoring control message ${msg.optString("type")}")
        }
    }

    /** The user stopped streaming on the phone. */
    fun stoppedByUser() {
        writer.control(JSONObject().put("type", "stopped").put("reason", "user"))
    }

    private fun onStreamFailed(e: Throwable) {
        runCatching { writer.control(JSONObject().put("type", "error").put("message", e.message ?: e.toString())) }
            .onFailure {
                // The socket itself is broken: wake the reader so the session ends.
                runCatching { socket.shutdownInput() }
            }
    }

    /** The battery's charge and temperature, every few seconds, for the PC's widgets. */
    private fun reportStatus() {
        try {
            while (!Thread.currentThread().isInterrupted) {
                val battery = context.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
                if (battery != null) {
                    val level = battery.getIntExtra(BatteryManager.EXTRA_LEVEL, -1)
                    val scale = battery.getIntExtra(BatteryManager.EXTRA_SCALE, 100)
                    val tenths = battery.getIntExtra(BatteryManager.EXTRA_TEMPERATURE, Int.MIN_VALUE)
                    val plugged = battery.getIntExtra(BatteryManager.EXTRA_PLUGGED, 0) != 0
                    val msg = JSONObject().put("type", "status").put("charging", plugged)
                    if (level >= 0 && scale > 0) msg.put("battery", level * 100 / scale)
                    if (tenths != Int.MIN_VALUE) msg.put("temperature", tenths / 10.0)
                    writer.control(msg)
                }
                Thread.sleep(STATUS_PERIOD_MS)
            }
        } catch (_: InterruptedException) {
        } catch (e: Exception) {
            // The connection is gone; the session's reader notices on its own.
            Log.d(TAG, "status reports stop: $e")
        }
    }

    private fun deviceInfo() = JSONObject()
        .put("type", "device_info")
        .put("manufacturer", Build.MANUFACTURER)
        .put("model", Build.MODEL)
        .put("android", Build.VERSION.RELEASE)
        .put("sdk", Build.VERSION.SDK_INT)

    private companion object {
        const val TAG = "LensLink"
        const val STATUS_PERIOD_MS = 5000L
    }
}
