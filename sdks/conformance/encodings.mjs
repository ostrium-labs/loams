// Encoding agreement between a recorded body and the `content-type` it is
// filed under.
//
// This is its own module so `verify-corpus.mjs` and `conformance.test.mjs` can
// share one implementation: the check is the only thing standing between a
// mislabelled recording and thirteen SDKs each rediscovering it, so it needs its
// own tests rather than living inside a script nobody can import.
//
// **It used to be wrong, and it was wrong in the direction of inventing a defect
// in a correct recording.** It judged *every* frame in a Connect streaming body
// against the codec in the `content-type`, including the end-of-stream frame. The
// Connect protocol spec defines that frame as a JSON-encoded `EndStreamResponse`
// **regardless of the codec**, so a `+proto` stream whose end frame is JSON is
// exactly what a correct server sends -- and this check called it a defect. It
// did so on `live_watch`, which was reported to the SDK owners as a broken
// recording and made `run.sh` fail for the TypeScript SDK.
//
// Replaying the recorded bytes settles it. `live_watch.json` is one empty data
// frame followed by a JSON end frame carrying `feature_not_in_variant`, filed
// under `application/connect+proto`, and connect-python reads it and raises
// `FeatureNotInVariantError` with the reason intact:
//
//     messages: 0 | error: FeatureNotInVariantError: ...Watch is not in the standard variant
//
// So the recording was always fine and the checker was not. The rule now follows
// the frame's flags instead of assuming one codec for the whole body.

/** Flags in a Connect streaming frame: bit 0 compressed, bit 1 end-of-stream. */
const FLAG_COMPRESSED = 0x01;
const FLAG_END = 0x02;

/**
 * Whether a recorded body contradicts the encoding its `content-type` claims.
 *
 * Only ever reports a *defect*, never a guess: "declared proto but a message
 * frame is parseable JSON" is unambiguous, because a serialized proto message does
 * not parse as JSON. The reverse is not asserted, since deciding that arbitrary
 * bytes are a valid `EndStreamResponse` is a parser, not a check.
 *
 * Per frame, by flag:
 *
 * - **end-of-stream** (bit 1): the payload is JSON by specification whatever the
 *   codec says, so it is checked for being JSON and never against the label.
 *   This is the case that used to produce the false positive.
 * - **compressed** (bit 0): the payload is compressed, so whether it is JSON is
 *   not answerable without decompressing it. Declines to judge, rather than
 *   reporting every compressed frame as a mismatch.
 * - **message** (otherwise): judged against the codec in the label, which is the
 *   case that genuinely catches a mislabelled recording.
 *
 * gRPC-Web and gRPC are deliberately not judged at all. Their framing carries
 * trailers in a data frame, so the byte after the 5-byte prefix is not the first
 * byte of a message and a naive read produces false alarms.
 */
export function encodingMismatch(contentType, body) {
  if (!contentType || !contentType.startsWith('application/connect')) {
    return null;
  }
  if (typeof body !== 'string' || body === '') {
    return null;
  }
  const raw = Buffer.from(body, 'base64');
  // A Connect streaming body is one or more 5-byte-prefixed frames.
  if (raw.length < 6) {
    return null;
  }
  const claimsJson = contentType.includes('+json');
  let offset = 0;
  while (offset + 5 <= raw.length) {
    const flags = raw[offset];
    const length = raw.readUInt32BE(offset + 1);
    const payload = raw.subarray(offset + 5, offset + 5 + length);
    if (payload.length !== length) {
      return null; // truncated; nothing to judge
    }
    if (flags & FLAG_COMPRESSED) {
      offset += 5 + length;
      continue; // not answerable without decompressing
    }
    let parsed = null;
    try {
      parsed = JSON.parse(payload.toString('utf8'));
    } catch {
      parsed = null;
    }
    if (flags & FLAG_END) {
      // Always JSON, whatever the label claims.
      if (parsed === null) {
        return `the end-of-stream frame is not JSON, which the protocol requires`;
      }
    } else if (claimsJson && parsed === null) {
      return `declares ${contentType} but the frame does not parse as JSON`;
    } else if (!claimsJson && parsed !== null) {
      return `declares ${contentType} but the frame is JSON`;
    }
    offset += 5 + length;
  }
  return null;
}
