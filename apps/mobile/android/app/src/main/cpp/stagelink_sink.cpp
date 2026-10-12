// The Android audio sink.
//
// An Oboe stream whose callback calls `slr_render_stereo` and does nothing
// else. Deliberately the smallest piece in the system: it is the only code
// that has to be written once per platform, so every decision that can be
// made in shared Rust is made there instead.
//
// ## Rules this file obeys, and why
//
// * **No allocation in the callback.** Not a style point — a malloc that hits
//   a slow path misses the deadline, and a missed deadline is a gap in
//   somebody's monitor feed.
// * **No JNI calls in the callback.** Attaching to the JVM, or calling up into
//   Kotlin, can block. The callback talks to Rust and to nothing else.
// * **No thread-locals and no cached thread id.** Oboe destroys the stream and
//   creates a *new callback thread* on a route change — plugging in earphones
//   — so anything remembered about "the audio thread" is wrong afterwards.
// * **Stereo out, from a mono mix.** `slr_render_stereo` duplicates the mono
//   feed to both ears. The desktop sends one pre-mixed mono signal, and a
//   stereo image invented from it would place instruments where they are not.

#include <atomic>
#include <memory>
#include <mutex>

#include <android/log.h>
#include <jni.h>
#include <oboe/Oboe.h>

#include "stagelink_receiver.h"

#define LOG_TAG "StageLinkSink"
#define LOGI(...) __android_log_print(ANDROID_LOG_INFO, LOG_TAG, __VA_ARGS__)
#define LOGW(...) __android_log_print(ANDROID_LOG_WARN, LOG_TAG, __VA_ARGS__)

namespace {

class Sink : public oboe::AudioStreamDataCallback,
             public oboe::AudioStreamErrorCallback {
 public:
  /*
   * `receiver` is the handle from `slr_join`, passed down as an opaque
   * pointer. This class never owns it: Dart created it and Dart destroys it,
   * and a sink that freed it would leave Dart holding a dangling handle.
   */
  explicit Sink(SlrReceiver *receiver) : receiver_(receiver) {}

  oboe::DataCallbackResult onAudioReady(oboe::AudioStream *stream,
                                        void *audio_data,
                                        int32_t num_frames) override {
    auto *out = static_cast<float *>(audio_data);

    /*
     * One call, and the only one. Returns the frame count, or a negative
     * status — and on a negative status it has already written silence, so
     * there is nothing to clean up here. Silence is the right answer for a
     * failure: a callback that returns nothing produces a click, and a gap is
     * merely a gap.
     */
    const int32_t rendered = slr_render_stereo(receiver_, out, num_frames);
    if (rendered < 0) {
      // Counted rather than logged. Logging from the audio callback can block
      // on the log socket, which is the one thing this thread must not do.
      render_failures_.fetch_add(1, std::memory_order_relaxed);
    }

    return oboe::DataCallbackResult::Continue;
  }

  /*
   * A route change — earphones in or out, or a Bluetooth device appearing.
   * Oboe tears the stream down and will not reuse the thread, so the stream is
   * rebuilt from scratch on a *different* thread rather than restarted.
   */
  void onErrorAfterClose(oboe::AudioStream *stream,
                         oboe::Result result) override {
    LOGW("stream closed: %s; rebuilding", oboe::convertToText(result));
    needs_rebuild_.store(true, std::memory_order_release);
  }

  bool needsRebuild() const {
    return needs_rebuild_.load(std::memory_order_acquire);
  }

  void rebuildHandled() {
    needs_rebuild_.store(false, std::memory_order_release);
  }

  int64_t renderFailures() const {
    return render_failures_.load(std::memory_order_relaxed);
  }

 private:
  SlrReceiver *receiver_;
  std::atomic<int64_t> render_failures_{0};
  std::atomic<bool> needs_rebuild_{false};
};

/*
 * One stream per process. A phone has one pair of ears, and a second stream
 * would be two things fighting for the same output.
 *
 * Guarded by a mutex taken only from the JNI (app) thread. The audio callback
 * never touches it.
 */
std::mutex g_mutex;
std::shared_ptr<oboe::AudioStream> g_stream;
std::unique_ptr<Sink> g_sink;

oboe::Result openStream(SlrReceiver *receiver, int32_t requested_rate) {
  g_sink = std::make_unique<Sink>(receiver);

  oboe::AudioStreamBuilder builder;
  builder.setDirection(oboe::Direction::Output)
      /*
       * LowLatency and Exclusive are the whole point: they are what gets the
       * MMAP fast path and a small burst size on hardware that has them.
       * Oboe falls back on its own where they are not available, which is the
       * reason for using Oboe rather than AAudio directly.
       */
      ->setPerformanceMode(oboe::PerformanceMode::LowLatency)
      ->setSharingMode(oboe::SharingMode::Exclusive)
      ->setFormat(oboe::AudioFormat::Float)
      ->setChannelCount(oboe::ChannelCount::Stereo)
      ->setSampleRate(requested_rate)
      /*
       * Media, not VoiceCommunication. VoiceCommunication routes to the
       * earpiece on some devices, applies AEC and AGC that mangle a monitor
       * mix, and is the wrong usage for music.
       */
      ->setUsage(oboe::Usage::Media)
      ->setContentType(oboe::ContentType::Music)
      ->setDataCallback(g_sink.get())
      ->setErrorCallback(g_sink.get());

  const oboe::Result result = builder.openStream(g_stream);
  if (result != oboe::Result::OK) {
    LOGW("could not open the audio stream: %s", oboe::convertToText(result));
    g_sink.reset();
    return result;
  }

  LOGI("stream open: %d Hz, %d frames per burst, %s path",
       g_stream->getSampleRate(), g_stream->getFramesPerBurst(),
       g_stream->getPerformanceMode() == oboe::PerformanceMode::LowLatency
           ? "low-latency"
           : "normal");

  /*
   * Twice the burst, which is Oboe's own guidance: one burst is the hardware
   * minimum and leaves nothing for a late wake-up, and more than two adds
   * latency the jitter buffer is already handling better.
   */
  g_stream->setBufferSizeInFrames(g_stream->getFramesPerBurst() * 2);
  return g_stream->requestStart();
}

}  // namespace

extern "C" {

/*
 * Starts playback.
 *
 * `receiver_handle` is the `SlrReceiver*` from `slr_join`, passed as a long
 * because that is what crosses JNI. Returns 0 on success, or Oboe's error code
 * negated so the Kotlin side can tell failures apart.
 */
JNIEXPORT jint JNICALL
Java_nz_co_stagelink_stagelink_1mobile_AudioSink_nativeStart(
    JNIEnv *env, jobject /* this */, jlong receiver_handle,
    jint requested_rate) {
  std::lock_guard<std::mutex> lock(g_mutex);

  if (receiver_handle == 0) {
    LOGW("refusing to start with a null receiver handle");
    return -1;
  }
  if (g_stream != nullptr) {
    LOGI("already playing");
    return 0;
  }

  auto *receiver = reinterpret_cast<SlrReceiver *>(receiver_handle);
  const oboe::Result result = openStream(receiver, requested_rate);
  if (result != oboe::Result::OK) {
    g_stream.reset();
    return static_cast<jint>(result);
  }
  return 0;
}

/* Stops playback and releases the stream. Safe to call when not playing. */
JNIEXPORT void JNICALL
Java_nz_co_stagelink_stagelink_1mobile_AudioSink_nativeStop(
    JNIEnv *env, jobject /* this */) {
  std::lock_guard<std::mutex> lock(g_mutex);
  if (g_stream == nullptr) return;

  g_stream->requestStop();
  g_stream->close();
  g_stream.reset();
  /*
   * The sink is destroyed after the stream, never before: Oboe may still be
   * inside the callback when `close` is called, and freeing the callback
   * object first would be a use-after-free in somebody's ears.
   */
  g_sink.reset();
  LOGI("stream closed");
}

/*
 * Whether a route change tore the stream down.
 *
 * Polled by Kotlin rather than pushed, because pushing would mean a JNI call
 * up from the error callback and the whole point is that the audio side never
 * calls into the JVM.
 */
JNIEXPORT jboolean JNICALL
Java_nz_co_stagelink_stagelink_1mobile_AudioSink_nativeNeedsRebuild(
    JNIEnv *env, jobject /* this */) {
  std::lock_guard<std::mutex> lock(g_mutex);
  return g_sink != nullptr && g_sink->needsRebuild() ? JNI_TRUE : JNI_FALSE;
}

/* The sample rate Oboe actually gave us, or 0 if not playing. */
JNIEXPORT jint JNICALL
Java_nz_co_stagelink_stagelink_1mobile_AudioSink_nativeSampleRate(
    JNIEnv *env, jobject /* this */) {
  std::lock_guard<std::mutex> lock(g_mutex);
  return g_stream == nullptr ? 0 : g_stream->getSampleRate();
}

/*
 * The output latency Oboe reports, in milliseconds, or -1 if unavailable.
 *
 * This is the figure that makes the receiver's telemetry honest: it is the
 * largest term in the end-to-end latency and the only one that can be
 * measured from the device rather than estimated.
 */
JNIEXPORT jdouble JNICALL
Java_nz_co_stagelink_stagelink_1mobile_AudioSink_nativeOutputLatencyMs(
    JNIEnv *env, jobject /* this */) {
  std::lock_guard<std::mutex> lock(g_mutex);
  if (g_stream == nullptr) return -1.0;
  const auto latency = g_stream->calculateLatencyMillis();
  return latency ? latency.value() : -1.0;
}

/* Render failures since the stream opened. Should always be zero. */
JNIEXPORT jlong JNICALL
Java_nz_co_stagelink_stagelink_1mobile_AudioSink_nativeRenderFailures(
    JNIEnv *env, jobject /* this */) {
  std::lock_guard<std::mutex> lock(g_mutex);
  return g_sink == nullptr ? 0 : g_sink->renderFailures();
}

}  // extern "C"
