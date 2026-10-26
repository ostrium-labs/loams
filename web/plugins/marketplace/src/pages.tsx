// The Seed Exchange's pages (design §73 §7.1–§7.2, §10.3; plan MK1 Task 9): browse
// with filters, a listing in plain words, the consent screen, the installed listings
// with their updates and revocation banners, and the sources.

import type {
  InstalledListing,
  ListingRevocation,
  ListingSummary,
  ListingTier,
  SearchListingsParams,
} from '@loams/agentd-client';
import {
  Badge,
  Button,
  Card,
  ConfirmDialog,
  Empty,
  Field,
  Input,
  Notice,
  SearchInput,
  Select,
  SkeletonList,
} from '@loams/ui';
import { useMutation } from '@tanstack/react-query';
import { type FormEvent, type ReactNode, useState } from 'react';
import { ConsentScreen } from './consent.js';
import {
  type useApi,
  useInstalled,
  useListing,
  usePlan,
  useRefresh,
  useSearch,
  useSources,
  useUpdates,
} from './data.js';
import { type MarketplaceError, marketplaceError } from './errors.js';
import {
  FRAMEWORKS,
  frameworkLabel,
  KINDS,
  kindLabel,
  LANGUAGES,
  LEVELS,
  languageLabel,
  levelLabel,
  riskLabel,
  riskVariant,
  TIERS,
  tierLabel,
  tierVariant,
} from './labels.js';
import { href, type Page } from './paths.js';

type Api = NonNullable<ReturnType<typeof useApi>>;

export interface PageProps {
  base: string;
  params: Record<string, string>;
  navigate: (to: string) => void;
}

function ErrorNotice({ error, title = "This didn't load" }: { error: unknown; title?: string }) {
  return (
    <Notice tone="danger" title={title}>
      {marketplaceError(error).message}
    </Notice>
  );
}

export function RevocationBanner({
  revocation,
  installed = false,
}: {
  revocation: ListingRevocation;
  installed?: boolean;
}) {
  const disable = revocation.severity === 'disable';
  return (
    <Notice tone={disable ? 'danger' : 'warn'} title={disable ? 'Revoked' : 'Advisory'}>
      {revocation.reason} (versions {revocation.versions}).{' '}
      {disable
        ? installed
          ? 'agentd stopped it and removed its harness and MCP server registrations. Its data stays.'
          : 'It cannot be installed.'
        : 'It still runs; read the advisory before relying on it.'}
      {revocation.advisory && (
        <>
          {' '}
          <a href={revocation.advisory} target="_blank" rel="noreferrer">
            Read the advisory
          </a>
        </>
      )}
    </Notice>
  );
}

function Badges({ s }: { s: ListingSummary }) {
  return (
    <span className="flex flex-wrap items-center gap-1.5">
      <Badge>{kindLabel(s.kind)}</Badge>
      <Badge variant={tierVariant(s.tier)}>{tierLabel(s.tier)}</Badge>
      {s.framework && <Badge>{frameworkLabel(s.framework)}</Badge>}
      {s.language && <Badge>{languageLabel(s.language)}</Badge>}
      {s.level && <Badge variant="accent">{levelLabel(s.level)}</Badge>}
    </span>
  );
}

// ── Browse ─────────────────────────────────────────────────────────────────────

/** The search filters from the route's query (`?kind=harness` from Studio's picker). */
export function filtersOf(params: Record<string, string>): SearchListingsParams {
  const one = (v: string | undefined) => (v ? [v] : []);
  return {
    query: params.q ?? '',
    kinds: one(params.kind),
    frameworks: one(params.framework),
    languages: one(params.language),
    minLevel: params.level || null,
    tiers: one(params.tier) as ListingTier[],
    source: params.source || null,
  };
}

function FilterSelect({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: string;
  options: { value: string; label: string }[];
  onChange: (value: string) => void;
}) {
  return (
    <Field label={label}>
      {(p) => (
        <Select {...p} value={value} onChange={(e) => onChange(e.target.value)}>
          <option value="">Any</option>
          {options.map((o) => (
            <option key={o.value} value={o.value}>
              {o.label}
            </option>
          ))}
        </Select>
      )}
    </Field>
  );
}

export function BrowsePage({ api, base, params, navigate }: PageProps & { api: Api }) {
  const [query, setQuery] = useState(params.q ?? '');
  const filters = filtersOf(params);
  const results = useSearch(api, filters);
  const sources = useSources(api);
  const installed = useInstalled(api);
  const set = (key: string, value: string) =>
    navigate(href(base, 'browse', { ...params, [key]: value }));
  const installedIds = new Set((installed.data ?? []).map((i) => i.id));
  return (
    <div className="flex flex-col gap-4">
      <form
        aria-label="Search the Seed Exchange"
        className="flex flex-col gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          set('q', query);
        }}
      >
        <SearchInput
          value={query}
          onValueChange={setQuery}
          onSubmit={(v) => set('q', v)}
          label="Search listings"
          placeholder="Search by name, summary or tool"
        />
        <div className="grid grid-cols-2 gap-3 md:grid-cols-3 xl:grid-cols-6">
          <FilterSelect
            label="Kind"
            value={params.kind ?? ''}
            options={KINDS.map((k) => ({ value: k, label: kindLabel(k) }))}
            onChange={(v) => set('kind', v)}
          />
          <FilterSelect
            label="Framework"
            value={params.framework ?? ''}
            options={FRAMEWORKS.map((f) => ({ value: f, label: frameworkLabel(f) }))}
            onChange={(v) => set('framework', v)}
          />
          <FilterSelect
            label="Language"
            value={params.language ?? ''}
            options={LANGUAGES.map((l) => ({ value: l, label: languageLabel(l) }))}
            onChange={(v) => set('language', v)}
          />
          <FilterSelect
            label="Level"
            value={params.level ?? ''}
            options={LEVELS.map((l) => ({ value: l, label: `${levelLabel(l)} or more` }))}
            onChange={(v) => set('level', v)}
          />
          <FilterSelect
            label="Tier"
            value={params.tier ?? ''}
            options={TIERS.map((t) => ({ value: t, label: tierLabel(t) }))}
            onChange={(v) => set('tier', v)}
          />
          <FilterSelect
            label="Source"
            value={params.source ?? ''}
            options={(sources.data ?? []).map((s) => ({ value: s.name, label: s.name }))}
            onChange={(v) => set('source', v)}
          />
        </div>
      </form>
      {results.isPending ? (
        <SkeletonList label="Loading listings" rows={4} />
      ) : results.isError ? (
        <ErrorNotice error={results.error} />
      ) : results.data.length === 0 ? (
        <Empty title="No listings match">Clear a filter, or add a source under Sources.</Empty>
      ) : (
        <ul aria-label="Listings" className="m-0 grid list-none gap-3 p-0 md:grid-cols-2">
          {results.data.map((s) => (
            <li key={`${s.source}/${s.id}`}>
              <article
                aria-label={s.title}
                className="flex h-full flex-col gap-2 rounded-sm border border-rule bg-raised p-3"
              >
                <header className="flex items-start justify-between gap-2">
                  <div className="min-w-0">
                    <h3 className="m-0 text-body font-semibold text-ink">{s.title}</h3>
                    <code className="text-micro text-muted">{s.id}</code>
                  </div>
                  {s.latest && <Badge variant="outline">{s.latest}</Badge>}
                </header>
                <p className="m-0 text-small text-muted">{s.summary}</p>
                <Badges s={s} />
                {s.revocation && <RevocationBanner revocation={s.revocation} />}
                <footer className="mt-auto flex items-center gap-2">
                  <Button
                    size="sm"
                    variant="secondary"
                    onClick={() => navigate(href(base, 'listing', { id: s.id, source: s.source }))}
                  >
                    Details
                  </Button>
                  {installedIds.has(s.id) ? (
                    <Badge variant="success">Installed</Badge>
                  ) : (
                    <span className="text-micro text-faint">from {s.source}</span>
                  )}
                </footer>
              </article>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

// ── A listing in plain words ───────────────────────────────────────────────────

type Json = Record<string, unknown>;
const obj = (v: unknown): Json | undefined =>
  v !== null && typeof v === 'object' && !Array.isArray(v) ? (v as Json) : undefined;
const str = (v: unknown): string | undefined => (typeof v === 'string' && v ? v : undefined);

export function ListingPage({ api, base, params, navigate }: PageProps & { api: Api }) {
  const id = params.id ?? '';
  const detail = useListing(api, id, params.version || null, params.source || null);
  const installed = useInstalled(api);
  if (!id) return <Notice tone="warn" title="No listing named" />;
  if (detail.isPending) return <SkeletonList label="Loading the listing" rows={5} />;
  if (detail.isError) return <ErrorNotice error={detail.error} />;
  const { summary: s, listing, versions } = detail.data;
  const mine = installed.data?.find((i) => i.id === s.id);
  const publisher = obj(listing.publisher);
  const runtime = obj(listing.runtime);
  const tools = Array.isArray(listing.tools) ? listing.tools.map(obj).filter((t) => !!t) : [];
  return (
    <div className="flex flex-col gap-4">
      <header className="flex flex-col gap-1">
        <h2 className="m-0 text-title font-semibold text-ink">{s.title}</h2>
        <p className="m-0 flex flex-wrap items-center gap-1.5 text-small text-muted">
          <code>{s.id}</code>
          <Badges s={s} />
          <span>from {s.source}</span>
        </p>
        <p className="m-0 text-ink">{s.summary}</p>
      </header>
      {s.revocation && <RevocationBanner revocation={s.revocation} />}
      <div className="flex flex-wrap items-center gap-2">
        <Button
          disabled={s.revocation?.severity === 'disable'}
          onClick={() =>
            navigate(href(base, 'install', { id: s.id, source: s.source, version: params.version }))
          }
        >
          {mine ? 'Review and reinstall…' : 'Install…'}
        </Button>
        {mine && <Badge variant="success">Installed {mine.version}</Badge>}
        <span className="text-small text-muted">
          Install shows what it will be allowed to do first.
        </span>
      </div>
      {str(listing.description) && <p className="m-0 text-ink">{str(listing.description)}</p>}
      <Card title="In plain words" headingLevel={3}>
        <dl className="m-0 grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-small">
          <dt className="text-muted">Publisher</dt>
          <dd className="m-0">{str(publisher?.name) ?? 'Not declared'}</dd>
          <dt className="text-muted">Licence</dt>
          <dd className="m-0">{str(listing.licence) ?? 'Not declared'}</dd>
          <dt className="text-muted">Runs on</dt>
          <dd className="m-0">
            {str(runtime?.kind) ?? 'agentd'}
            {str(runtime?.version) ? ` ${str(runtime?.version)}` : ''}
          </dd>
          <dt className="text-muted">Source</dt>
          <dd className="m-0">
            <code className="break-all">{str(obj(listing.source)?.repository) ?? '—'}</code>
          </dd>
          <dt className="text-muted">Support</dt>
          <dd className="m-0">{str(listing.support) ?? 'Not declared'}</dd>
        </dl>
      </Card>
      <Card title="Tools" headingLevel={3}>
        {tools.length === 0 ? (
          <p className="m-0 text-small text-muted">No tools declared.</p>
        ) : (
          <ul aria-label="Tools" className="m-0 flex list-none flex-col gap-1.5 p-0">
            {tools.map((t) => (
              <li key={str(t.name)} className="flex flex-wrap items-baseline gap-1.5 text-small">
                <Badge variant={riskVariant(str(t.risk) ?? '')}>
                  {riskLabel(str(t.risk) ?? '')}
                </Badge>
                <code>{str(t.name)}</code>
                <span className="text-muted">{str(t.description)}</span>
              </li>
            ))}
          </ul>
        )}
      </Card>
      <Card title="Versions" headingLevel={3}>
        <ul aria-label="Versions" className="m-0 flex list-none flex-col gap-1 p-0 text-small">
          {[...versions].reverse().map((v) => (
            <li key={v.version} className="flex flex-wrap items-center gap-1.5">
              <code>{v.version}</code>
              {v.version === s.latest && <Badge variant="accent">Latest</Badge>}
              {v.yanked && <Badge variant="warning">Yanked: {v.yanked}</Badge>}
              {v.revocation && (
                <Badge variant={v.revocation.severity === 'disable' ? 'danger' : 'warning'}>
                  {v.revocation.severity === 'disable' ? 'Revoked' : 'Advisory'}:{' '}
                  {v.revocation.reason}
                </Badge>
              )}
            </li>
          ))}
        </ul>
      </Card>
    </div>
  );
}

// ── Consent ────────────────────────────────────────────────────────────────────

export function InstallPage({ api, base, params, navigate }: PageProps & { api: Api }) {
  const id = params.id ?? '';
  const update = params.update === '1';
  const plan = usePlan(api, id, params.version || null, params.source || null);
  const refresh = useRefresh(api);
  const [error, setError] = useState<MarketplaceError | undefined>();
  const confirm = useMutation({
    mutationFn: async (secrets: Record<string, string>) => {
      if (!plan.data) throw new Error('No plan');
      const p = plan.data;
      // Exactly the plan the person read: agentd answers plan_changed otherwise.
      return update
        ? api.api.update({ id: p.summary.id, version: p.version, planDigest: p.digest })
        : api.api.install({
            id: p.summary.id,
            version: p.version,
            source: p.summary.source,
            planDigest: p.digest,
            config: {},
            secrets,
          });
    },
    onMutate: () => setError(undefined),
    onError: (e) => setError(marketplaceError(e)),
    onSuccess: async () => {
      await refresh();
      navigate(href(base, 'installed', { done: id }));
    },
  });
  if (!id) return <Notice tone="warn" title="No listing named" />;
  if (plan.isPending) return <SkeletonList label="Making the install plan" rows={6} />;
  if (plan.isError) return <ErrorNotice error={plan.error} title="No plan" />;
  return (
    <ConsentScreen
      plan={plan.data}
      mode={update ? 'update' : 'install'}
      {...(params.from ? { from: params.from } : {})}
      pending={confirm.isPending}
      error={error}
      onConfirm={(secrets) => confirm.mutate(secrets)}
      onCancel={() => navigate(href(base, 'listing', { id, source: params.source }))}
      onReplan={() => {
        setError(undefined);
        void plan.refetch();
      }}
    />
  );
}

// ── Installed ──────────────────────────────────────────────────────────────────

function InstalledRow({
  item,
  update,
  api,
  base,
  navigate,
}: {
  item: InstalledListing;
  update?: { to: string; widens: boolean } | undefined;
  api: Api;
  base: string;
  navigate: (to: string) => void;
}) {
  const refresh = useRefresh(api);
  const [error, setError] = useState<MarketplaceError | undefined>();
  const [confirming, setConfirming] = useState(false);
  const run = useMutation({
    mutationFn: async (what: 'update' | 'uninstall'): Promise<void> => {
      if (what === 'uninstall') await api.api.uninstall(item.id);
      else await api.api.update({ id: item.id, version: update?.to ?? null, planDigest: null });
    },
    onMutate: () => setError(undefined),
    onError: (e) => setError(marketplaceError(e)),
    onSuccess: () => refresh(),
  });
  const consent = () =>
    navigate(
      href(base, 'install', {
        id: item.id,
        source: item.source,
        version: update?.to,
        update: '1',
        from: item.version,
      }),
    );
  return (
    <li aria-label={item.id} className="flex flex-col gap-2 border-rule-soft border-b py-3">
      <div className="flex flex-wrap items-center gap-2">
        <code className="text-ink">{item.id}</code>
        <Badge variant="outline">{item.version}</Badge>
        <Badge>{kindLabel(item.kind)}</Badge>
        <span className="text-small text-muted">
          from {item.source}, installed {new Date(item.installedMs).toLocaleDateString()}
        </span>
        <span className="ml-auto flex items-center gap-2">
          {update &&
            (update.widens ? (
              <Button size="sm" onClick={consent}>
                Review update to {update.to}…
              </Button>
            ) : (
              <Button size="sm" disabled={run.isPending} onClick={() => run.mutate('update')}>
                Update to {update.to}
              </Button>
            ))}
          <Button
            size="sm"
            variant="danger"
            disabled={run.isPending}
            onClick={() => setConfirming(true)}
          >
            Uninstall
          </Button>
        </span>
      </div>
      {update?.widens && (
        <p className="m-0 text-small text-muted">
          {update.to} asks for more than {item.version}, so it needs your consent again.
        </p>
      )}
      {item.revocation && <RevocationBanner revocation={item.revocation} installed />}
      {error && (
        <Notice tone="danger" title="That failed">
          {error.message}
          {error.reason === 'consent_required' && (
            <>
              {' '}
              <Button size="sm" variant="secondary" onClick={consent}>
                Read the plan
              </Button>
            </>
          )}
        </Notice>
      )}
      <ConfirmDialog
        open={confirming}
        onClose={() => setConfirming(false)}
        object={item.id}
        impact="agentd removes the package, its registrations and its secrets. The agent's data stays where it is."
        confirm="Uninstall"
        onConfirm={() => {
          setConfirming(false);
          run.mutate('uninstall');
        }}
      />
    </li>
  );
}

export function InstalledPage({ api, base, params, navigate }: PageProps & { api: Api }) {
  const installed = useInstalled(api);
  const updates = useUpdates(api);
  if (installed.isPending) return <SkeletonList label="Loading installed listings" rows={3} />;
  if (installed.isError) return <ErrorNotice error={installed.error} />;
  const byId = new Map((updates.data ?? []).map((u) => [u.id, u]));
  return (
    <div className="flex flex-col gap-3">
      {params.done && (
        <Notice tone="success" title="Installed">
          <code>{params.done}</code> is installed. A harness shows in Studio's harness picker.
        </Notice>
      )}
      {updates.isError && <ErrorNotice error={updates.error} title="Updates were not checked" />}
      {installed.data.length === 0 ? (
        <Empty title="Nothing installed">Browse the Seed Exchange to install a listing.</Empty>
      ) : (
        <ul aria-label="Installed listings" className="m-0 list-none p-0">
          {installed.data.map((item) => (
            <InstalledRow
              key={item.id}
              item={item}
              update={byId.get(item.id)}
              api={api}
              base={base}
              navigate={navigate}
            />
          ))}
        </ul>
      )}
    </div>
  );
}

// ── Sources ────────────────────────────────────────────────────────────────────

export function SourcesPage({ api }: PageProps & { api: Api }) {
  const sources = useSources(api);
  const refresh = useRefresh(api);
  const [form, setForm] = useState({ name: '', url: '', identity: '', issuer: '' });
  const [error, setError] = useState<MarketplaceError | undefined>();
  const add = useMutation({
    mutationFn: () =>
      api.api.addSource({
        name: form.name.trim(),
        url: form.url.trim(),
        identity: form.identity.trim(),
        issuer: form.issuer.trim() || null,
      }),
    onMutate: () => setError(undefined),
    onError: (e) => setError(marketplaceError(e)),
    onSuccess: async () => {
      setForm({ name: '', url: '', identity: '', issuer: '' });
      await refresh();
    },
  });
  const remove = useMutation({
    mutationFn: (name: string) => api.api.removeSource(name),
    onError: (e) => setError(marketplaceError(e)),
    onSuccess: () => refresh(),
  });
  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (form.name && form.url && form.identity) add.mutate();
  };
  const field = (key: keyof typeof form, label: string, hint?: string) => (
    <Field label={label} {...(hint ? { hint } : {})}>
      {(p) => (
        <Input
          {...p}
          value={form[key]}
          onChange={(e) => setForm((f) => ({ ...f, [key]: e.target.value }))}
        />
      )}
    </Field>
  );
  return (
    <div className="flex flex-col gap-4">
      {sources.isPending ? (
        <SkeletonList label="Loading sources" rows={2} />
      ) : sources.isError ? (
        <ErrorNotice error={sources.error} />
      ) : (
        <ul aria-label="Sources" className="m-0 list-none p-0">
          {sources.data.map((s) => (
            <li
              key={s.name}
              aria-label={s.name}
              className="flex flex-col gap-1 border-rule-soft border-b py-3"
            >
              <div className="flex flex-wrap items-center gap-2">
                <strong className="text-ink">{s.name}</strong>
                {s.official && <Badge variant="success">The Seed Exchange</Badge>}
                <span className="text-small text-muted">
                  {s.listings} {s.listings === 1 ? 'listing' : 'listings'}
                  {s.refreshedMs > 0
                    ? `, checked ${new Date(s.refreshedMs).toLocaleString()}`
                    : ', not checked yet'}
                </span>
                {!s.official && (
                  <Button
                    size="sm"
                    variant="quiet"
                    className="ml-auto"
                    disabled={remove.isPending}
                    onClick={() => remove.mutate(s.name)}
                  >
                    Remove
                  </Button>
                )}
              </div>
              <code className="break-all text-micro text-muted">{s.url}</code>
              <span className="text-micro text-faint">
                Signed by <code className="break-all">{s.identity}</code> ({s.issuer})
              </span>
              {s.error && (
                <Notice tone="warn" title="The last check failed">
                  {s.error}
                </Notice>
              )}
            </li>
          ))}
        </ul>
      )}
      <Card title="Add a source" headingLevel={3}>
        <form aria-label="Add a source" className="flex flex-col gap-3" onSubmit={submit}>
          <p className="m-0 text-small text-muted">
            agentd keeps a source only once its index verifies against the signing identity you
            give. Removing a source leaves what you installed from it installed.
          </p>
          {field('name', 'Name')}
          {field('url', 'Index URL', 'The index.json URL; its signature sits beside it.')}
          {field('identity', 'Signing identity', 'The certificate identity of the index workflow.')}
          {field('issuer', 'Issuer', "Empty for GitHub Actions' issuer.")}
          <div>
            <Button
              type="submit"
              disabled={add.isPending || !form.name || !form.url || !form.identity}
            >
              {add.isPending ? 'Checking…' : 'Add source'}
            </Button>
          </div>
        </form>
      </Card>
      {error && (
        <Notice tone="danger" title="That failed">
          {error.message}
        </Notice>
      )}
    </div>
  );
}

export const PAGE_VIEWS: Record<Page, (p: PageProps & { api: Api }) => ReactNode> = {
  browse: BrowsePage,
  listing: ListingPage,
  install: InstallPage,
  installed: InstalledPage,
  sources: SourcesPage,
};
