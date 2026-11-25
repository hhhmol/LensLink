package io.github.lenslink.stream

import android.media.MediaFormat
import org.json.JSONObject

/** Parameters of a `start` control message. */
data class StreamConfig(
    val cameraId: String,
    val width: Int,
    val height: Int,
    val fps: Int,
    val codec: String,
    val bitrate: Int,
    val zoom: Float?,
    val stabilization: Boolean,
    val keyframeIntervalSec: Int,
) {
    val mime: String
        get() = if (codec == "h265") MediaFormat.MIMETYPE_VIDEO_HEVC else MediaFormat.MIMETYPE_VIDEO_AVC

    companion object {
        fun from(j: JSONObject) = StreamConfig(
            cameraId = j.optString("camera_id", "0"),
            width = j.optInt("width", 1920),
            height = j.optInt("height", 1080),
            fps = j.optInt("fps", 30),
            codec = j.optString("codec", "h264").lowercase(),
            bitrate = j.optInt("bitrate", 20_000_000),
            zoom = if (j.has("zoom")) j.getDouble("zoom").toFloat() else null,
            stabilization = j.optBoolean("stabilization", true),
            keyframeIntervalSec = j.optInt("keyframe_interval", 2),
        )
    }
}
