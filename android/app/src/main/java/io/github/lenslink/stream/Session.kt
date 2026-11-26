package io.github.lenslink.stream

import android.content.Context
import android.net.LocalSocket
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

    fun run() {
        try {
            writer.handshake()
            val reader = PacketReader(socket.inputStream)
            reader.handshake()
            writer.control(deviceInfo())
            Log.i(TAG, "PC connected")

            while (true) {
                val packet = reader.next() ?: break
                if (packet.kind != Protocol.KIND_CONTROL) continue
                handle(JSONObject(String(packet.payload, Charsets.UTF_8)))
            }
        } finally {
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

    private fun deviceInfo() = JSONObject()
        .put("type", "device_info")
        .put("manufacturer", Build.MANUFACTURER)
        .put("model", Build.MODEL)
        .put("android", Build.VERSION.RELEASE)
        .put("sdk", Build.VERSION.SDK_INT)

    private companion object {
        const val TAG = "LensLink"
    }
}
