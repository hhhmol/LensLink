package io.github.lenslink.stream

import android.annotation.SuppressLint
import android.content.Context
import android.hardware.camera2.CameraCaptureSession
import android.hardware.camera2.CameraCharacteristics
import android.hardware.camera2.CameraDevice
import android.hardware.camera2.CameraManager
import android.hardware.camera2.CameraMetadata
import android.hardware.camera2.CaptureRequest
import android.hardware.camera2.params.OutputConfiguration
import android.hardware.camera2.params.SessionConfiguration
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaFormat
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.util.Log
import android.util.Range
import android.view.Surface
import java.util.concurrent.Executor

/**
 * Camera2 → MediaCodec input surface → packets. Everything runs on one camera thread,
 * so camera, codec and socket writes never race each other.
 */
class CameraStreamer(
    context: Context,
    private val config: StreamConfig,
    private val writer: PacketWriter,
    private val onFatal: (Throwable) -> Unit,
) {
    private val manager = context.getSystemService(CameraManager::class.java)
    private val thread = HandlerThread("lenslink-camera").apply { start() }
    private val handler = Handler(thread.looper)
    private val executor = Executor { handler.post(it) }

    private var codec: MediaCodec? = null
    private var inputSurface: Surface? = null
    private var camera: CameraDevice? = null
    private var session: CameraCaptureSession? = null
    private var buffer = ByteArray(1 shl 20)

    @Volatile
    private var stopped = false

    fun start() {
        handler.post {
            try {
                open()
            } catch (e: Exception) {
                fail(e)
            }
        }
    }

    /** Idempotent; safe from any thread. */
    fun stop() {
        if (stopped) return
        stopped = true
        handler.post {
            runCatching { session?.close() }
            runCatching { camera?.close() }
            runCatching { codec?.stop() }
            runCatching { codec?.release() }
            runCatching { inputSurface?.release() }
            thread.quitSafely()
            Log.i(TAG, "stream stopped")
        }
    }

    @SuppressLint("MissingPermission") // checked by the activity before the service starts
    private fun open() {
        val ch = manager.getCameraCharacteristics(config.cameraId)
        validate(ch)

        val format = MediaFormat.createVideoFormat(config.mime, config.width, config.height).apply {
            setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatSurface)
            setInteger(MediaFormat.KEY_BIT_RATE, config.bitrate)
            setInteger(MediaFormat.KEY_FRAME_RATE, config.fps)
            setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, config.keyframeIntervalSec)
            setInteger(MediaFormat.KEY_PRIORITY, 0) // real time
            if (Build.VERSION.SDK_INT >= 30) setInteger(MediaFormat.KEY_LATENCY, 1)
            // B-frames make the decoder wait for later frames; a webcam needs every frame shown at once.
            if (Build.VERSION.SDK_INT >= 29) setInteger(MediaFormat.KEY_MAX_B_FRAMES, 0)
        }
        val c = MediaCodec.createEncoderByType(config.mime)
        val encoderCaps = c.codecInfo.getCapabilitiesForType(config.mime).encoderCapabilities
        if (encoderCaps.isBitrateModeSupported(MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR)) {
            format.setInteger(MediaFormat.KEY_BITRATE_MODE, MediaCodecInfo.EncoderCapabilities.BITRATE_MODE_CBR)
        }
        c.setCallback(codecCallback, handler)
        c.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE)
        inputSurface = c.createInputSurface()
        c.start()
        codec = c
        Log.i(TAG, "encoder ${c.name}: ${config.width}x${config.height}@${config.fps} ${config.codec} ${config.bitrate / 1000} kbit/s")

        manager.openCamera(config.cameraId, cameraCallback, handler)
    }

    private fun validate(ch: CameraCharacteristics) {
        val map = ch.get(CameraCharacteristics.SCALER_STREAM_CONFIGURATION_MAP)
            ?: throw IllegalArgumentException("camera ${config.cameraId} has no stream configuration")
        val sizes = map.getOutputSizes(MediaCodec::class.java).orEmpty()
        val size = sizes.firstOrNull { it.width == config.width && it.height == config.height }
            ?: throw IllegalArgumentException(
                "camera ${config.cameraId} cannot output ${config.width}x${config.height}; " +
                    "supported: " + sizes.joinToString { "${it.width}x${it.height}" })
        val minFrameNs = map.getOutputMinFrameDuration(MediaCodec::class.java, size)
        if (minFrameNs > 0) {
            val maxFps = 1_000_000_000.0 / minFrameNs
            if (config.fps > maxFps + 0.5) {
                throw IllegalArgumentException(
                    "camera ${config.cameraId} gives at most ${"%.0f".format(maxFps)} fps at ${config.width}x${config.height}")
            }
        }
    }

    private val cameraCallback = object : CameraDevice.StateCallback() {
        override fun onOpened(device: CameraDevice) {
            if (stopped) {
                device.close()
                return
            }
            camera = device
            try {
                createSession(device)
            } catch (e: Exception) {
                fail(e)
            }
        }

        override fun onDisconnected(device: CameraDevice) {
            device.close()
            fail(IllegalStateException("camera disconnected (taken by another app?)"))
        }

        override fun onError(device: CameraDevice, error: Int) {
            device.close()
            fail(IllegalStateException("camera error $error"))
        }
    }

    private fun createSession(device: CameraDevice) {
        val surface = inputSurface ?: return
        val callback = object : CameraCaptureSession.StateCallback() {
            override fun onConfigured(s: CameraCaptureSession) {
                if (stopped) return
                session = s
                try {
                    s.setRepeatingRequest(buildRequest(device, surface), null, handler)
                    Log.i(TAG, "camera ${config.cameraId} streaming")
                } catch (e: Exception) {
                    fail(e)
                }
            }

            override fun onConfigureFailed(s: CameraCaptureSession) {
                fail(IllegalStateException("camera rejected the session configuration"))
            }
        }
        if (Build.VERSION.SDK_INT >= 28) {
            device.createCaptureSession(SessionConfiguration(
                SessionConfiguration.SESSION_REGULAR, listOf(OutputConfiguration(surface)), executor, callback))
        } else {
            @Suppress("DEPRECATION")
            device.createCaptureSession(listOf(surface), callback, handler)
        }
    }

    private fun buildRequest(device: CameraDevice, surface: Surface): CaptureRequest {
        val ch = manager.getCameraCharacteristics(config.cameraId)
        return device.createCaptureRequest(CameraDevice.TEMPLATE_RECORD).apply {
            addTarget(surface)
            set(CaptureRequest.CONTROL_MODE, CameraMetadata.CONTROL_MODE_AUTO)
            set(CaptureRequest.CONTROL_AE_TARGET_FPS_RANGE, pickFpsRange(ch))

            val afModes = ch.get(CameraCharacteristics.CONTROL_AF_AVAILABLE_MODES)
            if (afModes?.contains(CameraMetadata.CONTROL_AF_MODE_CONTINUOUS_VIDEO) == true) {
                set(CaptureRequest.CONTROL_AF_MODE, CameraMetadata.CONTROL_AF_MODE_CONTINUOUS_VIDEO)
            }

            if (config.stabilization) {
                val eis = ch.get(CameraCharacteristics.CONTROL_AVAILABLE_VIDEO_STABILIZATION_MODES)
                if (eis?.contains(CameraMetadata.CONTROL_VIDEO_STABILIZATION_MODE_ON) == true) {
                    set(CaptureRequest.CONTROL_VIDEO_STABILIZATION_MODE, CameraMetadata.CONTROL_VIDEO_STABILIZATION_MODE_ON)
                }
                val ois = ch.get(CameraCharacteristics.LENS_INFO_AVAILABLE_OPTICAL_STABILIZATION)
                if (ois?.contains(CameraMetadata.LENS_OPTICAL_STABILIZATION_MODE_ON) == true) {
                    set(CaptureRequest.LENS_OPTICAL_STABILIZATION_MODE, CameraMetadata.LENS_OPTICAL_STABILIZATION_MODE_ON)
                }
            }

            val zoom = config.zoom
            if (zoom != null && Build.VERSION.SDK_INT >= 30) {
                val range = ch.get(CameraCharacteristics.CONTROL_ZOOM_RATIO_RANGE)
                set(CaptureRequest.CONTROL_ZOOM_RATIO, range?.clamp(zoom) ?: zoom)
            }
        }.build()
    }

    /** Prefer a fixed [fps, fps] range so the encoder gets a steady frame rate. */
    private fun pickFpsRange(ch: CameraCharacteristics): Range<Int> {
        val ranges = ch.get(CameraCharacteristics.CONTROL_AE_AVAILABLE_TARGET_FPS_RANGES).orEmpty()
        return ranges.firstOrNull { it.lower == config.fps && it.upper == config.fps }
            ?: ranges.filter { it.upper == config.fps }.maxByOrNull { it.lower }
            ?: Range(config.fps, config.fps)
    }

    private val codecCallback = object : MediaCodec.Callback() {
        override fun onInputBufferAvailable(c: MediaCodec, index: Int) = Unit // surface input

        override fun onOutputBufferAvailable(c: MediaCodec, index: Int, info: MediaCodec.BufferInfo) {
            if (stopped) return
            try {
                val out = c.getOutputBuffer(index)
                if (out != null && info.size > 0) {
                    if (buffer.size < info.size) buffer = ByteArray(info.size * 2)
                    out.position(info.offset)
                    out.get(buffer, 0, info.size)

                    var flags = 0
                    if (info.flags and MediaCodec.BUFFER_FLAG_CODEC_CONFIG != 0) flags = flags or Protocol.FLAG_CONFIG
                    if (info.flags and MediaCodec.BUFFER_FLAG_KEY_FRAME != 0) flags = flags or Protocol.FLAG_KEYFRAME
                    val pts = if (flags and Protocol.FLAG_CONFIG != 0) 0L else info.presentationTimeUs
                    writer.packet(Protocol.KIND_VIDEO, flags, pts, buffer, 0, info.size)
                }
                c.releaseOutputBuffer(index, false)
            } catch (e: Exception) {
                fail(e)
            }
        }

        override fun onError(c: MediaCodec, e: MediaCodec.CodecException) = fail(e)

        override fun onOutputFormatChanged(c: MediaCodec, format: MediaFormat) {
            Log.i(TAG, "encoder output format: $format")
        }
    }

    private fun fail(e: Throwable) {
        if (stopped) return
        Log.e(TAG, "stream failed", e)
        onFatal(e)
        stop()
    }

    private companion object {
        const val TAG = "LensLink"
    }
}
