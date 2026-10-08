import type { FactoryAppInfo, LoamsDesktopApi } from '@loams/desktop/contracts';
import { Card, Stat, Stats } from '@loams/ui';
import { useEffect, useState } from 'react';

interface Summary {
  prs?: number;
  errors?: number;
  issues?: number;
}

async function q<T>(
  desktop: LoamsDesktopApi,
  app: 'forgejo' | 'glitchtip',
  op: string,
  params: Record<string, unknown>,
) {
  const r = await desktop.factory.query<T>({ app, op, params });
  return r.ok ? r.value : undefined;
}

async function load(desktop: LoamsDesktopApi, apps: FactoryAppInfo[]): Promise<Summary> {
  const on = (id: string) => apps.some((a) => a.id === id && a.health === 'ok');
  const out: Summary = {};
  if (on('forgejo')) {
    const repos = await q<{ fullName?: string; openIssues?: number }[]>(
      desktop,
      'forgejo',
      'repos',
      {
        limit: 5,
      },
    );
    if (repos) {
      out.issues = repos.reduce((n, r) => n + (r.openIssues ?? 0), 0);
      let prs = 0;
      for (const r of repos) {
        const [owner, repo] = r.fullName?.split('/') ?? [];
        if (!owner || !repo) continue;
        prs +=
          (
            await q<unknown[]>(desktop, 'forgejo', 'issues', {
              type: 'pulls',
              owner,
              repo,
              limit: 50,
            })
          )?.length ?? 0;
      }
      out.prs = prs;
    }
  }
  if (on('glitchtip')) {
    const orgs = await q<{ slug?: string }[]>(desktop, 'glitchtip', 'organizations', {});
    const slug = orgs?.[0]?.slug;
    if (slug) {
      out.errors = (
        await q<unknown[]>(desktop, 'glitchtip', 'issues', { orgSlug: slug, limit: 50 })
      )?.length;
    }
  }
  return out;
}

/** `environment.overview.card`: factory.summary, only while an app it reads is configured. */
export function SummaryCard({ desktop }: { desktop: LoamsDesktopApi }) {
  const [s, setS] = useState<Summary>();
  useEffect(() => {
    let live = true;
    desktop.factory
      .list()
      .then((apps) => load(desktop, apps))
      .then((x) => live && setS(x))
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [desktop]);
  if (!s || (s.prs === undefined && s.errors === undefined && s.issues === undefined)) return null;
  return (
    <div data-card="factory.summary">
      <Card title="Software Factory">
        <Stats>
          {s.prs !== undefined && <Stat label="Open PRs" value={s.prs} />}
          {s.errors !== undefined && <Stat label="Unresolved errors" value={s.errors} />}
          {s.issues !== undefined && <Stat label="Open issues" value={s.issues} />}
        </Stats>
      </Card>
    </div>
  );
}
