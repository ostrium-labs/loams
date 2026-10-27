// The thread page's server streams (SD1 Task 18). connect-query reads unary methods;
// these hooks hold the long-lived watches: the transcript through the shared store
// (`@loams/studio-ui/chat`, one watch per thread for the page and the dock, D917),
// the thread's own record from `WatchThreads`, and its pending approvals and
// effective approval mode from `WatchApprovals` (G17, SD1 Task 13). A daemon
// without approvals (`unimplemented`, before SD1 Task 13) shows none.

import { toJson } from '@bufbuild/protobuf';
import { ValueSchema } from '@bufbuild/protobuf/wkt';
import { Code, ConnectError, createClient, type Transport } from '@connectrpc/connect';
import { fromConnectTranscript } from '@loams/agentd-client/connect';
import {
  type Approval,
  ApprovalMode,
  ApprovalRisk,
  ApprovalService,
  type Chat,
  ThreadService,
} from '@loams/proto/agentd';
import {
  type PendingApproval,
  type TranscriptStore,
  type TranscriptWatch,
  transcriptStoreFor,
} from '@loams/studio-ui/chat';
import { useEffect, useMemo, useState } from 'react';

/** `ThreadService.WatchTranscript`, mapped to the ndjson frame the fold takes (T6-5). */
export function connectTranscriptWatch(transport: Transport): TranscriptWatch {
  const client = createClient(ThreadService, transport);
  return async function* watch(chatId, signal) {
    for await (const response of client.watchTranscript({ chatId }, { signal })) {
      if (response.update) yield fromConnectTranscript(response.update);
    }
  };
}

/** The transcript store for the daemon behind `transport`; the dock gets the same one. */
export function useTranscriptStore(transport: Transport | undefined): TranscriptStore | null {
  return useMemo(
    () => (transport ? transcriptStoreFor(transport, connectTranscriptWatch(transport)) : null),
    [transport],
  );
}

const RETRY_MS = 2000;

/**
 * Follows a server stream while mounted, reopening it after an error (but not after
 * `unimplemented`, which means the daemon has no such method yet).
 */
function useWatch<T>(
  transport: Transport | undefined,
  open: (transport: Transport, signal: AbortSignal) => AsyncIterable<T>,
  onMessage: (message: T) => void,
  deps: readonly unknown[],
): { unsupported: boolean; error: string | null } {
  const [state, setState] = useState<{ unsupported: boolean; error: string | null }>({
    unsupported: false,
    error: null,
  });
  // oxlint-disable-next-line react/exhaustive-deps -- the caller names the stream's inputs
  useEffect(() => {
    if (!transport) return;
    const abort = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const run = async () => {
      try {
        for await (const message of open(transport, abort.signal)) {
          setState({ unsupported: false, error: null });
          onMessage(message);
        }
      } catch (cause) {
        if (abort.signal.aborted) return;
        const error = ConnectError.from(cause);
        if (error.code === Code.Unimplemented) {
          setState({ unsupported: true, error: null });
          return;
        }
        setState({ unsupported: false, error: error.rawMessage });
      }
      if (!abort.signal.aborted) timer = setTimeout(run, RETRY_MS);
    };
    void run();
    return () => {
      abort.abort();
      if (timer) clearTimeout(timer);
    };
  }, [transport, ...deps]);
  return state;
}

/** The thread's record (title, config, branch) from `WatchThreads`. */
export function useThreadRecord(transport: Transport | undefined, chatId: string) {
  const [chat, setChat] = useState<Chat | null>(null);
  const [seen, setSeen] = useState(false);
  const state = useWatch(
    transport,
    (t, signal) => createClient(ThreadService, t).watchThreads({}, { signal }),
    (response) => {
      setSeen(true);
      setChat(response.chats.find((c) => c.id === chatId) ?? null);
    },
    [chatId],
  );
  return { chat, loaded: seen, ...state };
}

const RISK: Record<ApprovalRisk, PendingApproval['risk']> = {
  [ApprovalRisk.UNSPECIFIED]: 'unknown',
  [ApprovalRisk.READ]: 'read',
  [ApprovalRisk.WRITE]: 'write',
  [ApprovalRisk.DESTRUCTIVE]: 'destructive',
};

export function pendingApprovalOf(approval: Approval): PendingApproval {
  return {
    id: approval.id,
    chatId: approval.chatId,
    turnId: approval.turnId,
    tool: approval.tool,
    args: approval.args ? toJson(ValueSchema, approval.args) : null,
    risk: RISK[approval.risk] ?? 'unknown',
    createdAtMs: Number(approval.createdAtMs),
  };
}

/** A thread's approval mode as the header names it. */
export type ThreadApprovalMode = 'supervised' | 'full-access';

const MODE: Partial<Record<ApprovalMode, ThreadApprovalMode>> = {
  [ApprovalMode.SUPERVISED]: 'supervised',
  [ApprovalMode.FULL_ACCESS]: 'full-access',
};

/**
 * The thread's pending approvals (G17), oldest first, and its effective approval
 * mode as the daemon applies it (SD1 Task 13; `undefined` until it says).
 */
export function useThreadApprovals(transport: Transport | undefined, chatId: string) {
  const [pending, setPending] = useState<PendingApproval[]>([]);
  const [mode, setMode] = useState<ThreadApprovalMode | undefined>(undefined);
  const state = useWatch(
    transport,
    (t, signal) => createClient(ApprovalService, t).watchApprovals({ chatId }, { signal }),
    (response) => {
      setPending(
        response.pending
          .filter((a) => a.chatId === chatId)
          .map(pendingApprovalOf)
          .sort((a, b) => a.createdAtMs - b.createdAtMs),
      );
      setMode(response.mode === undefined ? undefined : MODE[response.mode]);
    },
    [chatId],
  );
  return {
    pending: state.unsupported ? [] : pending,
    mode: state.unsupported ? undefined : mode,
    ...state,
  };
}
