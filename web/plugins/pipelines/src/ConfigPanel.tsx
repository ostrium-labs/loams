// The node side panel: label, connector config (secret fields write-only), transform
// settings, the node's input and output schemas, and its connections (the keyboard path
// for wiring the graph without dragging handles).

import { Badge, Button, Checkbox, Field, Input, Select, Textarea } from '@loams/ui';
import { useId, useState } from 'react';
import {
  type ConfigField,
  type ConfigValue,
  FIELD_TYPES,
  type FieldMapping,
  type FieldType,
  type PipelineGraph,
  type PipelineNode,
  type Schema,
  TRANSFORM_HINT,
  TRANSFORM_LABEL,
  type TransformKind,
} from './model.js';
import { canConnect, castable, type Lookup, schemaText } from './schema.js';

export interface ConfigPanelProps {
  graph: PipelineGraph;
  node: PipelineNode;
  lookup: Lookup;
  /** The node's merged input schema and its output schema. */
  input: Schema | undefined;
  output: Schema | undefined;
  problems: readonly string[];
  readOnly?: boolean;
  onChange: (patch: Partial<PipelineNode>) => void;
  /** Stores a secret value outside the graph; the panel keeps no copy. */
  onSecret: (key: string, value: string | undefined) => void;
  onConnect: (source: string, target: string) => void;
  onDisconnect: (edgeId: string) => void;
  onRemove: () => void;
}

/** A write-only secret field: shows whether it is set, never what it is. */
export function SecretField({
  field,
  isSet,
  disabled,
  onSave,
}: {
  field: ConfigField;
  isSet: boolean;
  disabled?: boolean;
  onSave: (value: string | undefined) => void;
}) {
  // The draft lives only until it is saved; it is never read back from anywhere.
  const [draft, setDraft] = useState('');
  return (
    <Field
      label={
        <span className="flex items-center gap-2">
          {field.label}
          {field.required && <span className="text-small text-faint"> required</span>}
          <Badge>{isSet ? 'set' : 'not set'}</Badge>
        </span>
      }
      hint={field.hint}
    >
      {(ids) => (
        <div className="flex gap-2">
          <Input
            {...ids}
            type="password"
            autoComplete="new-password"
            spellCheck={false}
            disabled={disabled}
            placeholder={isSet ? 'Enter a new value to replace it' : 'Enter a value'}
            value={draft}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && draft) {
                e.preventDefault();
                onSave(draft);
                setDraft('');
              }
            }}
          />
          <Button
            size="sm"
            disabled={disabled || !draft}
            onClick={() => {
              onSave(draft);
              setDraft('');
            }}
          >
            {isSet ? 'Replace' : 'Save'}
          </Button>
          {isSet && (
            <Button size="sm" variant="quiet" disabled={disabled} onClick={() => onSave(undefined)}>
              Clear
            </Button>
          )}
        </div>
      )}
    </Field>
  );
}

function ConfigInput({
  field,
  value,
  disabled,
  onChange,
}: {
  field: ConfigField;
  value: ConfigValue | undefined;
  disabled?: boolean;
  onChange: (v: ConfigValue | undefined) => void;
}) {
  if (field.kind === 'boolean') {
    return (
      <Checkbox
        label={field.label}
        checked={value === true}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
      />
    );
  }
  const label = (
    <span>
      {field.label}
      {field.required && <span className="ml-2 text-small text-faint"> required</span>}
    </span>
  );
  return (
    <Field label={label} hint={field.hint}>
      {(ids) =>
        field.kind === 'select' ? (
          <Select
            {...ids}
            disabled={disabled}
            value={value === undefined ? '' : String(value)}
            onChange={(e) => onChange(e.target.value || undefined)}
          >
            <option value="">Choose…</option>
            {field.options?.map((o) => (
              <option key={o} value={o}>
                {o}
              </option>
            ))}
          </Select>
        ) : (
          <Input
            {...ids}
            type={field.kind === 'number' ? 'number' : 'text'}
            inputMode={field.kind === 'number' ? 'numeric' : undefined}
            disabled={disabled}
            placeholder={field.placeholder}
            value={value === undefined ? '' : String(value)}
            onChange={(e) => {
              const raw = e.target.value;
              if (raw === '') return onChange(undefined);
              onChange(field.kind === 'number' ? Number(raw) : raw);
            }}
          />
        )
      }
    </Field>
  );
}

const fieldsOf = (s: Schema | undefined) => (s && !s.open ? s.fields : []);

function MapEditor({
  node,
  input,
  disabled,
  onChange,
}: {
  node: PipelineNode;
  input: Schema | undefined;
  disabled?: boolean;
  onChange: (m: FieldMapping[] | undefined) => void;
}) {
  const fields = fieldsOf(input);
  if (fields.length === 0) {
    return <p className="m-0 text-body text-muted">Connect a typed input to map its fields.</p>;
  }
  // No mappings yet means every field passes through under its own name.
  const current = node.mappings ?? fields.map((f) => ({ from: f.name, to: f.name }));
  const byFrom = new Map(current.map((m) => [m.from, m]));
  const set = (from: string, to: string | undefined) => {
    const next = fields
      .map((f) =>
        f.name === from ? { from, to: to ?? '' } : (byFrom.get(f.name) ?? { from: f.name, to: '' }),
      )
      .filter((m) => m.to !== '');
    onChange(next);
  };
  return (
    <table className="w-full text-body">
      <caption className="sr-only">Field mappings</caption>
      <thead>
        <tr className="text-left text-small text-muted">
          <th scope="col">Keep</th>
          <th scope="col">Field</th>
          <th scope="col">Output name</th>
        </tr>
      </thead>
      <tbody>
        {fields.map((f) => {
          const m = byFrom.get(f.name);
          const kept = Boolean(m?.to);
          return (
            <tr key={f.name}>
              <td>
                <input
                  type="checkbox"
                  aria-label={`Keep ${f.name}`}
                  checked={kept}
                  disabled={disabled}
                  onChange={(e) => set(f.name, e.target.checked ? f.name : undefined)}
                />
              </td>
              <td className={/* mono-ok: code */ 'font-mono text-small'}>
                {f.name}:{f.type}
              </td>
              <td>
                <Input
                  aria-label={`Output name for ${f.name}`}
                  disabled={disabled || !kept}
                  value={m?.to ?? ''}
                  onChange={(e) => set(f.name, e.target.value || f.name)}
                />
              </td>
            </tr>
          );
        })}
      </tbody>
    </table>
  );
}

function CastEditor({
  node,
  input,
  disabled,
  onChange,
}: {
  node: PipelineNode;
  input: Schema | undefined;
  disabled?: boolean;
  onChange: (c: Record<string, FieldType>) => void;
}) {
  const fields = fieldsOf(input);
  if (fields.length === 0) {
    return <p className="m-0 text-body text-muted">Connect a typed input to map its schema.</p>;
  }
  const casts = node.casts ?? {};
  return (
    <ul className="m-0 flex list-none flex-col gap-2 p-0">
      {fields.map((f) => (
        <li key={f.name} className="flex items-center justify-between gap-2">
          <label
            htmlFor={`${node.id}-cast-${f.name}`}
            className={/* mono-ok: id */ 'font-mono text-small'}
          >
            {f.name}:{f.type} →
          </label>
          <Select
            id={`${node.id}-cast-${f.name}`}
            disabled={disabled}
            value={casts[f.name] ?? f.type}
            onChange={(e) => {
              const to = e.target.value as FieldType;
              const next = { ...casts };
              if (to === f.type) delete next[f.name];
              else next[f.name] = to;
              onChange(next);
            }}
          >
            {FIELD_TYPES.filter((t) => castable(f.type, t)).map((t) => (
              <option key={t} value={t}>
                {t}
              </option>
            ))}
          </Select>
        </li>
      ))}
    </ul>
  );
}

function Connections({
  graph,
  node,
  readOnly,
  onConnect,
  onDisconnect,
}: Pick<ConfigPanelProps, 'graph' | 'node' | 'readOnly' | 'onConnect' | 'onDisconnect'>) {
  const selectId = useId();
  const [target, setTarget] = useState('');
  const label = (id: string) => graph.nodes.find((n) => n.id === id)?.label ?? id;
  const ins = graph.edges.filter((e) => e.target === node.id);
  const outs = graph.edges.filter((e) => e.source === node.id);
  const candidates =
    node.kind === 'sink' ? [] : graph.nodes.filter((n) => canConnect(graph, node.id, n.id).ok);
  return (
    <section aria-label="Connections" className="flex flex-col gap-2">
      <h3 className="m-0 text-small font-medium text-muted">Connections</h3>
      <ul className="m-0 flex list-none flex-col gap-1 p-0 text-body">
        {ins.map((e) => (
          <li key={e.id} className="flex items-center justify-between gap-2">
            <span>from {label(e.source)}</span>
            {!readOnly && (
              <Button size="sm" variant="quiet" onClick={() => onDisconnect(e.id)}>
                Disconnect from {label(e.source)}
              </Button>
            )}
          </li>
        ))}
        {outs.map((e) => (
          <li key={e.id} className="flex items-center justify-between gap-2">
            <span>to {label(e.target)}</span>
            {!readOnly && (
              <Button size="sm" variant="quiet" onClick={() => onDisconnect(e.id)}>
                Disconnect from {label(e.target)}
              </Button>
            )}
          </li>
        ))}
        {ins.length + outs.length === 0 && <li className="text-muted">Not connected.</li>}
      </ul>
      {!readOnly && node.kind !== 'sink' && (
        <div className="flex items-end gap-2">
          <div className="loams-field min-w-0 flex-1">
            <label htmlFor={selectId}>Connect output to</label>
            <Select id={selectId} value={target} onChange={(e) => setTarget(e.target.value)}>
              <option value="">Choose a node…</option>
              {candidates.map((n) => (
                <option key={n.id} value={n.id}>
                  {n.label}
                </option>
              ))}
            </Select>
          </div>
          <Button
            size="sm"
            disabled={!target}
            onClick={() => {
              onConnect(node.id, target);
              setTarget('');
            }}
          >
            Connect
          </Button>
        </div>
      )}
    </section>
  );
}

export function ConfigPanel(props: ConfigPanelProps) {
  const { node, lookup, input, output, problems, readOnly, onChange, onSecret } = props;
  const def = node.kind === 'transform' ? undefined : lookup(node.ref);
  const title =
    node.kind === 'transform'
      ? TRANSFORM_LABEL[node.ref as TransformKind]
      : (def?.name ?? node.ref);
  const setConfig = (key: string, v: ConfigValue | undefined) => {
    const next = { ...node.config };
    if (v === undefined) delete next[key];
    else next[key] = v;
    onChange({ config: next });
  };

  return (
    <aside
      aria-label={`Configure ${node.label}`}
      className="flex min-w-0 flex-col gap-4 overflow-y-auto"
    >
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0">
          <h2 className="m-0 truncate text-emphasis">{node.label}</h2>
          <p className="m-0 text-small text-muted">
            {node.kind} · {title}
            {def?.delivery?.[node.kind as 'source' | 'sink'] &&
              ` · ${def.delivery[node.kind as 'source' | 'sink']}`}
          </p>
        </div>
        {!readOnly && (
          <Button size="sm" variant="danger" onClick={props.onRemove}>
            Remove node
          </Button>
        )}
      </div>

      {problems.length > 0 && (
        <ul
          className="m-0 flex list-none flex-col gap-1 p-0 text-body text-danger"
          aria-label="Problems with this node"
        >
          {problems.map((p) => (
            <li key={p}>{p}</li>
          ))}
        </ul>
      )}

      <fieldset disabled={readOnly} className="m-0 flex flex-col gap-3 border-0 p-0">
        <legend className="sr-only">Settings</legend>
        <Field label="Label">
          {(ids) => (
            <Input
              {...ids}
              value={node.label}
              onChange={(e) => onChange({ label: e.target.value || node.ref })}
            />
          )}
        </Field>

        {def?.config.map((f) =>
          f.kind === 'secret' ? (
            <SecretField
              key={f.key}
              field={f}
              isSet={Boolean(node.secrets[f.key])}
              disabled={readOnly}
              onSave={(v) => onSecret(f.key, v)}
            />
          ) : (
            <ConfigInput
              key={f.key}
              field={f}
              value={node.config[f.key]}
              disabled={readOnly}
              onChange={(v) => setConfig(f.key, v)}
            />
          ),
        )}

        {node.kind === 'transform' && (
          <p className="m-0 text-body text-muted">{TRANSFORM_HINT[node.ref as TransformKind]}</p>
        )}
        {node.ref === 'filter' && node.kind === 'transform' && (
          <Field
            label="Expression"
            hint={`Fields: ${schemaText(input)}. For example: amount > 100 && status == "paid"`}
          >
            {(ids) => (
              <Textarea
                {...ids}
                rows={3}
                className={/* mono-ok: code */ 'font-mono'}
                spellCheck={false}
                value={String(node.config.expression ?? '')}
                onChange={(e) => setConfig('expression', e.target.value || undefined)}
              />
            )}
          </Field>
        )}
        {node.ref === 'map' && node.kind === 'transform' && (
          <MapEditor
            node={node}
            input={input}
            disabled={readOnly}
            onChange={(m) => onChange({ mappings: m })}
          />
        )}
        {node.ref === 'schema' && node.kind === 'transform' && (
          <CastEditor
            node={node}
            input={input}
            disabled={readOnly}
            onChange={(c) => onChange({ casts: c })}
          />
        )}
      </fieldset>

      <dl className="m-0 grid grid-cols-[auto_1fr] gap-x-3 gap-y-1 text-small">
        {node.kind !== 'source' && (
          <>
            <dt className="text-muted">Input</dt>
            <dd className={/* mono-ok: code */ 'm-0 break-words font-mono'}>{schemaText(input)}</dd>
          </>
        )}
        {node.kind !== 'sink' && (
          <>
            <dt className="text-muted">Output</dt>
            <dd className={/* mono-ok: code */ 'm-0 break-words font-mono'}>
              {schemaText(output)}
            </dd>
          </>
        )}
        {node.kind === 'sink' && def?.accepts && (
          <>
            <dt className="text-muted">Accepts</dt>
            <dd className={/* mono-ok: code */ 'm-0 break-words font-mono'}>
              {def.accepts.open
                ? 'any record'
                : def.accepts.fields
                    .map((f) => `${f.name}:${f.type}${f.required ? '*' : ''}`)
                    .join(', ')}
            </dd>
          </>
        )}
      </dl>

      <Connections {...props} />
    </aside>
  );
}
