// Encoding agreement between a recorded body and the `content-type` it is
// filed under.
//
// This is its own module so `verify-corpus.mjs` and `conformance.test.mjs` can
// share one implementation: the check is the only thing standing between a
// mislabelled recording and thirteen SDKs each rediscovering it, so it needs its
// own tests rather than living inside a script nobody can import.

/**
 * Whether a recorded body matches the encoding its `content-type` claims.
 *
 * Only ever reports a *defect*, never a guess: "declared proto but the payload
 * is parseable JSON" is unambiguous, because a serialized proto message does not
 * parse as JSON. The reverse is not asserted, since deciding that arbitrary
 * bytes are a valid `EndStreamResponse` is a parser, not a check.
 *
 * gRPC-Web and gRPC are deliberately not judged here. Their framing carries
 * trailers in a data frame, so the byte after the 5-byte prefix is not the first
 * byte of a message and a naive read produces false alarms.
 *
 * This check exists because `live_watch` shipped with a JSON end-of-stream body
 * labelled `application/connect+proto`. Every status and frame-count expectation
 * passed, so the corpus read as healthy, and the first SDK to replay it got an
 * `InternalError` with no reason instead of the `feature_not_in_variant` the
 * recording was meant to pin.
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
  let offset = 0;
  while (offset + 5 <= raw.length) {
    const length = raw.readUInt32BE(offset + 1);
    const payload = raw.subarray(offset + 5, offset + 5 + length);
    if (payload.length !== length) {
      return null; // truncated; nothing to judge
    }
    let parsed = null;
    try {
      parsed = JSON.parse(payload.toString('utf8'));
    } catch {
      parsed = null;
    }
    const claimsJson = contentType.includes('+json');
    if (claimsJson && parsed === null) {
      return `declares ${contentType} but the frame does not parse as JSON`;
    }
    if (!claimsJson && parsed !== null) {
      return `declares ${contentType} but the frame is JSON`;
    }
    offset += 5 + length;
  }
  return null;
}

