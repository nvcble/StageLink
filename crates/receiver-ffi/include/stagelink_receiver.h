/*
 * StageLink receiver — C ABI.
 *
 * Hand-written rather than generated. `cbindgen` would produce most of it, but
 * the parts that matter here are the thread rules and the ownership rules, and
 * a generator cannot write those — it would emit correct signatures with none
 * of the information a caller actually needs in order not to cause a dropout
 * in somebody's ears.
 *
 * Keep in step with `crates/receiver-ffi/src/`. `slr_abi_version()` exists so
 * a mismatch is caught at load rather than read as nonsense.
 *
 * ## What crosses this boundary
 *
 * Join, leave, the performer's controls, a stats struct polled at about 10 Hz,
 * and the render callback. **Nothing per packet.** The network thread and the
 * audio callback both live in Rust; the caller never sees a datagram. No
 * credential ever comes back out: the caller passes the scanned QR payload in
 * and receives an opaque handle.
 *
 * ## Thread rules
 *
 * `slr_render` and `slr_render_stereo` are called from the audio callback and
 * from nowhere else. They never allocate, never take a blocking lock and never
 * call back into the caller. Every other function is called from any other
 * thread.
 *
 * Breaking that rule does not corrupt anything — the render path uses a
 * non-blocking lock — but it will produce silence, because the safe answer
 * when the audio thread cannot proceed is a gap rather than a click.
 */

#ifndef STAGELINK_RECEIVER_H
#define STAGELINK_RECEIVER_H

#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* ------------------------------------------------------------------ status */

/*
 * Returned by every call. Negative on failure, so the render functions can
 * return either a frame count or a status in one value.
 *
 * The codes are distinct where the *instruction to a person* differs, not
 * where the internal cause does: "rescan the code", "update the app" and
 * "check the network" are three different things somebody has to do.
 */
typedef enum {
  SLR_OK = 0,
  /* A null pointer, a zero length, or a string that was not UTF-8. */
  SLR_INVALID_ARGUMENT = -1,
  /* Not a usable StageLink join code. Tell the user to rescan. */
  SLR_BAD_JOIN_CODE = -2,
  /* Different major protocol version. Rescanning will not help. */
  SLR_INCOMPATIBLE_VERSION = -3,
  /* The desktop did not answer. Same network? */
  SLR_COULD_NOT_CONNECT = -4,
  /* The desktop refused the code. Ask the operator for a fresh one. */
  SLR_REFUSED = -5,
  /* The audio port could not be opened. */
  SLR_AUDIO_PORT_UNAVAILABLE = -6,
  SLR_INTERNAL = -7,
  /* A panic was caught at the boundary. Always a bug in the library. */
  SLR_PANICKED = -8
} SlrStatus;

/* ------------------------------------------------------------------- types */

typedef struct SlrReceiver SlrReceiver;

/* What the app tells the library about itself when joining. */
typedef struct {
  /* Shown in the operator's roster, e.g. "Pixel 8". Borrowed for the call. */
  const char *device_name;
  /* One of "android", "ios", "windows", "macos", "linux". */
  const char *platform;
  const char *platform_version; /* may be NULL */
  const char *app_version;
  /*
   * Long-term public key, hex. Not a secret: it is how the desktop recognises
   * this device across reconnects, so the operator's label and any revocation
   * stick to it. Must be stable for the lifetime of the installation.
   */
  const char *public_key;
  /* 0 tight (20 ms), 1 normal (40 ms), 2 safe (80 ms). */
  uint32_t preset;
  /*
   * Bind an OS-chosen audio port instead of the one in the QR code. Wrong on
   * a phone, where the QR's port is expected; needed when several receivers
   * share one address.
   */
  bool ephemeral_audio_port;
} SlrConfig;

/* Three-band EQ, as the performer set it. */
typedef struct {
  float low_hz;
  float low_db;
  float low_q;
  float mid_hz;
  float mid_db;
  float mid_q;
  float high_hz;
  float high_db;
  float high_q;
} SlrEq;

/*
 * Everything the UI needs, in one poll. A struct copy rather than a call per
 * field, because drawing one screen should not cost nine FFI calls.
 *
 * Every field is measured. None is estimated or invented, and a figure that
 * cannot be measured is reported as such rather than as zero.
 */
typedef struct {
  /* 0 disconnected, 1 connecting, 2 connected, 3 failed. */
  uint32_t connection_state;
  /*
   * True once the desktop has sent audio parameters. Before that, silence is
   * expected rather than a fault — the operator has not started the stream.
   * Showing "no audio" to a performer before the band has started would be
   * wrong.
   */
  bool has_stream;
  bool playing;

  uint64_t packets_received;
  uint64_t packets_lost;
  float loss_percent;
  uint64_t duplicates;
  uint64_t reordered;
  /* Datagrams that were not valid StageLink packets. */
  uint64_t rejected;
  /*
   * Encrypted packets this receiver had no key for. A few at the start of a
   * stream are normal; a figure that keeps climbing while nothing is heard is
   * the one fault that looks like perfect health from the desktop.
   */
  uint64_t keyless;
  /* Datagrams from an address other than the desktop we joined. */
  uint64_t foreign_datagrams;

  /* Jitter-buffer depth, in milliseconds. */
  /* Peak of the arriving mix, linear: 1.0 full scale, 0.0 silence. */
  float arrival_peak;
  float buffer_ms;
  /*
   * Separate gaps concealed. More telling than a sample count: fifty short
   * gaps and one long one sound nothing alike.
   */
  uint64_t concealed_runs;
  float concealed_ms;
  uint64_t resyncs;
  /* Clock difference the drift controller settled on, in ppm. */
  float drift_ppm;
  /*
   * Samples the safety clip caught. Should always be zero; non-zero means the
   * limiter is misbehaving and is worth reporting as a fault.
   */
  uint64_t clipped;

  uint64_t heartbeats;
  uint64_t missed_heartbeats;
  /*
   * Control-channel round-trip time, or -1 if none has been measured.
   *
   * NOT audio latency. It is the control path, over TCP. Do not label it as
   * latency in the UI: a performer reading "4 ms" would conclude their
   * monitor is 4 ms behind, and it is not.
   */
  float control_rtt_ms;
} SlrStats;

/* --------------------------------------------------------------- functions */

/*
 * ABI version this library implements. Check it against the value the app was
 * built against before calling anything else: a stale .so would otherwise
 * read the wrong bytes out of these structs, and the symptom would be
 * nonsense rather than an error.
 */
uint32_t slr_abi_version(void);

/* Initialises logging to stderr. Optional; safe to call more than once. */
void slr_init_logging(void);

/*
 * Joins a desktop from a scanned QR payload.
 *
 * Blocks until the handshake completes or fails — a second or two at worst —
 * so call it off the UI thread. On success writes a handle to *out and
 * returns SLR_OK. On failure leaves *out NULL and returns why.
 *
 * The handle must eventually be passed to slr_leave().
 */
SlrStatus slr_join(const char *qr, const SlrConfig *config, SlrReceiver **out);

/*
 * Leaves and frees the receiver.
 *
 * Tells the desktop at once, so the roster entry goes immediately rather than
 * after five missed heartbeats. Passing NULL is allowed and does nothing.
 */
void slr_leave(SlrReceiver *receiver);

/*
 * Renders mono frames. THE AUDIO CALLBACK ONLY.
 *
 * Returns the number of frames written, or a negative SlrStatus. Writes
 * silence and returns the frame count if the render path is momentarily
 * unavailable, because a callback that returns nothing is a click and silence
 * is merely a gap.
 *
 * `out` must point to `frames` writable floats.
 */
int32_t slr_render(SlrReceiver *receiver, float *out, uint32_t frames);

/*
 * Renders interleaved stereo by duplicating the mono mix to both ears.
 *
 * `frames` is frames, not samples: `out` must hold frames * 2 floats.
 *
 * Both ears receive the same signal. The desktop sends one pre-mixed mono
 * feed, and a stereo image invented from it would place instruments where
 * they are not.
 */
int32_t slr_render_stereo(SlrReceiver *receiver, float *out, uint32_t frames);

/*
 * Sets the performer's volume in decibels.
 *
 * Clamped internally to a range that cannot be a hazard. A value outside it
 * is applied as the nearest allowed one rather than refused.
 */
SlrStatus slr_set_gain_db(SlrReceiver *receiver, float gain_db);

/* Mutes or unmutes. Ramped over 5 ms, so it does not click. */
SlrStatus slr_set_muted(SlrReceiver *receiver, bool muted);

/* Sets the three-band EQ. Every parameter is clamped internally. */
SlrStatus slr_set_eq(SlrReceiver *receiver, const SlrEq *eq);

/*
 * Sets the hearing-safety profile: 0 protected (-6 dBFS), 1 standard
 * (-3 dBFS), 2 unrestricted (0 dBFS).
 *
 * The one coarse, auditable call that can raise the output ceiling. An
 * unknown value is treated as protected — the safe direction — rather than
 * refused, because a refused call leaves whatever was in force and the caller
 * may not check the result.
 */
SlrStatus slr_set_safety_profile(SlrReceiver *receiver, uint32_t profile);

/* Sets the buffer preset: 0 tight, 1 normal, 2 safe. */
SlrStatus slr_set_preset(SlrReceiver *receiver, uint32_t preset);

/*
 * Fills *out with current statistics, and services the control channel.
 *
 * Call this on a timer at about 10 Hz. It is also what keeps the telemetry
 * the desktop sees up to date, so an app that stops polling stops reporting.
 */
SlrStatus slr_stats(SlrReceiver *receiver, SlrStats *out);

#ifdef __cplusplus
} /* extern "C" */
#endif

#endif /* STAGELINK_RECEIVER_H */
