package io.github.lenslink.stream

import org.json.JSONObject

/**
 * Camera settings the PC can change while streaming. `null` means automatic.
 * Kept by the session, so they survive a mode switch.
 */
data class Controls(
    /** Logical-camera zoom ratio; below 1 selects the ultrawide lens on multi-camera phones. */
    val zoom: Float? = null,
    /** Manual focus distance in diopters (1/m, 0 = infinity); null = continuous autofocus. */
    val focusDiopters: Float? = null,
    /** Manual exposure time; with [iso], turns auto-exposure off. The missing one keeps its last auto value. */
    val exposureNs: Long? = null,
    val iso: Int? = null,
    /** Auto-exposure compensation in EV; rounded to the camera's step. */
    val ev: Float = 0f,
    /** White balance as a colour temperature; mapped to the nearest preset. null = auto. */
    val awbKelvin: Int? = null,
    /** "auto", "50hz", "60hz" or "off". */
    val antibanding: String = "auto",
    val torch: Boolean = false,
) {
    /** Applies the keys present in a `set` message; JSON null resets a key to automatic. */
    fun merge(j: JSONObject): Controls {
        var c = this
        if (j.has("zoom")) c = c.copy(zoom = j.nullableDouble("zoom")?.toFloat())
        if (j.has("focus_diopters")) c = c.copy(focusDiopters = j.nullableDouble("focus_diopters")?.toFloat())
        if (j.has("exposure_ns")) c = c.copy(exposureNs = j.nullableDouble("exposure_ns")?.toLong())
        if (j.has("iso")) c = c.copy(iso = j.nullableDouble("iso")?.toInt())
        if (j.has("ev")) c = c.copy(ev = j.nullableDouble("ev")?.toFloat() ?: 0f)
        if (j.has("awb_kelvin")) c = c.copy(awbKelvin = j.nullableDouble("awb_kelvin")?.toInt())
        if (j.has("antibanding")) c = c.copy(antibanding = j.optString("antibanding", "auto"))
        if (j.has("torch")) c = c.copy(torch = j.optBoolean("torch", false))
        return c
    }

    private fun JSONObject.nullableDouble(key: String): Double? =
        if (isNull(key)) null else optDouble(key).takeIf { !it.isNaN() }
}
