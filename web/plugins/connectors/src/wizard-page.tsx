// `/connectors/:id/new`: the create-connector wizard (CN2). Four steps: basics (name, direction,
// runtime), configure (a form generated from the connector's JSON Schema, validated by the main
// process), Fabric binding, and review. Secret fields are write-only: typed into password inputs,
// held in component state, sent once to `FlowClient.createInstance` and then cleared. The review
// step shows only whether each secret was set.

import type { ConnectorDetail, LoamsDesktopApi } from '@loams/desktop/contracts';
import { Button, Card, Checkbox, Field, Input, Notice, Select } from '@loams/ui';
import { useCallback, useEffect, useMemo, useState } from 'react';
import { runtimeClass, useCatalog } from './catalog-page.js';
import { Fields } from './configure-form.js';
import type { Direction, FabricBinding, FlowClient, RuntimeClass } from './flow-client.js';
import { NAME_RE } from './flow-client.js';
import {
  buildConfig,
  buildFields,
  fieldForPointer,
  leaves,
  secretValues,
  type Values,
} from './form.js';
import { FixtureNotice } from './instances-page.js';
import { PageHead } from './page-head.js';

export const STEPS = ['Basics', 'Configure', 'Fabric binding', 'Review'] as const;

type Validation = { valid: boolean; errors: { path: string; message: string }[] };
type BindingKind = FabricBinding['kind'];

export function bindingOf(kind: BindingKind, target: string, partitions: string): FabricBinding {
  const p = Number(partitions);
  const parts = Number.isInteger(p) && p > 0 ? { partitions: p } : {};
  if (kind === 'topic') return { kind, topic: target.trim(), ...parts };
  if (kind === 'table') return { kind, table: target.trim(), ...parts };
  return { kind, collection: target.trim() };
}

export function WizardPage({
  desktop,
  flow,
  id,
  navigate,
}: {
  desktop: LoamsDesktopApi;
  flow: FlowClient;
  id: string;
  navigate: (to: string) => void;
}) {
  const catalog = useCatalog(desktop);
  const summary = catalog.state === 'ready' ? catalog.data.find((c) => c.id === id) : undefined;
  const [detail, setDetail] = useState<ConnectorDetail>();
  const [loadError, setLoadError] = useState<string>();
  useEffect(() => {
    let live = true;
    desktop.connectors
      .get(id)
      .then((r) => {
        if (!live) return;
        if (r.ok) setDetail(r.value);
        else setLoadError(r.message);
      })
      .catch((e) => live && setLoadError(e instanceof Error ? e.message : String(e)));
    return () => {
      live = false;
    };
  }, [desktop, id]);

  const directions = useMemo<Direction[]>(
    () =>
      summary
        ? ([summary.source && 'source', summary.sink && 'sink'].filter(Boolean) as Direction[])
        : [],
    [summary],
  );

  const [step, setStep] = useState(0);
  const [name, setName] = useState(`my-${id}`);
  const [direction, setDirection] = useState<Direction | ''>('');
  const [runtime, setRuntime] = useState<RuntimeClass | ''>('');
  const [values, setValues] = useState<Values>({});
  const [kind, setKind] = useState<BindingKind>('topic');
  const [target, setTarget] = useState('');
  const [partitions, setPartitions] = useState('');
  const [enabled, setEnabled] = useState(true);
  // Each result is bound to the config object it validated, so a stale result never stands in
  // for the current values while a newer validation is pending.
  const [checked, setChecked] = useState<{ config: unknown; result: Validation }>();
  const [stepError, setStepError] = useState<string>();
  const [busy, setBusy] = useState(false);

  const dir: Direction | '' = direction || directions[0] || '';
  const rt: RuntimeClass = runtime || (summary ? runtimeClass(summary) : 'native');

  const secrets = useMemo(() => {
    const s = detail?.manifest.secrets;
    return Array.isArray(s) ? (s as string[]) : [];
  }, [detail]);
  const fields = useMemo(
    () => (detail ? buildFields(detail.schema, secrets) : []),
    [detail, secrets],
  );
  const config = useMemo(() => buildConfig(fields, values), [fields, values]);
  const set = (fid: string, v: string) => setValues((cur) => ({ ...cur, [fid]: v }));

  useEffect(() => {
    if (!detail) return;
    let live = true;
    desktop.connectors
      .validate(id, config)
      .then((r) => {
        if (live)
          setChecked({
            config,
            result: r.ok ? r.value : { valid: false, errors: [{ path: '', message: r.message }] },
          });
      })
      .catch((e) => {
        if (live) {
          setChecked({
            config,
            result: {
              valid: false,
              errors: [{ path: '', message: e instanceof Error ? e.message : String(e) }],
            },
          });
        }
      });
    return () => {
      live = false;
    };
  }, [desktop, id, config, detail]);

  const validation = checked?.config === config ? checked.result : undefined;

  const fieldErrors = useMemo(() => {
    const out: Record<string, string> = {};
    for (const e of validation?.errors ?? []) {
      const fid = fieldForPointer(fields, e.path);
      if (fid && !out[fid]) out[fid] = e.message;
    }
    return out;
  }, [validation, fields]);

  // Move focus to the step heading whenever one mounts: the heading is keyed by step, so this runs
  // once when the form first renders and again on each step change, landing keyboard and
  // screen-reader users on the new content.
  const focusHeading = useCallback((el: HTMLHeadingElement | null) => {
    el?.focus();
  }, []);

  const back = (
    <Button size="sm" variant="quiet" onClick={() => navigate(`/connectors/${id}`)}>
      Back to connector
    </Button>
  );

  if (loadError || (catalog.state === 'ready' && !summary)) {
    return (
      <div className="lc-page">
        <PageHead title="New instance" actions={back} />
        <Notice tone="danger" title={loadError ?? `No connector "${id}".`} />
      </div>
    );
  }
  if (!summary || !detail) {
    return (
      <div className="lc-page">
        <PageHead title="New instance" actions={back} />
        <p className="text-body text-muted">
          {catalog.state === 'error' ? catalog.message : 'Loading…'}
        </p>
      </div>
    );
  }
  if (summary.stub || fields.length === 0) {
    return (
      <div className="lc-page">
        <PageHead title={`New ${summary.name} instance`} actions={back} />
        <Notice title="Not available yet">
          This connector is planned: its config schema declares no field, so it cannot be created.
        </Notice>
      </div>
    );
  }

  const check = (s: number): string | undefined => {
    if (s === 0) {
      if (!NAME_RE.test(name))
        return 'Use lowercase letters, digits and dashes, starting with a letter.';
      if (!dir) return 'This connector declares neither a source nor a sink side.';
    }
    if (s === 1 && !validation?.valid) {
      return validation === undefined
        ? 'Still validating; try again.'
        : 'Fix the highlighted fields first.';
    }
    if (s === 2 && target.trim() === '') return `Enter the ${kind} to bind.`;
    return undefined;
  };

  const next = () => {
    const err = check(step);
    setStepError(err);
    if (!err) setStep((s) => Math.min(s + 1, STEPS.length - 1));
  };
  const prev = () => {
    setStepError(undefined);
    setStep((s) => Math.max(s - 1, 0));
  };

  const create = async () => {
    if (!dir) return;
    const err = check(0) ?? check(1) ?? check(2);
    if (err) {
      setStepError(err);
      return;
    }
    setBusy(true);
    setStepError(undefined);
    try {
      const r = await flow.createInstance({
        connector: id,
        name,
        direction: dir,
        runtime: rt,
        fabric: bindingOf(kind, target, partitions),
        config,
        secrets: secretValues(fields, values),
        enabled,
      });
      if (!r.ok) {
        setStepError(r.message);
        return;
      }
      // Secret values leave memory with the form state.
      setValues({});
      navigate(`/connectors/instances/${encodeURIComponent(r.value.name)}`);
    } catch (e) {
      setStepError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  const secretLeaves = leaves(fields).filter((f) => f.secret);

  return (
    <div className="lc-page">
      <PageHead
        title={`New ${summary.name} instance`}
        subtitle={<span className={/* mono-ok: id */ 'font-mono'}>{summary.id}</span>}
        actions={back}
      />
      <FixtureNotice flow={flow} />
      <nav aria-label="Wizard steps" className="mb-4">
        <ol className="m-0 flex list-none flex-wrap gap-2 p-0 text-body">
          {STEPS.map((s, i) => (
            <li
              key={s}
              aria-current={i === step ? 'step' : undefined}
              className={i === step ? 'font-semibold' : 'text-muted'}
            >
              {i + 1}. {s}
            </li>
          ))}
        </ol>
      </nav>
      <Card>
        {/* noValidate: the schema validator owns validation; native checks would block on a
            required field inside an optional object the user left empty. */}
        <form
          aria-label={`New instance: ${STEPS[step]}`}
          autoComplete="off"
          noValidate
          className="flex flex-col gap-4"
          onSubmit={(e) => {
            e.preventDefault();
            if (step < STEPS.length - 1) next();
            else void create();
          }}
        >
          <h2
            key={step}
            ref={focusHeading}
            tabIndex={-1}
            className="m-0 text-emphasis outline-none"
          >
            {STEPS[step]}
          </h2>

          {step === 0 && (
            <>
              <Field label="Instance name" hint="Lowercase letters, digits and dashes.">
                {(a) => (
                  <Input {...a} required value={name} onChange={(e) => setName(e.target.value)} />
                )}
              </Field>
              <fieldset className="m-0 flex flex-col gap-2 border-0 p-0">
                <legend className="mb-1 text-body">Direction</legend>
                {directions.map((d) => (
                  <label key={d} className="loams-check">
                    <input
                      type="radio"
                      name="direction"
                      value={d}
                      checked={dir === d}
                      onChange={() => setDirection(d)}
                    />
                    {d === 'source'
                      ? 'Source: read the system into the Fabric'
                      : 'Sink: write the Fabric to the system'}
                  </label>
                ))}
              </fieldset>
              <Field
                label="Runtime"
                hint={
                  runtimeClass(summary) === 'native'
                    ? 'Native: the first-party connector. Fallback runs the declared Camel, Debezium or Iggy runtime.'
                    : 'This connector runs on a declared fallback runtime until its native replacement ships.'
                }
              >
                {(a) => (
                  <Select
                    {...a}
                    value={rt}
                    onChange={(e) => setRuntime(e.target.value as RuntimeClass)}
                  >
                    {runtimeClass(summary) === 'native' && <option value="native">native</option>}
                    <option value="fallback">fallback</option>
                  </Select>
                )}
              </Field>
            </>
          )}

          {step === 1 && (
            <>
              {secretLeaves.length > 0 && (
                <p className="m-0 text-small text-muted">
                  Secret fields are write-only: sent once to the namespace secret store, never shown
                  again.
                </p>
              )}
              <Fields
                fields={fields}
                values={values}
                set={set}
                errors={stepError ? fieldErrors : {}}
              />
            </>
          )}

          {step === 2 && (
            <>
              <Field label="Bind to">
                {(a) => (
                  <Select
                    {...a}
                    value={kind}
                    onChange={(e) => setKind(e.target.value as BindingKind)}
                  >
                    <option value="topic">Fabric topic</option>
                    <option value="table">Fluss table</option>
                    <option value="collection">Loams collection</option>
                  </Select>
                )}
              </Field>
              <Field label={kind === 'topic' ? 'Topic' : kind === 'table' ? 'Table' : 'Collection'}>
                {(a) => (
                  <Input
                    {...a}
                    required
                    value={target}
                    onChange={(e) => setTarget(e.target.value)}
                  />
                )}
              </Field>
              {kind !== 'collection' && (
                <Field label="Partitions" hint="0 or empty keeps the target's default.">
                  {(a) => (
                    <Input
                      {...a}
                      type="number"
                      min={0}
                      value={partitions}
                      onChange={(e) => setPartitions(e.target.value)}
                    />
                  )}
                </Field>
              )}
            </>
          )}

          {step === 3 && (
            <>
              <dl className="m-0 grid grid-cols-[10rem_1fr] gap-2 text-body">
                <dt className="text-muted">Name</dt>
                <dd className={/* mono-ok: id */ 'm-0 font-mono'}>{name}</dd>
                <dt className="text-muted">Direction</dt>
                <dd className="m-0">{dir}</dd>
                <dt className="text-muted">Runtime</dt>
                <dd className="m-0">{rt}</dd>
                <dt className="text-muted">Fabric</dt>
                <dd className={/* mono-ok: id */ 'm-0 font-mono'}>
                  {kind} {target}
                  {partitions && kind !== 'collection' ? ` (${partitions} partitions)` : ''}
                </dd>
                {secretLeaves.map((f) => (
                  <div key={f.id} className="contents">
                    <dt className="text-muted">Secret {f.id}</dt>
                    <dd className="m-0">{(values[f.id] ?? '') !== '' ? 'set' : 'not set'}</dd>
                  </div>
                ))}
              </dl>
              <section aria-label="Config">
                <pre className="m-0 max-h-80 overflow-auto border border-rule bg-surface p-3 font-mono text-small">
                  {JSON.stringify(config, null, 2)}
                </pre>
              </section>
              <Checkbox
                label="Start the instance after creating it"
                checked={enabled}
                onChange={(e) => setEnabled(e.target.checked)}
              />
            </>
          )}

          {stepError && (
            <Notice tone="danger" title="Cannot continue">
              {stepError}
            </Notice>
          )}

          <div className="flex justify-between gap-2">
            <Button type="button" variant="quiet" onClick={prev} disabled={step === 0}>
              Previous
            </Button>
            {step < STEPS.length - 1 ? (
              <Button type="submit" variant="primary">
                Next
              </Button>
            ) : (
              <Button type="submit" variant="primary" disabled={busy}>
                {busy ? 'Creating…' : 'Create instance'}
              </Button>
            )}
          </div>
        </form>
      </Card>
    </div>
  );
}
