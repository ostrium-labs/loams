// Lambda entry shim (design 71 §7): Function URL events (payload 2.0) to `fetch`,
// SQS batches to `queue` with partial batch failures, Scheduler input to
// `scheduled`. `ctx.waitUntil` promises are awaited before the invocation returns.
import app from "../app.mjs";
import { awsEnv } from "../adapters/aws.mjs";
import { fromResponse, toRequest } from "./lambda-http.mjs";

const env = awsEnv();

function context() {
  const pending = [];
  return { ctx: { waitUntil: (p) => pending.push(p), passThroughOnException() {} }, settle: () => Promise.allSettled(pending) };
}

export async function handler(event) {
  const { ctx, settle } = context();
  try {
    if (Array.isArray(event.Records) && event.Records[0]?.eventSource === "aws:sqs") {
      const failures = [];
      const messages = event.Records.map((r) => ({
        id: r.messageId,
        body: JSON.parse(r.body),
        attempts: Number(r.attributes?.ApproximateReceiveCount ?? 1),
        ack() {},
        retry() {
          failures.push({ itemIdentifier: r.messageId });
        },
      }));
      await app.queue({ queue: "bench", messages }, env, ctx);
      return { batchItemFailures: failures };
    }
    if (event["loams-scheduled"]) {
      if (app.scheduled) await app.scheduled({ cron: event["loams-scheduled"].cron, scheduledTime: Date.now() }, env, ctx);
      return { ok: true };
    }
    return await fromResponse(await app.fetch(toRequest(event), env, ctx));
  } finally {
    await settle();
  }
}
