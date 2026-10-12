package nz.co.stagelink.stagelink_mobile

import android.os.Bundle
import android.view.WindowManager
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

/**
 * The host activity, and the bridge to the audio sink.
 *
 * The channel carries five calls and no audio. Dart starts and stops the
 * stream and asks for the measured output latency; the samples themselves go
 * from Rust to Oboe without passing through the JVM at all.
 */
class MainActivity : FlutterActivity() {

    private lateinit var sink: AudioSink

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // The screen stays on while the app is in front: a phone that sleeps
        // mid-song is one a performer cannot mute without unlocking it first.
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
    }

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        sink = AudioSink(applicationContext)

        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, CHANNEL)
            .setMethodCallHandler { call, result ->
                when (call.method) {
                    "start" -> {
                        // The handle is a pointer Dart got from `slr_join`.
                        // Passed as a long because that is what crosses JNI;
                        // ownership stays with Dart.
                        val handle = call.argument<Any>("handle")
                        val address = when (handle) {
                            is Long -> handle
                            is Int -> handle.toLong()
                            else -> 0L
                        }
                        val rate = call.argument<Int>("sampleRate") ?: 48000
                        val failure = sink.start(address, rate)
                        if (failure == null) {
                            result.success(sink.sampleRate())
                        } else {
                            result.error("audio_output", failure, null)
                        }
                    }

                    "stop" -> {
                        sink.stop()
                        result.success(null)
                    }

                    "needsRebuild" -> result.success(sink.needsRebuild())

                    // Null rather than zero when unavailable: a receiver that
                    // cannot measure its output latency must say so, and
                    // reporting zero would make the telemetry a lie.
                    "outputLatencyMs" -> result.success(sink.outputLatencyMs())

                    "renderFailures" -> result.success(sink.renderFailures())

                    else -> result.notImplemented()
                }
            }
    }

    override fun onDestroy() {
        if (::sink.isInitialized) sink.stop()
        super.onDestroy()
    }

    companion object {
        const val CHANNEL = "nz.co.stagelink/audio_sink"
    }
}
