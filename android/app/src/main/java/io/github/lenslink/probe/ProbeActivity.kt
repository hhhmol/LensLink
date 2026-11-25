package io.github.lenslink.probe

import android.app.Activity
import android.content.ClipData
import android.content.ClipboardManager
import android.graphics.Typeface
import android.os.Build
import android.os.Bundle
import android.util.Log
import android.view.View
import android.view.WindowInsets
import android.widget.Button
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import android.widget.Toast
import org.json.JSONObject
import java.io.File
import kotlin.concurrent.thread

/**
 * Step 1 of the roadmap: shows what this phone's cameras and encoders can do and saves
 * the report to app-specific storage, where `adb` (and `lenslink-host probe`) can read it.
 */
class ProbeActivity : Activity() {

    private lateinit var output: TextView
    private var reportText: String = ""

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val pad = (12 * resources.displayMetrics.density).toInt()
        output = TextView(this).apply {
            typeface = Typeface.MONOSPACE
            textSize = 11f
            setTextIsSelectable(true)
            text = "Collecting camera information..."
        }
        val copy = Button(this).apply {
            text = "Copy report"
            setOnClickListener { copyReport() }
        }
        val column = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(pad, pad, pad, pad)
            addView(copy)
            addView(output)
        }
        val scroll = ScrollView(this).apply { addView(column) }
        applySystemBarPadding(scroll)
        setContentView(scroll)

        thread(name = "lenslink-probe") {
            val text = try {
                collect()
            } catch (e: Exception) {
                Log.e(TAG, "probe failed", e)
                "Probe failed:\n" + Log.getStackTraceString(e)
            }
            runOnUiThread {
                reportText = text
                output.text = text
            }
        }
    }

    private fun collect(): String {
        val device = JSONObject()
            .put("manufacturer", Build.MANUFACTURER)
            .put("model", Build.MODEL)
            .put("device", Build.DEVICE)
            .put("android", Build.VERSION.RELEASE)
            .put("sdk", Build.VERSION.SDK_INT)
        if (Build.VERSION.SDK_INT >= 31) device.put("soc", "${Build.SOC_MANUFACTURER} ${Build.SOC_MODEL}")

        val report = JSONObject()
            .put("schema", 1)
            .put("device", device)
            .put("cameras", CameraProbe.run(this))
            .put("encoders", EncoderProbe.run())
        val text = ReportText.render(report)

        val dir = getExternalFilesDir(null)
        if (dir != null) {
            File(dir, "camera-report.json").writeText(report.toString(2))
            File(dir, "camera-report.txt").writeText(text)
            Log.i(TAG, "report saved to ${dir.absolutePath}")
        }
        return text
    }

    private fun copyReport() {
        if (reportText.isEmpty()) return
        getSystemService(ClipboardManager::class.java)
            .setPrimaryClip(ClipData.newPlainText("LensLink camera report", reportText))
        Toast.makeText(this, "Copied", Toast.LENGTH_SHORT).show()
    }

    /** Android 15 draws apps edge-to-edge; keep the text clear of the status and navigation bars. */
    private fun applySystemBarPadding(view: View) {
        view.setOnApplyWindowInsetsListener { v, insets ->
            if (Build.VERSION.SDK_INT >= 30) {
                val bars = insets.getInsets(WindowInsets.Type.systemBars())
                v.setPadding(bars.left, bars.top, bars.right, bars.bottom)
            } else {
                @Suppress("DEPRECATION")
                v.setPadding(insets.systemWindowInsetLeft, insets.systemWindowInsetTop,
                    insets.systemWindowInsetRight, insets.systemWindowInsetBottom)
            }
            insets
        }
    }

    private companion object {
        const val TAG = "LensLink"
    }
}
