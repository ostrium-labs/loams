import { createClient } from '@connectrpc/connect';
import { useQuery, useTransport } from '@connectrpc/connect-query';
import {
  AuditTable,
  LoadError,
  Loading,
  Page,
  PageHead,
  useAccount,
  useTrail,
} from '@loams/admin-ui';
import type { RouterService, SessionService } from '@loams/console-host';
import { admin } from '@loams/proto';
import { Card, Checkbox, Field, SelectMenu, StatusTag } from '@loams/ui';
import { useEffect, useRef, useState } from 'react';

/** The newest events the page keeps; older ones fall off the bottom. */
export const KEEP = 500;

export type WatchState =
  | { phase: 'connecting'; events: admin.AuditEvent[] }
  | { phase: 'live'; events: admin.AuditEvent[] }
  | { phase: 'failed'; events: admin.AuditEvent[]; error: unknown };

/** Folds one `WatchAuditEvents` message into the newest-first list. */
export function fold(events: admin.AuditEvent[], m: admin.WatchAuditEventsResponse) {
  switch (m.event.case) {
    case 'snapshot':
      return m.event.value.events.slice(0, KEEP);
    case 'append': {
      const added = m.event.value;
      return [added, ...events.filter((e) => e.id !== added.id)].slice(0, KEEP);
    }
    default:
      return events;
  }
}

/**
 * The audit log, live: a snapshot, then every event as it happens (API2 Task 7's
 * `AuditService/WatchAuditEvents`). A dropped stream resumes from its last cursor.
 */
export function useAuditWatch(project: string): [WatchState, () => void] {
  const transport = useTransport();
  const [state, setState] = useState<WatchState>({ phase: 'connecting', events: [] });
  const [attempt, setAttempt] = useState(0);
  // Where the last stream for this project stopped, so a retry resumes there.
  const resume = useRef({ project, cursor: '', events: [] as admin.AuditEvent[] });
  // oxlint-disable-next-line react/exhaustive-deps -- `attempt` re-opens the stream
  useEffect(() => {
    const client = createClient(admin.AuditService, transport);
    const controller = new AbortController();
    if (resume.current.project !== project) resume.current = { project, cursor: '', events: [] };
    const from = resume.current;
    let cursor = from.cursor;
    let events = from.events;
    setState({ phase: 'connecting', events });
    (async () => {
      try {
        for await (const m of client.watchAuditEvents(
          { project, snapshotSize: 100, resumeCursor: cursor },
          { signal: controller.signal },
        )) {
          if (m.cursor) cursor = m.cursor;
          events = fold(events, m);
          resume.current = { project, cursor, events };
          setState({ phase: 'live', events });
        }
      } catch (error) {
        if (!controller.signal.aborted) setState({ phase: 'failed', events, error });
      }
    })();
    return () => controller.abort();
  }, [project, transport, attempt]);
  return [state, () => setAttempt((n) => n + 1)];
}

const ALL = 'all';

export function AuditPage({ router, session }: { router: RouterService; session: SessionService }) {
  const orgName = useAccount(session)?.org?.name ?? 'Organization';
  useTrail(router, [{ label: orgName, href: '/' }, { label: 'Audit log' }]);
  const projects = useQuery(admin.ProjectService.method.listProjects, {});
  const [project, setProject] = useState(ALL);
  const [deniedOnly, setDeniedOnly] = useState(false);
  const [watch, retry] = useAuditWatch(project === ALL ? '' : project);
  const events = watch.events.filter((e) => !deniedOnly || e.outcome === admin.AuditOutcome.DENIED);
  return (
    <Page>
      <PageHead eyebrow="Organization" title="Audit log">
        Every mutating call and every token issued, with the actor and whom it acted for.
      </PageHead>
      <div className="filters">
        <Field label="Project">
          {() => (
            <SelectMenu
              label="Project"
              value={project}
              onValueChange={setProject}
              options={[
                { value: ALL, label: 'All projects and the org' },
                ...(projects.data?.projects ?? []).map((p) => ({ value: p.slug, label: p.name })),
              ]}
            />
          )}
        </Field>
        <Checkbox
          label="Denied only"
          checked={deniedOnly}
          onChange={(e) => setDeniedOnly(e.target.checked)}
        />
        <span aria-live="polite">
          {watch.phase === 'live' ? (
            <StatusTag status="done">Live</StatusTag>
          ) : watch.phase === 'connecting' ? (
            <StatusTag status="progress">Connecting</StatusTag>
          ) : (
            <StatusTag status="failed">Disconnected</StatusTag>
          )}
        </span>
      </div>
      {watch.phase === 'failed' && (
        <div style={{ marginBottom: 16 }}>
          <LoadError error={watch.error} retry={retry} />
        </div>
      )}
      <Card flush>
        {watch.phase === 'connecting' && watch.events.length === 0 ? (
          <Loading />
        ) : (
          <AuditTable events={events} />
        )}
      </Card>
    </Page>
  );
}
