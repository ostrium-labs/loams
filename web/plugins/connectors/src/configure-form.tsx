import type { ConnectorDetail, ConnectorSummary, LoamsDesktopApi } from '@loams/desktop/contracts';
import { Button, Card, Field, Input, Notice, Select, Textarea } from '@loams/ui';
import { useEffect, useMemo, useState } from 'react';
import {
  buildConfig,
  buildFields,
  exportYaml,
  type FieldDef,
  fieldForPointer,
  secretPlaceholder,
  type Values,
} from './form.js';

type Validation = { valid: boolean; errors: { path: string; message: string }[] };

function FieldInput({
  f,
  values,
  set,
  error,
}: {
  f: FieldDef;
  values: Values;
  set: (id: string, v: string) => void;
  error?: string;
}) {
  const value = values[f.id] ?? '';
  const def = f.default === undefined ? undefined : String(f.default);
  const hint = (
    <>
      {f.description}
      {f.secret && (
        <>
          {' '}
          Never stored; exported as <code>{secretPlaceholder(f.id)}</code>.
        </>
      )}
    </>
  );
  const label = (
    <>
      <span className="font-mono">{f.key}</span>
      {f.required && <span aria-hidden="true"> *</span>}
    </>
  );
  return (
    <Field label={label} hint={hint} error={error}>
      {(a) => {
        if (f.kind === 'enum' || f.kind === 'boolean') {
          const opts = f.kind === 'boolean' ? ['true', 'false'] : (f.enum ?? []).map(String);
          return (
            <Select
              {...a}
              required={f.required}
              value={value}
              onChange={(e) => set(f.id, e.target.value)}
            >
              <option value="">{def === undefined ? 'Not set' : `Default (${def})`}</option>
              {opts.map((o) => (
                <option key={o} value={o}>
                  {o}
                </option>
              ))}
            </Select>
          );
        }
        if (f.kind === 'array') {
          return (
            <Textarea
              {...a}
              required={f.required}
              rows={3}
              placeholder="One per line"
              value={value}
              onChange={(e) => set(f.id, e.target.value)}
            />
          );
        }
        if (f.secret) {
          return (
            <Input
              {...a}
              type="password"
              autoComplete="off"
              spellCheck={false}
              required={f.required}
              value={value}
              onChange={(e) => set(f.id, e.target.value)}
            />
          );
        }
        return (
          <Input
            {...a}
            type={f.kind === 'string' ? 'text' : 'number'}
            required={f.required}
            placeholder={def}
            value={value}
            onChange={(e) => set(f.id, e.target.value)}
          />
        );
      }}
    </Field>
  );
}

function Fields({
  fields,
  values,
  set,
  errors,
}: {
  fields: FieldDef[];
  values: Values;
  set: (id: string, v: string) => void;
  errors: Record<string, string>;
}) {
  return (
    <div className="flex flex-col gap-4">
      {fields.map((f) =>
        f.children ? (
          <fieldset key={f.id} className="m-0 border border-rule p-4">
            <legend className="px-2 font-mono text-sm">
              {f.key}
              {f.required && <span aria-hidden="true"> *</span>}
            </legend>
            {f.description && <p className="mb-3 text-sm text-muted">{f.description}</p>}
            <Fields fields={f.children} values={values} set={set} errors={errors} />
          </fieldset>
        ) : (
          <FieldInput key={f.id} f={f} values={values} set={set} error={errors[f.id]} />
        ),
      )}
    </div>
  );
}

/** The Configure card: a form generated from the connector's JSON Schema, validated by the main process. */
export function ConfigureForm({
  desktop,
  summary,
  detail,
}: {
  desktop: LoamsDesktopApi;
  summary: ConnectorSummary;
  detail: ConnectorDetail;
}) {
  const secrets = useMemo(() => {
    const s = detail.manifest.secrets;
    return Array.isArray(s) ? (s as string[]) : [];
  }, [detail]);
  const fields = useMemo(() => buildFields(detail.schema, secrets), [detail, secrets]);
  // Form state only. Nothing here is written to storage; secret values stay in memory.
  const [values, setValues] = useState<Values>({});
  const [name, setName] = useState(`my-${summary.id}`);
  const [validation, setValidation] = useState<Validation>();
  const [submitted, setSubmitted] = useState(false);
  const [note, setNote] = useState<string>();
  const set = (id: string, v: string) => setValues((cur) => ({ ...cur, [id]: v }));

  const config = useMemo(() => buildConfig(fields, values), [fields, values]);
  const yaml = useMemo(
    () => exportYaml({ connector: summary.id, name: name.trim() || summary.id, config }),
    [summary.id, name, config],
  );

  useEffect(() => {
    let live = true;
    desktop.connectors
      .validate(summary.id, config)
      .then((r) => {
        if (live)
          setValidation(
            r.ok ? r.value : { valid: false, errors: [{ path: '', message: r.message }] },
          );
      })
      .catch((e) => {
        if (live) {
          setValidation({
            valid: false,
            errors: [{ path: '', message: e instanceof Error ? e.message : String(e) }],
          });
        }
      });
    return () => {
      live = false;
    };
  }, [desktop, summary.id, config]);

  const errors = useMemo(() => {
    const out: Record<string, string> = {};
    for (const e of validation?.errors ?? []) {
      const id = fieldForPointer(fields, e.path);
      if (id && !out[id]) out[id] = e.message;
    }
    return out;
  }, [validation, fields]);
  const general = (validation?.errors ?? []).filter((e) => !fieldForPointer(fields, e.path));

  if (summary.stub || fields.length === 0) {
    return (
      <Card title="Configure">
        <Notice title="No configuration fields yet">
          This connector is planned. Its config schema is a placeholder that declares no field, so
          there is nothing to configure until the connector ships.
        </Notice>
      </Card>
    );
  }

  const ready = () => {
    setSubmitted(true);
    if (validation?.valid) return true;
    setNote('Fix the highlighted fields first.');
    return false;
  };
  const copy = async () => {
    if (!ready()) return;
    await desktop.shell.clipboardWrite(yaml);
    setNote('YAML copied.');
  };
  const save = () => {
    if (!ready()) return;
    const url = URL.createObjectURL(new Blob([yaml], { type: 'text/yaml' }));
    const a = document.createElement('a');
    a.href = url;
    a.download = `${name.trim() || summary.id}.yaml`;
    a.click();
    URL.revokeObjectURL(url);
    setNote('YAML saved.');
  };

  return (
    <Card
      title="Configure"
      actions={
        <div className="flex items-center gap-2">
          <Button size="sm" onClick={() => void copy()}>
            Copy YAML
          </Button>
          <Button size="sm" variant="primary" onClick={save}>
            Save YAML file
          </Button>
        </div>
      }
    >
      <div className="grid gap-6 lg:grid-cols-2">
        <form
          className="flex min-w-0 flex-col gap-4"
          aria-label={`Configure ${summary.name}`}
          onSubmit={(e) => e.preventDefault()}
          autoComplete="off"
        >
          <Field label="Instance name">
            {(a) => <Input {...a} value={name} onChange={(e) => setName(e.target.value)} />}
          </Field>
          <Fields fields={fields} values={values} set={set} errors={submitted ? errors : {}} />
        </form>
        <div className="flex min-w-0 flex-col gap-3">
          <div className="flex items-center justify-between">
            <strong className="text-sm">Instance YAML</strong>
            <span className="text-xs text-muted" aria-live="polite">
              {validation === undefined
                ? 'Validating…'
                : validation.valid
                  ? 'Valid'
                  : `${validation.errors.length} problem(s)`}
            </span>
          </div>
          <section aria-label="Instance YAML">
            <pre className="m-0 max-h-96 overflow-auto border border-rule bg-surface p-3 font-mono text-xs">
              {yaml}
            </pre>
          </section>
          {secrets.length > 0 && (
            <p className="text-xs text-muted">
              Secret fields are never stored or exported. Create them in the namespace secret store
              under the names shown; the YAML refers to them as <code>{'${secret:<name>}'}</code>.
            </p>
          )}
          {submitted && general.length > 0 && (
            <Notice tone="danger" title="The config is not valid">
              {general.map((g) => g.message).join('; ')}
            </Notice>
          )}
          {note && (
            <p className="text-xs text-muted" role="status">
              {note}
            </p>
          )}
        </div>
      </div>
    </Card>
  );
}
