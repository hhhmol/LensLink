package io.github.lenslink.probe

import org.json.JSONArray
import org.json.JSONObject
import java.util.Locale

/** Renders the JSON report as plain ASCII text for the screen and the console. */
object ReportText {

    fun render(report: JSONObject): String = buildString {
        val d = report.getJSONObject("device")
        appendLine("LensLink camera probe")
        append("${d.optString("manufacturer")} ${d.optString("model")} (${d.optString("device")})")
        append(", Android ${d.optString("android")} / SDK ${d.optInt("sdk")}")
        d.optString("soc").takeIf { it.isNotEmpty() }?.let { append(", SoC $it") }
        appendLine()

        val cameras = report.getJSONArray("cameras")
        for (i in 0 until cameras.length()) camera(cameras.getJSONObject(i))

        appendLine()
        appendLine("== Encoders ==")
        val encoders = report.getJSONArray("encoders")
        for (i in 0 until encoders.length()) encoder(encoders.getJSONObject(i))
    }

    private fun StringBuilder.camera(c: JSONObject) {
        appendLine()
        val hidden = if (c.optBoolean("listed")) "" else "  [hidden: not in cameraIdList]"
        appendLine("== Camera ${c.getString("id")}: ${c.optString("facing")}, ${c.optString("hardwareLevel")}$hidden ==")

        val physical = c.optJSONArray("physicalCameraIds").strings()
        if (physical.isNotEmpty()) appendLine("  logical camera of: ${physical.joinToString()}")

        val optics = listOfNotNull(
            c.optString("pixelArray").ifEmpty { null }?.let { "sensor $it" },
            c.optString("sensorSizeMm").ifEmpty { null }?.let { "$it mm" },
            c.optJSONArray("focalLengthsMm").strings().takeIf { it.isNotEmpty() }?.let { "focal ${it.joinToString("/")} mm" },
            c.optJSONArray("apertures").strings().takeIf { it.isNotEmpty() }?.let { "f/${it.joinToString("/")}" },
        )
        if (optics.isNotEmpty()) appendLine("  ${optics.joinToString(", ")}")

        val caps = c.optJSONArray("capabilities").strings()
        appendLine("  manual sensor: ${yes("MANUAL_SENSOR" in caps)}, manual post-processing: ${yes("MANUAL_POST_PROCESSING" in caps)}, RAW: ${yes("RAW" in caps)}")
        appendLine("  capabilities: ${caps.joinToString()}")

        // Some HALs (Xiaomi) omit the key entirely; that is not the same as reporting 0 = fixed focus.
        val focus = when {
            !c.has("minFocusDistanceDiopters") -> "min distance not reported (manual focus distance unavailable)"
            c.getDouble("minFocusDistanceDiopters") == 0.0 -> "fixed"
            else -> String.format(Locale.US, "closest %.1f cm", 100.0 / c.getDouble("minFocusDistanceDiopters"))
        }
        appendLine("  focus: $focus; AF ${c.optJSONArray("afModes").strings().joinToString()}")
        appendLine("  white balance: ${c.optJSONArray("awbModes").strings().joinToString()}")

        c.optJSONArray("isoRange")?.let { appendLine("  ISO ${it.optLong(0)}-${it.optLong(1)}") }
        c.optJSONArray("exposureRangeNs")?.let {
            appendLine("  exposure ${seconds(it.optLong(0))} .. ${seconds(it.optLong(1))}")
        }
        c.optJSONArray("exposureCompensationEv")?.let {
            appendLine(String.format(Locale.US, "  exposure compensation %+.1f .. %+.1f EV", it.optDouble(0), it.optDouble(1)))
        }
        appendLine("  AE fps ranges: ${c.optJSONArray("aeTargetFpsRanges").strings().joinToString()}")
        appendLine("  stabilization: video ${c.optJSONArray("videoStabilization").strings().joinToString()}; OIS ${yes(c.optBoolean("ois"))}")
        c.optJSONArray("zoomRatioRange")?.let {
            appendLine(String.format(Locale.US, "  zoom ratio %.2f .. %.2f", it.optDouble(0), it.optDouble(1)))
        }
        c.optJSONArray("dynamicRangeProfiles")?.let { appendLine("  dynamic range: ${it.strings().joinToString()}") }

        val sizes = c.optJSONArray("videoSizes")
        if (sizes != null && sizes.length() > 0) {
            appendLine("  encoder-surface sizes (max fps):")
            val items = (0 until sizes.length()).map { sizes.getJSONObject(it) }
                .map { "${it.getString("size")}@${trimFps(it.getDouble("maxFps"))}" }
            items.chunked(4).forEach { appendLine("    ${it.joinToString("  ")}") }
        }
        val highSpeed = c.optJSONArray("highSpeedVideo")
        if (highSpeed != null && highSpeed.length() > 0) {
            appendLine("  high-speed video:")
            for (i in 0 until highSpeed.length()) {
                val h = highSpeed.getJSONObject(i)
                appendLine("    ${h.getString("size")}: ${h.getJSONArray("fps").strings().joinToString()}")
            }
        }
    }

    private fun StringBuilder.encoder(e: JSONObject) {
        val kind = when {
            !e.has("hardware") -> ""
            e.optBoolean("hardware") -> ", hardware"
            else -> ", software"
        }
        appendLine("${e.getString("name")} (${e.getString("mime")}$kind), max ${e.optString("maxSize")}")
        val sizes = e.optJSONObject("sizes")
        if (sizes != null) {
            for (label in sizes.keys()) {
                val s = sizes.getJSONObject(label)
                val measured = s.optJSONArray("measuredFps")?.let { ", measured ${it.optInt(0)}-${it.optInt(1)}" } ?: ""
                appendLine("    $label: up to ${s.getInt("maxFps")} fps$measured")
            }
        }
        appendLine("    bitrate modes: ${e.optJSONArray("bitrateModes").strings().joinToString()}")
        appendLine("    profiles: ${e.optJSONArray("profiles").strings().joinToString()}")
    }

    private fun JSONArray?.strings(): List<String> =
        if (this == null) emptyList() else (0 until length()).map { get(it).toString() }

    private fun yes(b: Boolean) = if (b) "yes" else "no"

    private fun trimFps(fps: Double) =
        if (fps == Math.floor(fps)) fps.toLong().toString() else String.format(Locale.US, "%.1f", fps)

    private fun seconds(ns: Long): String = when {
        ns <= 0 -> "?"
        ns < 1_000_000_000L -> "1/" + Math.round(1e9 / ns) + " s"
        else -> String.format(Locale.US, "%.1f s", ns / 1e9)
    }
}
