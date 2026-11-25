package io.github.lenslink

import android.Manifest
import android.app.Activity
import android.content.ClipData
import android.content.ClipboardManager
import android.content.pm.PackageManager
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
import io.github.lenslink.probe.CameraProbe
import io.github.lenslink.probe.EncoderProbe
import io.github.lenslink.probe.ReportText
import io.github.lenslink.stream.StreamService
import org.json.JSONObject
import java.io.File
import kotlin.concurrent.thread

/**
 * Starts / stops the streaming service and shows the camera probe report.
 * The report is also saved where `lenslink-host probe` reads it.
 */
class MainActivity : Activity() {

    private lateinit var output: TextView
    private lateinit var toggle: Button
    private var reportText: String = ""

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        val pad = (12 * resources.displayMetrics.density).toInt()
        toggle = Button(this).apply { setOnClickListener { onToggle() } }
        val copy = Button(this).apply {
            text = "Copy camera report"
            setOnClickListener { copyReport() }
        }
        output = TextView(this).apply {
            typeface = Typeface.MONOSPACE
            textSize = 11f
            setTextIsSelectable(true)
            text = "Collecting camera information..."
        }
        val column = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setPadding(pad, pad, pad, pad)
            addView(toggle)
            addView(copy)
            addView(output)
        }
        val scroll = ScrollView(this).apply { addView(column) }
        applySystemBarPadding(scroll)
        setContentView(scroll)

        thread(name = "lenslink-probe") {
            val text = try {
                collectReport()
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

    override fun onResume() {
        super.onResume()
        updateToggle()
    }

    private fun onToggle() {
        if (StreamService.running) {
            StreamService.stop(this)
            toggle.postDelayed({ updateToggle() }, 300)
            return
        }
        val missing = requiredPermissions().filter { checkSelfPermission(it) != PackageManager.PERMISSION_GRANTED }
        if (missing.isEmpty()) startStreaming() else requestPermissions(missing.toTypedArray(), REQUEST_PERMISSIONS)
    }

    override fun onRequestPermissionsResult(requestCode: Int, permissions: Array<out String>, grantResults: IntArray) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode != REQUEST_PERMISSIONS) return
        if (checkSelfPermission(Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED) {
            startStreaming()
        } else {
            Toast.makeText(this, "LensLink needs camera access to stream", Toast.LENGTH_LONG).show()
        }
    }

    private fun startStreaming() {
        StreamService.start(this)
        toggle.postDelayed({ updateToggle() }, 300)
    }

    private fun updateToggle() {
        toggle.text = if (StreamService.running) "Stop streaming service" else "Start streaming service"
    }

    private fun requiredPermissions(): List<String> {
        val list = mutableListOf(Manifest.permission.CAMERA)
        if (Build.VERSION.SDK_INT >= 33) list += Manifest.permission.POST_NOTIFICATIONS
        return list
    }

    private fun collectReport(): String {
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

        getExternalFilesDir(null)?.let { dir ->
            File(dir, "camera-report.json").writeText(report.toString(2))
            File(dir, "camera-report.txt").writeText(text)
        }
        return text
    }

    private fun copyReport() {
        if (reportText.isEmpty()) return
        getSystemService(ClipboardManager::class.java)
            .setPrimaryClip(ClipData.newPlainText("LensLink camera report", reportText))
        Toast.makeText(this, "Copied", Toast.LENGTH_SHORT).show()
    }

    /** Android 15+ draws apps edge-to-edge; keep content clear of the system bars. */
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
        const val REQUEST_PERMISSIONS = 1
    }
}
