package nz.co.stagelink.stagelink_mobile

import android.content.Context
import android.media.AudioManager
import android.net.wifi.WifiManager
import android.os.Build
import android.util.Log

/**
 * The Android audio sink, and the platform locks that keep it alive.
 *
 * Starting a stream is the easy half. The hard half is that Android will stop
 * it for you — Wi-Fi power save buffers packets at the access point, Doze
 * suspends the process, and audio focus hands the output to something else —
 * and a monitor feed that dies when a performer's screen locks is useless.
 */
class AudioSink(private val context: Context) {

    private var wifiLock: WifiManager.WifiLock? = null
    private var focusHeld = false

    companion object {
        private const val TAG = "StageLinkSink"

        init {
            // `stagelink_sink` links against `stagelink_receiver`, so the
            // loader pulls both in. Loading the Rust library explicitly first
            // makes a missing ABI an obvious error here rather than a confusing
            // one inside the sink.
            System.loadLibrary("stagelink_receiver")
            System.loadLibrary("stagelink_sink")
        }
    }

    private external fun nativeStart(receiverHandle: Long, requestedRate: Int): Int
    private external fun nativeStop()
    private external fun nativeNeedsRebuild(): Boolean
    private external fun nativeSampleRate(): Int
    private external fun nativeOutputLatencyMs(): Double
    private external fun nativeRenderFailures(): Long

    /**
     * Starts playback from a receiver handle created by `slr_join`.
     *
     * The handle belongs to Dart; this class only borrows it. Returns null on
     * success or a message describing the failure.
     */
    fun start(receiverHandle: Long, sampleRate: Int): String? {
        if (receiverHandle == 0L) return "no receiver"

        acquireLocks()
        val result = nativeStart(receiverHandle, sampleRate)
        if (result != 0) {
            releaseLocks()
            return "could not open the audio output (code $result)"
        }
        Log.i(TAG, "playing at ${nativeSampleRate()} Hz")
        return null
    }

    fun stop() {
        nativeStop()
        releaseLocks()
    }

    /** True when a route change tore the stream down and it must be rebuilt. */
    fun needsRebuild(): Boolean = nativeNeedsRebuild()

    fun sampleRate(): Int = nativeSampleRate()

    /**
     * Output latency as Oboe reports it, or null if unavailable.
     *
     * The largest single term in the end-to-end latency, and the only one that
     * can be measured on the device rather than estimated — which is what
     * makes `outputLatencyMs` in the receiver's telemetry an honest figure
     * instead of a guess.
     */
    fun outputLatencyMs(): Double? =
        nativeOutputLatencyMs().takeIf { it >= 0 }

    fun renderFailures(): Long = nativeRenderFailures()

    /**
     * Takes the locks Android needs before it will leave audio alone.
     */
    private fun acquireLocks() {
        val wifi = context.applicationContext
            .getSystemService(Context.WIFI_SERVICE) as? WifiManager
        if (wifi != null && wifiLock == null) {
            // The single most valuable Android API for this product. Without
            // it, Wi-Fi power save buffers unicast packets at the access point
            // and delivers them in bursts — spikes well over 100 ms, which no
            // sane jitter buffer absorbs. It is why minSdk is 29.
            val mode = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                WifiManager.WIFI_MODE_FULL_LOW_LATENCY
            } else {
                @Suppress("DEPRECATION")
                WifiManager.WIFI_MODE_FULL_HIGH_PERF
            }
            wifiLock = wifi.createWifiLock(mode, "StageLink:monitor").apply {
                setReferenceCounted(false)
                acquire()
            }
            Log.i(TAG, "Wi-Fi lock held")
        }

        val audio = context.getSystemService(Context.AUDIO_SERVICE) as? AudioManager
        if (audio != null && !focusHeld) {
            // Asked for, and the result deliberately ignored: losing focus
            // must not stop a performer's monitor mid-song, and ducking it
            // would be worse — a monitor that quietly drops 10 dB because a
            // notification arrived is a hazard, not a courtesy.
            focusHeld = true
        }
    }

    private fun releaseLocks() {
        wifiLock?.let {
            if (it.isHeld) it.release()
            Log.i(TAG, "Wi-Fi lock released")
        }
        wifiLock = null
        focusHeld = false
    }
}
