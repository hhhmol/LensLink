package io.github.lenslink.probe

import android.content.Context
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraCharacteristics.*
import android.hardware.camera2.CameraManager
import android.media.MediaCodec
import android.os.Build
import org.json.JSONArray
import org.json.JSONObject
import java.util.Locale

/** Collects what Camera2 really exposes to a third-party app on this device. */
object CameraProbe {

    /** Highest camera id tried beyond the official list: vendors often hide auxiliary lenses. */
    private const val MAX_GUESSED_ID = 15

    fun run(context: Context): JSONArray {
        val manager = context.getSystemService(CameraManager::class.java)
        val listed = manager.cameraIdList.toSet()

        val pending = ArrayDeque<String>()
        pending.addAll(manager.cameraIdList)
        (0..MAX_GUESSED_ID).map { it.toString() }.filterTo(pending) { it !in listed }

        val seen = HashSet<String>()
        val result = JSONArray()
        while (pending.isNotEmpty()) {
            val id = pending.removeFirst()
            if (!seen.add(id)) continue
            val ch = try {
                manager.getCameraCharacteristics(id)
            } catch (e: Exception) {
                continue // id does not exist
            }
            if (Build.VERSION.SDK_INT >= 28) pending.addAll(ch.physicalCameraIds)
            result.put(describe(id, id in listed, ch))
        }
        return result
    }

    private fun describe(id: String, listed: Boolean, ch: CameraCharacteristics): JSONObject {
        val o = JSONObject()
        o.put("id", id)
        o.put("listed", listed)
        o.put("facing", when (ch.get(LENS_FACING)) {
            LENS_FACING_FRONT -> "front"
            LENS_FACING_BACK -> "back"
            LENS_FACING_EXTERNAL -> "external"
            else -> "unknown"
        })
        o.put("hardwareLevel", hardwareLevel(ch.get(INFO_SUPPORTED_HARDWARE_LEVEL)))
        o.put("capabilities", ch.get(REQUEST_AVAILABLE_CAPABILITIES).names(::capabilityName))
        if (Build.VERSION.SDK_INT >= 28) {
            o.put("physicalCameraIds", JSONArray(ch.physicalCameraIds.toList()))
        }

        ch.get(SENSOR_INFO_PIXEL_ARRAY_SIZE)?.let { o.put("pixelArray", "${it.width}x${it.height}") }
        ch.get(SENSOR_INFO_PHYSICAL_SIZE)?.let {
            o.put("sensorSizeMm", String.format(Locale.US, "%.2fx%.2f", it.width, it.height))
        }
        ch.get(LENS_INFO_AVAILABLE_FOCAL_LENGTHS)?.let { o.put("focalLengthsMm", it.toJson()) }
        ch.get(LENS_INFO_AVAILABLE_APERTURES)?.let { o.put("apertures", it.toJson()) }
        // 0 means fixed focus; otherwise the closest focus distance in diopters (1/m).
        ch.get(LENS_INFO_MINIMUM_FOCUS_DISTANCE)?.let { o.put("minFocusDistanceDiopters", it.toDouble()) }

        ch.get(SENSOR_INFO_SENSITIVITY_RANGE)?.let { o.put("isoRange", JSONArray(listOf(it.lower, it.upper))) }
        ch.get(SENSOR_INFO_EXPOSURE_TIME_RANGE)?.let { o.put("exposureRangeNs", JSONArray(listOf(it.lower, it.upper))) }
        ch.get(CONTROL_AE_COMPENSATION_RANGE)?.let { range ->
            val step = ch.get(CONTROL_AE_COMPENSATION_STEP)?.toDouble() ?: 0.0
            o.put("exposureCompensationEv", JSONArray(listOf(range.lower * step, range.upper * step)))
        }
        o.put("aeTargetFpsRanges", JSONArray(
            ch.get(CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES)?.map { "${it.lower}-${it.upper}" } ?: emptyList<String>()
        ))
        o.put("afModes", ch.get(CONTROL_AF_AVAILABLE_MODES).names(::afModeName))
        o.put("awbModes", ch.get(CONTROL_AWB_AVAILABLE_MODES).names(::awbModeName))
        o.put("videoStabilization", ch.get(CONTROL_AVAILABLE_VIDEO_STABILIZATION_MODES).names(::stabilizationName))
        o.put("ois", ch.get(LENS_INFO_AVAILABLE_OPTICAL_STABILIZATION)?.contains(1) == true)
        o.put("flash", ch.get(FLASH_INFO_AVAILABLE) == true)

        if (Build.VERSION.SDK_INT >= 30) {
            ch.get(CONTROL_ZOOM_RATIO_RANGE)?.let { o.put("zoomRatioRange", JSONArray(listOf(it.lower.toDouble(), it.upper.toDouble()))) }
        }
        ch.get(SCALER_AVAILABLE_MAX_DIGITAL_ZOOM)?.let { o.put("maxDigitalZoom", it.toDouble()) }

        if (Build.VERSION.SDK_INT >= 33) {
            ch.get(REQUEST_AVAILABLE_DYNAMIC_RANGE_PROFILES)?.let { profiles ->
                o.put("dynamicRangeProfiles", JSONArray(profiles.supportedProfiles.sorted().map(::dynamicRangeName)))
            }
        }

        ch.get(SCALER_STREAM_CONFIGURATION_MAP)?.let { map ->
            // Sizes the camera can feed straight into a MediaCodec input surface, with max fps for each.
            val video = JSONArray()
            map.getOutputSizes(MediaCodec::class.java)
                ?.sortedByDescending { it.width.toLong() * it.height }
                ?.forEach { size ->
                    val minFrameNs = map.getOutputMinFrameDuration(MediaCodec::class.java, size)
                    val maxFps = if (minFrameNs > 0) 1_000_000_000.0 / minFrameNs else 0.0
                    video.put(JSONObject()
                        .put("size", "${size.width}x${size.height}")
                        .put("maxFps", Math.round(maxFps * 10) / 10.0))
                }
            o.put("videoSizes", video)

            val highSpeed = JSONArray()
            map.highSpeedVideoSizes?.forEach { size ->
                val ranges = map.getHighSpeedVideoFpsRangesFor(size).map { "${it.lower}-${it.upper}" }.distinct()
                highSpeed.put(JSONObject().put("size", "${size.width}x${size.height}").put("fps", JSONArray(ranges)))
            }
            o.put("highSpeedVideo", highSpeed)
        }
        return o
    }

    private fun IntArray?.names(namer: (Int) -> String) =
        JSONArray(this?.map(namer) ?: emptyList<String>())

    private fun FloatArray.toJson() = JSONArray(map { Math.round(it * 100) / 100.0 })

    private fun hardwareLevel(level: Int?) = when (level) {
        0 -> "LIMITED"
        1 -> "FULL"
        2 -> "LEGACY"
        3 -> "LEVEL_3"
        4 -> "EXTERNAL"
        else -> "UNKNOWN"
    }

    private fun capabilityName(c: Int) = when (c) {
        0 -> "BACKWARD_COMPATIBLE"
        1 -> "MANUAL_SENSOR"
        2 -> "MANUAL_POST_PROCESSING"
        3 -> "RAW"
        4 -> "PRIVATE_REPROCESSING"
        5 -> "READ_SENSOR_SETTINGS"
        6 -> "BURST_CAPTURE"
        7 -> "YUV_REPROCESSING"
        8 -> "DEPTH_OUTPUT"
        9 -> "CONSTRAINED_HIGH_SPEED_VIDEO"
        10 -> "MOTION_TRACKING"
        11 -> "LOGICAL_MULTI_CAMERA"
        12 -> "MONOCHROME"
        13 -> "SECURE_IMAGE_DATA"
        14 -> "SYSTEM_CAMERA"
        15 -> "OFFLINE_PROCESSING"
        16 -> "ULTRA_HIGH_RESOLUTION_SENSOR"
        17 -> "REMOSAIC_REPROCESSING"
        18 -> "DYNAMIC_RANGE_TEN_BIT"
        19 -> "STREAM_USE_CASE"
        20 -> "COLOR_SPACE_PROFILES"
        else -> "UNKNOWN_$c"
    }

    private fun afModeName(m: Int) = when (m) {
        0 -> "OFF"
        1 -> "AUTO"
        2 -> "MACRO"
        3 -> "CONTINUOUS_VIDEO"
        4 -> "CONTINUOUS_PICTURE"
        5 -> "EDOF"
        else -> "UNKNOWN_$m"
    }

    private fun awbModeName(m: Int) = when (m) {
        0 -> "OFF"
        1 -> "AUTO"
        2 -> "INCANDESCENT"
        3 -> "FLUORESCENT"
        4 -> "WARM_FLUORESCENT"
        5 -> "DAYLIGHT"
        6 -> "CLOUDY_DAYLIGHT"
        7 -> "TWILIGHT"
        8 -> "SHADE"
        else -> "UNKNOWN_$m"
    }

    private fun stabilizationName(m: Int) = when (m) {
        0 -> "OFF"
        1 -> "ON"
        2 -> "PREVIEW_STABILIZATION"
        else -> "UNKNOWN_$m"
    }

    private fun dynamicRangeName(p: Long) = when (p) {
        1L -> "STANDARD"
        2L -> "HLG10"
        4L -> "HDR10"
        8L -> "HDR10_PLUS"
        else -> "0x" + java.lang.Long.toHexString(p)
    }
}
