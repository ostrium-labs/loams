import { timestampFromDate } from '@bufbuild/protobuf/wkt';
import { Code, ConnectError } from '@connectrpc/connect';
import { admin } from '@loams/proto';
import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import { AuditTable, avatarKind, errorText, iso, num, slugify, useTrail } from '../src/index.js';
import { SEED, stubRouter, withConsole } from '../src/testing.js';

afterEach(cleanup);

describe('@loams/admin-ui', () => {
  it('turns proto values into what the pages show', () => {
    const at = new Date('2026-10-13T09:00:00Z');
    expect(iso(timestampFromDate(at))).toBe('2026-10-13T09:00:00.000Z');
    expect(iso(undefined)).toBeUndefined();
    expect(num(394n)).toBe(394);
    expect(avatarKind(admin.SubjectKind.SERVICE_ACCOUNT)).toBe('service_account');
    expect(slugify(' QA env! ')).toBe('qa-env');
  });

  it('says what failed in one sentence', () => {
    expect(errorText(new ConnectError('no such project', Code.NotFound))).toBe('no such project');
    expect(errorText(new ConnectError('', Code.Unavailable))).toMatch(
      /Check that the API is reachable/,
    );
    expect(errorText(new Error('boom'))).toBe('boom');
  });

  it('audit_table_marks_denied_events_and_acting_for', () => {
    render(withConsole(<AuditTable events={SEED.events} />));
    expect(screen.getByText('documents.delete')).toBeTruthy();
    expect(screen.getByText('Denied')).toBeTruthy();
    expect(screen.getByText(/for Aiko Tanaka/)).toBeTruthy();
    expect(screen.getByText('docs-search/production')).toBeTruthy();
  });

  it('use_trail_sets_the_page_breadcrumbs', () => {
    const router = stubRouter();
    function Probe() {
      useTrail(router, [{ label: 'Acme', href: '/' }, { label: 'Teams' }]);
      return null;
    }
    render(<Probe />);
    expect(router.trail()).toEqual([{ label: 'Acme', href: '/' }, { label: 'Teams' }]);
  });
});
