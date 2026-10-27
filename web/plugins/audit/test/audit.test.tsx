// The audit log, live over AuditService/WatchAuditEvents (API2 Task 10).

import { create, type MessageInitShape } from '@bufbuild/protobuf';
import {
  createAdminTransport,
  SEED,
  stubRouter,
  stubSession,
  withConsole,
} from '@loams/admin-ui/testing';
import { admin } from '@loams/proto';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import { AuditPage } from '../src/audit.js';
import { fold } from '../src/index.js';

afterEach(cleanup);

const appended = create(admin.AuditEventSchema, {
  id: 'evt_c0001',
  action: 'api_key.created',
  actor: { kind: admin.SubjectKind.USER, name: 'Priya Raman' },
  target: { kind: 'api_key', name: 'ci' },
  outcome: admin.AuditOutcome.SUCCESS,
});

const msg = (init: MessageInitShape<typeof admin.WatchAuditEventsResponseSchema>) =>
  create(admin.WatchAuditEventsResponseSchema, init);

describe('@loams/plugin-audit', () => {
  it('audit_page_is_live_over_watch_audit_events', async () => {
    const watched: admin.WatchAuditEventsRequest[] = [];
    let release: () => void = () => {};
    const later = new Promise<void>((r) => {
      release = r;
    });
    const transport = createAdminTransport({
      audit: {
        async *watchAuditEvents(req) {
          watched.push(req);
          yield msg({ cursor: 'a2', event: { case: 'snapshot', value: { events: SEED.events } } });
          await later;
          yield msg({ cursor: 'a3', event: { case: 'append', value: appended } });
          yield msg({ cursor: 'a3', event: { case: 'heartbeat', value: {} } });
        },
      },
    });
    render(
      withConsole(<AuditPage router={stubRouter('/audit')} session={stubSession()} />, transport),
    );
    expect(await screen.findByText('query.search')).toBeTruthy();
    expect(screen.getByText('Live')).toBeTruthy();
    release();
    expect(await screen.findByText('api_key.created')).toBeTruthy();
    expect(watched[0]).toMatchObject({ project: '', snapshotSize: 100 });
  });

  it('filters_by_project_and_outcome', async () => {
    const watched: string[] = [];
    const transport = createAdminTransport({
      audit: {
        async *watchAuditEvents(req) {
          watched.push(req.project);
          yield msg({ cursor: 'a1', event: { case: 'snapshot', value: { events: SEED.events } } });
        },
      },
    });
    render(
      withConsole(<AuditPage router={stubRouter('/audit')} session={stubSession()} />, transport),
    );
    expect(await screen.findByText('query.search')).toBeTruthy();
    fireEvent.click(screen.getByLabelText('Denied only'));
    await waitFor(() => expect(screen.queryByText('query.search')).toBeNull());
    expect(screen.getByText('documents.delete')).toBeTruthy();
    expect(watched).toEqual(['']);
  });

  it('folds_snapshots_and_appends_newest_first', () => {
    const snap = create(admin.WatchAuditEventsResponseSchema, {
      event: { case: 'snapshot', value: { events: SEED.events } },
    });
    const add = create(admin.WatchAuditEventsResponseSchema, {
      event: { case: 'append', value: appended },
    });
    const beat = create(admin.WatchAuditEventsResponseSchema, {
      event: { case: 'heartbeat', value: {} },
    });
    const events = fold(fold(fold([], snap), add), beat);
    expect(events.map((e) => e.id)).toEqual(['evt_c0001', 'evt_1', 'evt_2']);
    // A replayed append (after a resume) is not listed twice.
    expect(fold(events, add).map((e) => e.id)).toEqual(['evt_c0001', 'evt_1', 'evt_2']);
  });
});
