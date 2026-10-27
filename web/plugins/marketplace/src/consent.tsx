// Install is consent (design §73 §7.2, D1466; plan MK1 Task 9). The screen is made
// from agentd's `PlanInstall` and shows every permission in it: the Place's data
// scopes, the tools by risk, the approvals that always ask, the egress hosts, the
// dependencies it installs or reuses, the secrets to supply, the models and the
// proposed budgets, and the licence, tier, provenance and the version's age, as a
// "can / can't" list. Install sends exactly the plan's digest, so agentd refuses
// (`plan_changed`) if the listing changed after the person read it. Nothing is
// created before they confirm.

import type { InstallPlan } from '@loams/agentd-client';
import { Badge, Button, Card, Input, Notice } from '@loams/ui';
import { type FormEvent, type ReactNode, useId, useState } from 'react';
import type { MarketplaceError } from './errors.js';
import {
  frameworkLabel,
  kindLabel,
  languageLabel,
  levelLabel,
  riskLabel,
  riskVariant,
  tierLabel,
  tierVariant,
} from './labels.js';

type Json = Record<string, unknown>;

const obj = (v: unknown): Json | undefined =>
  v !== null && typeof v === 'object' && !Array.isArray(v) ? (v as Json) : undefined;
const list = (v: unknown): unknown[] => (Array.isArray(v) ? v : []);
const text = (v: unknown): string | undefined =>
  typeof v === 'string' && v !== '' ? v : typeof v === 'number' ? String(v) : undefined;

/** The Place's data scopes, in words. */
export function dataScopes(place: Json | null): string[] {
  return list(place?.data).flatMap((d) => {
    const scope = obj(d);
    const name = text(scope?.name);
    if (!name) return [];
    const access = scope?.access === 'write' ? 'Read and write' : 'Read';
    return [`${access} the ${text(scope?.kind) ?? 'data'} ${name}`];
  });
}

const strings = (v: unknown): string[] =>
  list(v).flatMap((s) => (text(s) ? [text(s) as string] : []));

function Section({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section aria-label={title} className="flex flex-col gap-2">
      <h3 className="m-0 text-small font-semibold text-ink">{title}</h3>
      {children}
    </section>
  );
}

function Items({ label, items, empty }: { label: string; items: ReactNode[]; empty: string }) {
  if (items.length === 0) return <p className="m-0 text-small text-muted">{empty}</p>;
  return (
    <ul aria-label={label} className="m-0 flex list-none flex-col gap-1.5 p-0">
      {items.map((item, i) => (
        // oxlint-disable-next-line react/no-array-index-key -- a fixed list per plan
        <li key={i} className="text-small text-ink">
          {item}
        </li>
      ))}
    </ul>
  );
}

function ageText(days: number | null): string {
  if (days === null) return 'The index does not say when this version was published.';
  if (days === 0) return 'Published today.';
  return `Published ${days} ${days === 1 ? 'day' : 'days'} ago.`;
}

function provenanceText(p: Json | null): ReactNode {
  const type = text(p?.type);
  if (!type) return 'None declared.';
  const ref = text(p?.ref);
  return (
    <>
      {type}
      {ref && (
        <>
          {' '}
          from <code className="break-all">{ref}</code>
        </>
      )}
    </>
  );
}

function modelItems(models: Json | null): string[] {
  if (!models) return [];
  const out: string[] = [];
  const requires = obj(models.requires);
  if (requires?.tools === true) out.push('Needs a model with tool calls.');
  if (requires?.vision === true) out.push('Needs a model with vision.');
  const context = text(requires?.context_min);
  if (context) out.push(`Needs a context window of at least ${context} tokens.`);
  const suggested = obj(models.suggested);
  if (text(suggested?.model)) {
    out.push(`Suggests ${text(suggested?.provider) ?? 'a provider'}'s ${text(suggested?.model)}.`);
  }
  const apis = strings(models.api);
  if (apis.length > 0) out.push(`Speaks ${apis.join(', ')}.`);
  return out;
}

function budgetItems(cost: Json | null): string[] {
  if (!cost) return [];
  const out: string[] = [];
  const n = (k: string) => text(cost[k]);
  if (n('typical_tokens_per_run')) out.push(`About ${n('typical_tokens_per_run')} tokens a run.`);
  if (n('max_tokens_per_run')) out.push(`At most ${n('max_tokens_per_run')} tokens a run.`);
  if (n('typical_runs_per_day')) out.push(`About ${n('typical_runs_per_day')} runs a day.`);
  if (n('budget_usd_per_run')) out.push(`A budget of $${n('budget_usd_per_run')} a run.`);
  if (n('budget_usd_per_day')) out.push(`A budget of $${n('budget_usd_per_day')} a day.`);
  return out;
}

export interface ConsentScreenProps {
  plan: InstallPlan;
  /** `update`: consent to an update that widens a permission (`from` is installed). */
  mode?: 'install' | 'update';
  from?: string;
  pending?: boolean;
  error?: MarketplaceError | undefined;
  /** Confirmed: the secrets typed, by input name. The caller sends `plan.digest`. */
  onConfirm: (secrets: Record<string, string>) => void;
  onCancel: () => void;
  /** Reads the plan again (after `plan_changed`). */
  onReplan: () => void;
}

export function ConsentScreen({
  plan,
  mode = 'install',
  from,
  pending = false,
  error,
  onConfirm,
  onCancel,
  onReplan,
}: ConsentScreenProps) {
  const id = useId();
  const [secrets, setSecrets] = useState<Record<string, string>>({});
  const s = plan.summary;
  const place = plan.place;
  const alwaysAsk = strings(place?.always_ask);
  const roles = strings(place?.roles);
  const revoked = s.revocation?.severity === 'disable';
  const missing = plan.secrets.filter((x) => x.required && !secrets[x.name]?.trim());
  const verb = mode === 'update' ? 'Update' : 'Install';

  const can: ReactNode[] = [
    ...dataScopes(place),
    ...plan.tools.map((t) => (
      <span key={`tool-${t.name}`} className="flex flex-wrap items-baseline gap-1.5">
        <Badge variant={riskVariant(t.risk)}>{riskLabel(t.risk)}</Badge>
        <code>{t.name}</code>
        {t.description && <span className="text-muted">{t.description}</span>}
        {t.alwaysAsks && <Badge variant="outline">Asks you every time</Badge>}
        {t.mcpServer && <span className="text-faint">through {t.mcpServer}</span>}
      </span>
    )),
    ...plan.egress.map((host) => `Reach ${host}`),
    ...roles.map((role) => `Use the role ${role}`),
  ];

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (missing.length > 0 || revoked || pending) return;
    onConfirm(Object.fromEntries(Object.entries(secrets).filter(([, v]) => v.trim() !== '')));
  };

  return (
    <form aria-label={`${verb} ${s.title}`} onSubmit={submit} className="flex flex-col gap-4">
      <header className="flex flex-col gap-1">
        <h2 className="m-0 text-title font-semibold text-ink">
          {verb} {s.title} {plan.version}
        </h2>
        <p className="m-0 flex flex-wrap items-center gap-1.5 text-small text-muted">
          <code>{s.id}</code>
          <Badge>{kindLabel(s.kind)}</Badge>
          <Badge variant={tierVariant(s.tier)}>{tierLabel(s.tier)}</Badge>
          {s.framework && <Badge>{frameworkLabel(s.framework)}</Badge>}
          {s.language && <Badge>{languageLabel(s.language)}</Badge>}
          {s.level && <Badge variant="accent">{levelLabel(s.level)}</Badge>}
          <span>from {s.source}</span>
        </p>
        {mode === 'update' && from && (
          <p className="m-0 text-small text-ink">
            {from} is installed. {plan.version} asks for more, so it needs your consent again.
          </p>
        )}
      </header>

      {s.revocation && (
        <Notice tone={revoked ? 'danger' : 'warn'} title={revoked ? 'Revoked' : 'Advisory'}>
          {s.revocation.reason} (versions {s.revocation.versions})
          {s.revocation.advisory && (
            <>
              {' '}
              <a href={s.revocation.advisory} target="_blank" rel="noreferrer">
                Read the advisory
              </a>
            </>
          )}
        </Notice>
      )}

      <div className="grid gap-4 md:grid-cols-2">
        <Card title="It can" headingLevel={3}>
          <Items label="It can" items={can} empty="Nothing beyond running." />
        </Card>
        <Card title="It can't" headingLevel={3}>
          <Items
            label="It can't"
            items={[
              ...plan.cannot,
              ...(plan.tools.length > 0 ? ['Call any tool not listed here.'] : []),
            ]}
            empty="The plan names nothing it is refused."
          />
        </Card>
      </div>

      <Section title="Asks you every time">
        <Items
          label="Asks you every time"
          items={alwaysAsk.map((t) => (
            <span key={t}>
              Before <code>{t}</code>, whatever the thread's approval mode
            </span>
          ))}
          empty="No tool always asks; the thread's approval mode decides."
        />
      </Section>

      <Section title="Network">
        <Items
          label="Network"
          items={plan.egress.map((h) => (
            <code key={h}>{h}</code>
          ))}
          empty="No hosts: it reaches only agentd's model proxy."
        />
      </Section>

      <Section title="Installs or reuses">
        <Items
          label="Installs or reuses"
          items={plan.dependencies.map((d) => (
            <span key={d.id}>
              <code>{d.id}</code> {d.version}:{' '}
              {d.installed ? 'already installed, reused' : 'installed with it'}
            </span>
          ))}
          empty="No other listings."
        />
      </Section>

      <Section title="Secrets to supply">
        {plan.secrets.length === 0 ? (
          <p className="m-0 text-small text-muted">None.</p>
        ) : (
          <div className="flex flex-col gap-2">
            <p className="m-0 text-small text-muted">
              They go to agentd's vault (the system keyring), never to a file or to the page.
            </p>
            {plan.secrets.map((x) => (
              <label
                key={x.name}
                className="flex flex-col gap-1 text-small"
                htmlFor={`${id}-${x.name}`}
              >
                <span className="text-ink">
                  <code>{x.name}</code> {x.required ? '(required)' : '(optional)'}
                  {x.description && <span className="text-muted"> {x.description}</span>}
                </span>
                <Input
                  id={`${id}-${x.name}`}
                  type="password"
                  autoComplete="off"
                  value={secrets[x.name] ?? ''}
                  required={x.required}
                  onChange={(e) => setSecrets((all) => ({ ...all, [x.name]: e.target.value }))}
                />
              </label>
            ))}
          </div>
        )}
      </Section>

      <Section title="Models">
        <Items
          label="Models"
          items={modelItems(plan.models)}
          empty="No model requirements declared. It uses the model you choose, through agentd's model proxy."
        />
      </Section>

      <Section title="Budgets">
        <Items label="Budgets" items={budgetItems(plan.cost)} empty="No budget proposed." />
        <p className="m-0 text-micro text-faint">
          Proposed by the listing. Editing them waits for agentd's per-install budgets.
        </p>
      </Section>

      <Section title="About this version">
        <dl className="m-0 grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-small">
          <dt className="text-muted">Licence</dt>
          <dd className="m-0 text-ink">{plan.licence || 'Not declared'}</dd>
          <dt className="text-muted">Tier</dt>
          <dd className="m-0 text-ink">{tierLabel(s.tier)}</dd>
          <dt className="text-muted">Provenance</dt>
          <dd className="m-0 text-ink">{provenanceText(plan.provenance)}</dd>
          <dt className="text-muted">Source</dt>
          <dd className="m-0 text-ink">
            {plan.sourceRepository ? (
              <a href={plan.sourceRepository} target="_blank" rel="noreferrer">
                {plan.sourceRepository}
              </a>
            ) : (
              'Not declared'
            )}
          </dd>
          <dt className="text-muted">Version</dt>
          <dd className="m-0 text-ink">
            {plan.version}. {ageText(plan.versionAgeDays)}
          </dd>
        </dl>
      </Section>

      {error && (
        <Notice tone="danger" title={`${verb} failed`}>
          {error.message}
          {error.reason === 'plan_changed' || error.reason === 'consent_required' ? (
            <>
              {' '}
              <Button type="button" variant="secondary" size="sm" onClick={onReplan}>
                Read the new plan
              </Button>
            </>
          ) : null}
        </Notice>
      )}

      <footer className="flex flex-wrap items-center gap-2">
        <Button type="submit" disabled={pending || revoked || missing.length > 0}>
          {pending ? `${verb === 'Install' ? 'Installing' : 'Updating'}…` : verb}
        </Button>
        <Button type="button" variant="quiet" onClick={onCancel}>
          Cancel
        </Button>
        <span className="text-small text-muted">
          {revoked
            ? 'A revoked version cannot be installed.'
            : missing.length > 0
              ? `Supply ${missing.map((m) => m.name).join(', ')} first.`
              : 'Nothing is created until you confirm.'}
        </span>
      </footer>
      <p className="m-0 text-micro text-faint">Plan {plan.digest}</p>
    </form>
  );
}
