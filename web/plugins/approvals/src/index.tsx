// @loams/plugin-approvals: the approvals inbox (§37 §7.3, AP1a Task 8).
//
// Holds WatchApprovals while the plugin is active (disposing it aborts the
// stream), lists pending approvals with the server-rendered summary and
// detail lines, and decides them. The rules are the server's (AP0 Ruling
// 7): a reason to reject or for anything destructive, typed confirmation of
// the target for DESTRUCTIVE, and step-up when the session is older than 5
// minutes. The generic `approval.renderer` is registered under "*"; other
// plugins may register richer renderers for their own kinds.
//
// Beside them, labelled by source, are the agent's approvals (design §53 §4.5,
// §8.1; plan SD1 Task 13): the tool calls a supervised loams-agentd thread waits
// on, from agentd's own `WatchApprovals` while a daemon is reachable
// (`./agent.ts`), answered once, always for the thread, or denied.

import { toJson } from '@bufbuild/protobuf';
import { timestampDate, ValueSchema } from '@bufbuild/protobuf/wkt';
import { type PlatformService, type PluginModule, service, watch } from '@loams/console-host';
import type { Context } from '@loams/cordis';
import { approvals } from '@loams/proto';
import { ApprovalRisk } from '@loams/proto/agentd';
import { Badge, Button, Card, Empty, Field, Input, Notice, StatusTag } from '@loams/ui';
import { useRef, useState, useSyncExternalStore } from 'react';
import {
  type AgentApproval,
  AgentDecision,
  AgentInbox,
  followAgentApprovals,
  resolveAgentApproval,
} from './agent.js';
import { Inbox, REASON_TEXT, reasonOf } from './store.js';

export { AgentDecision, AgentInbox, followAgentApprovals } from './agent.js';
export { Inbox, REASON_TEXT, reasonOf } from './store.js';

type Client = ReturnType<typeof service<'rpc.approvals'>>;

const RISK_TEXT: Record<number, string> = {
  [approvals.Risk.LOW]: 'low risk',
  [approvals.Risk.MEDIUM]: 'medium risk',
  [approvals.Risk.HIGH]: 'high risk',
  [approvals.Risk.DESTRUCTIVE]: 'destructive',
};

export function ApprovalCard({
  approval,
  client,
  onDecided,
}: {
  approval: approvals.Approval;
  client: Client;
  onDecided: (a: approvals.Approval) => void;
}) {
  const destructive = approval.risk === approvals.Risk.DESTRUCTIVE;
  const confirmName = approval.target.name ?? '';
  const [reason, setReason] = useState('');
  const [typed, setTyped] = useState('');
  const [busy, setBusy] = useState(false);
  // One idempotency key per decision for this revision (the card remounts on
  // a new revision), so a retry after a lost answer is deduplicated.
  const keys = useRef(new Map<approvals.DecisionKind, string>());
  const [error, setError] = useState<string>();
  const requester = approval.requestedBy?.displayName ?? 'someone';
  // An agent acting for a user: the chain is [agent, user, ...].
  const onBehalfOf = approval.actorChain
    .slice(1)
    .map((p) => p.displayName)
    .join(' for ');
  const expires = approval.expiresAt ? timestampDate(approval.expiresAt) : undefined;

  const decide = async (decision: approvals.DecisionKind) => {
    setBusy(true);
    setError(undefined);
    try {
      const res = await client.decideApproval({
        approvalId: approval.id,
        revision: approval.revision,
        decision,
        reason,
        idempotencyKey:
          keys.current.get(decision) ??
          (keys.current.set(decision, crypto.randomUUID()).get(decision) as string),
      });
      if (res.approval) onDecided(res.approval);
    } catch (e) {
      const why = reasonOf(e);
      setError((why && REASON_TEXT[why]) ?? String(e));
    } finally {
      setBusy(false);
    }
  };

  const reasonNeeded = destructive;
  const canApprove =
    !busy && (!reasonNeeded || reason.trim()) && (!destructive || typed === confirmName);
  return (
    <Card
      title={approval.summary}
      actions={
        <>
          <Badge>Server</Badge>{' '}
          {approval.environment?.protected && <StatusTag status="progress">protected</StatusTag>}{' '}
          <Badge>{RISK_TEXT[approval.risk] ?? 'unknown risk'}</Badge>
        </>
      }
    >
      <p className="lc-muted">
        {approval.environment?.name} · requested by {requester}
        {onBehalfOf ? ` for ${onBehalfOf}` : ''}
        {expires ? ` · expires ${expires.toLocaleString()}` : ''}
      </p>
      <ul className="lc-detail-lines">
        {approval.detailLines.map((line, i) => (
          // Server-rendered lines may repeat; the list is fixed per revision.
          // oxlint-disable-next-line react/no-array-index-key -- see above
          <li key={i}>{line}</li>
        ))}
      </ul>
      <Field
        label="Reason"
        hint={reasonNeeded ? 'Required for destructive approvals' : 'Required to reject'}
      >
        {(props) => <Input {...props} value={reason} onChange={(e) => setReason(e.target.value)} />}
      </Field>
      {destructive && (
        <Field label={`Type ${confirmName} to confirm`}>
          {(props) => <Input {...props} value={typed} onChange={(e) => setTyped(e.target.value)} />}
        </Field>
      )}
      {error && (
        <Notice tone="danger" title="Not decided">
          {error}
        </Notice>
      )}
      <div className="lc-actions">
        <Button
          variant="primary"
          disabled={!canApprove}
          onClick={() => decide(approvals.DecisionKind.APPROVE)}
        >
          Approve
        </Button>
        <Button
          variant="danger"
          disabled={busy || !reason.trim()}
          onClick={() => decide(approvals.DecisionKind.REJECT)}
        >
          Reject
        </Button>
      </div>
    </Card>
  );
}

/** An agent approval's arguments as text (redacted by the daemon; never HTML). */
function argsText(approval: AgentApproval): string {
  if (!approval.args) return '';
  try {
    return JSON.stringify(toJson(ValueSchema, approval.args), null, 2);
  } catch {
    return '';
  }
}

const AGENT_RISK_TEXT: Record<number, string> = {
  [ApprovalRisk.READ]: 'reads',
  [ApprovalRisk.WRITE]: 'writes',
  [ApprovalRisk.DESTRUCTIVE]: 'destructive',
};

/** What answers an agent approval: once, always for its thread, or deny. */
export type ResolveAgentApproval = (
  approval: AgentApproval,
  decision: AgentDecision,
) => Promise<void>;

/** One tool call a supervised agent thread waits on. */
export function AgentApprovalCard({
  approval,
  resolve,
  onResolved,
}: {
  approval: AgentApproval;
  resolve: ResolveAgentApproval;
  onResolved: (a: AgentApproval) => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const decide = async (decision: AgentDecision) => {
    setBusy(true);
    setError(undefined);
    try {
      await resolve(approval, decision);
      onResolved(approval);
    } catch (e) {
      setError(reasonOf(e) === 'approval_not_pending' ? 'It was already answered.' : String(e));
    } finally {
      setBusy(false);
    }
  };
  const args = argsText(approval);
  const created = new Date(Number(approval.createdAtMs));
  return (
    <Card
      title={`${approval.tool} wants to run`}
      actions={
        <>
          <Badge>Agent</Badge> <Badge>{AGENT_RISK_TEXT[approval.risk] ?? 'unknown risk'}</Badge>
        </>
      }
    >
      <p className="lc-muted">
        loams-agentd thread{' '}
        <a href={`#/studio/t/${encodeURIComponent(approval.chatId)}`}>{approval.chatId}</a> · asked{' '}
        {created.toLocaleString()}
      </p>
      {args && <pre className="lc-detail-lines">{args}</pre>}
      {error && (
        <Notice tone="danger" title="Not answered">
          {error}
        </Notice>
      )}
      <div className="lc-actions">
        <Button variant="primary" disabled={busy} onClick={() => decide(AgentDecision.ONCE)}>
          Allow once
        </Button>
        <Button disabled={busy} onClick={() => decide(AgentDecision.ALWAYS)}>
          Always for this thread
        </Button>
        <Button variant="danger" disabled={busy} onClick={() => decide(AgentDecision.DENY)}>
          Deny
        </Button>
      </div>
    </Card>
  );
}

/** The agent approvals of the inbox page, when a daemon serves them. */
export interface AgentSource {
  inbox: AgentInbox;
  resolve: ResolveAgentApproval;
}

const NO_AGENT = new AgentInbox();

export function InboxPage({
  inbox,
  client,
  agent,
}: {
  inbox: Inbox;
  client: Client;
  agent?: AgentSource;
}) {
  const state = useSyncExternalStore(inbox.subscribe, inbox.getSnapshot);
  const agentInbox = agent?.inbox ?? NO_AGENT;
  const agentState = useSyncExternalStore(agentInbox.subscribe, agentInbox.getSnapshot);
  return (
    <div className="lc-page">
      <header className="lc-page-head">
        <h1>Approvals</h1>
        <p role="status">
          {state.connected
            ? 'Requests that wait for a person, updated as they arrive.'
            : 'Connecting to the approvals stream…'}
        </p>
      </header>
      {state.error && (
        <Notice tone="warn" title="The approvals stream dropped">
          The list may be out of date: {state.error}
        </Notice>
      )}
      {agentState.error && (
        <Notice tone="warn" title="The agent approvals stream dropped">
          The agent's list may be out of date: {agentState.error}
        </Notice>
      )}
      {state.approvals.length === 0 && agentState.approvals.length === 0 ? (
        <Empty title="Nothing to approve">
          Destructive operations and agent actions that need a person show up here.
        </Empty>
      ) : (
        <>
          {state.approvals.map((a) => (
            <ApprovalCard
              key={`${a.id}@${a.revision}`}
              approval={a}
              client={client}
              onDecided={(d) => inbox.replace(d)}
            />
          ))}
          {agent &&
            agentState.approvals.map((a) => (
              <AgentApprovalCard
                key={`agent:${a.chatId}:${a.id}`}
                approval={a}
                resolve={agent.resolve}
                onResolved={(r) => agent.inbox.remove(r)}
              />
            ))}
        </>
      )}
    </div>
  );
}

function PendingChip({ inbox, agent }: { inbox: Inbox; agent: AgentInbox }) {
  const state = useSyncExternalStore(inbox.subscribe, inbox.getSnapshot);
  const agentState = useSyncExternalStore(agent.subscribe, agent.getSnapshot);
  const n = state.approvals.length + agentState.approvals.length;
  if (n === 0) return null;
  return (
    <a className="lc-chip" href="#/approvals">
      {n} approval{n === 1 ? '' : 's'} waiting
    </a>
  );
}

interface BadgeBridge {
  shell: { setBadge(count: number): Promise<void> };
}

/** Approvals still waiting for a decision (the inbox keeps only pending ones). */
export function pendingCount(state: { approvals: approvals.Approval[] }): number {
  return state.approvals.filter((a) => a.state === approvals.ApprovalState.PENDING).length;
}

export interface ApprovalsConfig {
  /** Raise a platform notification for each new pending approval. */
  notify?: boolean;
}

function notifier(platform: PlatformService) {
  return (a: approvals.Approval) => {
    platform
      .notify({ title: 'Approval needed', body: a.summary, route: `/approvals` })
      .catch(() => {});
  };
}

const plugin: PluginModule<ApprovalsConfig> = {
  name: 'approvals',
  inject: ['rpc.approvals', 'router', 'slots', 'platform', 'agentd'],
  apply(ctx: Context, config: ApprovalsConfig) {
    const client = service(ctx, 'rpc.approvals');
    const router = service(ctx, 'router');
    const slots = service(ctx, 'slots');
    const agentd = service(ctx, 'agentd');
    const inbox = new Inbox();
    // The agent's approvals while a daemon is reachable (SD1 Task 13).
    const agentInbox = new AgentInbox();
    ctx.effect(() => followAgentApprovals(agentd, agentInbox));
    const agent: AgentSource = {
      inbox: agentInbox,
      resolve: (approval, decision) => resolveAgentApproval(agentd, approval, decision),
    };
    if (config?.notify) inbox.onNew = notifier(service(ctx, 'platform'));
    watch(
      ctx,
      (signal) => client.watchApprovals({ resumeCursor: inbox.getSnapshot().cursor }, { signal }),
      (message) => inbox.apply(message),
      (error) => inbox.failed(error),
    );
    // Desktop shell only: mirror the pending count to the tray/dock badge. A web
    // build has no `loamsDesktop`, so this is a no-op there.
    ctx.effect(() => {
      const shell = (globalThis as { loamsDesktop?: BadgeBridge }).loamsDesktop?.shell;
      if (!shell) return () => {};
      let last = -1;
      const sync = () => {
        const n = pendingCount(inbox.getSnapshot()) + agentInbox.getSnapshot().approvals.length;
        if (n === last) return;
        last = n;
        shell.setBadge(n).catch(() => {});
      };
      sync();
      const off = inbox.subscribe(sync);
      const offAgent = agentInbox.subscribe(sync);
      return () => {
        off();
        offAgent();
        shell.setBadge(0).catch(() => {});
      };
    });
    ctx.effect(() =>
      router.page(
        {
          id: 'approvals',
          path: '/approvals',
          title: 'Approvals',
          plugin: 'approvals',
          nav: { group: 'Operate', order: 20 },
        },
        () => <InboxPage inbox={inbox} client={client} agent={agent} />,
      ),
    );
    ctx.effect(() =>
      slots.register({ name: 'shell.overlay', plugin: 'approvals' }, () => (
        <PendingChip inbox={inbox} agent={agentInbox} />
      )),
    );
  },
};

export default plugin;
