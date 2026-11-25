package io.github.lenslink.probe

import android.media.MediaCodecInfo
import android.media.MediaCodecInfo.CodecCapabilities
import android.media.MediaCodecInfo.CodecProfileLevel
import android.media.MediaCodecInfo.EncoderCapabilities
import android.media.MediaCodecList
import android.os.Build
import org.json.JSONArray
import org.json.JSONObject
import java.util.Locale

/** Lists the video encoders that could carry the stream, with the sizes and frame rates they claim. */
object EncoderProbe {

    private val MIME_TYPES = listOf("video/avc", "video/hevc", "video/av01")

    private val SIZES = listOf(
        Triple("720p", 1280, 720),
        Triple("1080p", 1920, 1080),
        Triple("1440p", 2560, 1440),
        Triple("2160p", 3840, 2160),
        Triple("4320p", 7680, 4320),
    )

    fun run(): JSONArray {
        val result = JSONArray()
        for (info in MediaCodecList(MediaCodecList.REGULAR_CODECS).codecInfos) {
            if (!info.isEncoder) continue
            for (type in info.supportedTypes) {
                val mime = type.lowercase(Locale.US)
                if (mime !in MIME_TYPES) continue
                val caps = info.getCapabilitiesForType(type)
                result.put(describe(info, mime, caps) ?: continue)
            }
        }
        return result
    }

    private fun describe(info: MediaCodecInfo, mime: String, caps: CodecCapabilities): JSONObject? {
        val video = caps.videoCapabilities ?: return null
        val o = JSONObject()
        o.put("name", info.name)
        o.put("mime", mime)
        if (Build.VERSION.SDK_INT >= 29) {
            o.put("hardware", info.isHardwareAccelerated)
            o.put("softwareOnly", info.isSoftwareOnly)
        }
        o.put("maxSize", "${video.supportedWidths.upper}x${video.supportedHeights.upper}")
        o.put("bitrateRangeBps", JSONArray(listOf(video.bitrateRange.lower, video.bitrateRange.upper)))

        val sizes = JSONObject()
        for ((label, w, h) in SIZES) {
            if (!video.isSizeSupported(w, h)) continue
            val entry = JSONObject().put("maxFps", video.getSupportedFrameRatesFor(w, h).upper.toInt())
            // Measured rates published by the vendor, when present; more realistic than the claimed maximum.
            runCatching { video.getAchievableFrameRatesFor(w, h) }.getOrNull()?.let {
                entry.put("measuredFps", JSONArray(listOf(it.lower.toInt(), it.upper.toInt())))
            }
            sizes.put(label, entry)
        }
        o.put("sizes", sizes)

        caps.encoderCapabilities?.let { enc ->
            val modes = mutableListOf<String>()
            if (enc.isBitrateModeSupported(EncoderCapabilities.BITRATE_MODE_CQ)) modes += "CQ"
            if (enc.isBitrateModeSupported(EncoderCapabilities.BITRATE_MODE_VBR)) modes += "VBR"
            if (enc.isBitrateModeSupported(EncoderCapabilities.BITRATE_MODE_CBR)) modes += "CBR"
            if (Build.VERSION.SDK_INT >= 31 && enc.isBitrateModeSupported(EncoderCapabilities.BITRATE_MODE_CBR_FD)) {
                modes += "CBR_FD"
            }
            o.put("bitrateModes", JSONArray(modes))
        }

        o.put("profiles", JSONArray(caps.profileLevels.map { profileName(mime, it.profile) }.distinct()))
        return o
    }

    private fun profileName(mime: String, p: Int): String = when (mime) {
        "video/avc" -> when (p) {
            CodecProfileLevel.AVCProfileBaseline -> "Baseline"
            CodecProfileLevel.AVCProfileMain -> "Main"
            CodecProfileLevel.AVCProfileHigh -> "High"
            CodecProfileLevel.AVCProfileHigh10 -> "High10"
            CodecProfileLevel.AVCProfileConstrainedBaseline -> "ConstrainedBaseline"
            CodecProfileLevel.AVCProfileConstrainedHigh -> "ConstrainedHigh"
            else -> "0x" + Integer.toHexString(p)
        }
        "video/hevc" -> when (p) {
            CodecProfileLevel.HEVCProfileMain -> "Main"
            CodecProfileLevel.HEVCProfileMain10 -> "Main10"
            CodecProfileLevel.HEVCProfileMain10HDR10 -> "Main10HDR10"
            CodecProfileLevel.HEVCProfileMain10HDR10Plus -> "Main10HDR10Plus"
            else -> "0x" + Integer.toHexString(p)
        }
        else -> "0x" + Integer.toHexString(p)
    }
}
