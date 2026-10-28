// A thread's live state for the composer (SD1 Task 19): its session status, its chat
// row (cwd, harness) and its queued messages, from the daemon's watch streams
// (`ThreadService.WatchSessions`, `WatchThreads`, `QueueService.WatchQueue`). connect-query
// reads unary methods only, so each stream is one effect per transport and thread. A
// stream that fails leaves the last answer (or none): the composer then treats the
// thread as idle, and the daemon queues or refuses what does not fit.

import { createClient, type Transport } from '@connectrpc/connect';
import {
  type Chat,
  type QueuedMessage,
  QueueService,
  SessionStatus,
  ThreadService,
} from '@loams/proto/agentd';
import { useEffect, useState } from 'react';

/** Runs `watch` while mounted; it stops when the signal aborts. */
function useStream(
  transport: Transport | undefined,
  key: string,
  watch: (transport: Transport, signal: AbortSignal) => Promise<void>,
): void {
  // oxlint-disable-next-line react/exhaustive-deps -- `key` names what `watch` reads.
  useEffect(() => {
    if (!transport) return;
    const controller = new AbortController();
    watch(transport, controller.signal).catch(() => {
      // Aborted on unmount, or the daemon went away: keep the last answer.
    });
    return () => controller.abort();
  }, [transport, key]);
}

/** The thread's session status; `undefined` until the daemon answers. */
export function useSessionStatus(
  transport: Transport | undefined,
  threadId: string,
): SessionStatus | undefined {
  const [status, setStatus] = useState<SessionStatus | undefined>(undefined);
  useStream(transport, threadId, async (t, signal) => {
    const client = createClient(ThreadService, t);
    for await (const { sessions } of client.watchSessions({}, { signal })) {
      setStatus(sessions.find((s) => s.chatId === threadId)?.status ?? SessionStatus.IDLE);
    }
  });
  return status;
}

/** Whether a turn is running (or waiting on the user inside one). */
export function isBusy(status: SessionStatus | undefined): boolean {
  return status === SessionStatus.WORKING || status === SessionStatus.AWAITING_INPUT;
}

/** The thread's chat row (cwd, branch, harness); `undefined` until known. */
export function useChat(transport: Transport | undefined, threadId: string): Chat | undefined {
  const [chat, setChat] = useState<Chat | undefined>(undefined);
  useStream(transport, threadId, async (t, signal) => {
    const client = createClient(ThreadService, t);
    for await (const { chats } of client.watchThreads({}, { signal })) {
      setChat(chats.find((c) => c.id === threadId));
    }
  });
  return chat;
}

/** The thread's queued messages, in delivery order. */
export function useQueue(transport: Transport | undefined, threadId: string): QueuedMessage[] {
  const [items, setItems] = useState<QueuedMessage[]>([]);
  useStream(transport, threadId, async (t, signal) => {
    const client = createClient(QueueService, t);
    for await (const { items: next } of client.watchQueue({ chatId: threadId }, { signal })) {
      setItems(next);
    }
  });
  return items;
}
