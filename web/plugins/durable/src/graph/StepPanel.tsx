// A step's detail beside the graph: timing, attempts, input, output, error.

import { Button, Card, Notice, StatusTag, Table } from '@loams/ui';
import { useEffect, useRef } from 'react';
import { type Decoded, decodeValue, type Value } from '../envelope.js';
import { formatMs, STATUS_LABEL } from './ExecutionGraph.js';
import type { Attempt, Step, StepStatus } from './model.js';

export function statusTone(s: StepStatus) {
  if (s === 'succeeded') return 'done' as const;
  if (s === 'failed') return 'failed' as const;
  if (s === 'pending' || s === 'sleeping') return 'planned' as const;
  return 'progress' as const;
}

const when = (ms: number | undefined) => (ms ? new Date(ms).toLocaleTimeString() : '—');

function Payload({ label, value }: { label: string; value: Value | undefined }) {
  const d: Decoded = decodeValue(value);
  return (
    <div className="flex flex-col gap-1" data-payload={label}>
      <div className="text-muted text-small">{label}</div>
      {d.kind === 'empty' ? (
        <span className="text-muted text-body">empty</span>
      ) : (
        <pre className="font-mono text-small overflow-auto max-h-48 m-0 whitespace-pre-wrap break-all">
          {d.kind === 'json' ? d.text : (d.text ?? d.raw)}
        </pre>
      )}
    </div>
  );
}

export function StepPanel({ step, onClose }: { step: Step; onClose: () => void }) {
  const heading = useRef<HTMLHeadingElement>(null);
  // Move focus to the panel when a step is opened, so the detail is read next.
  // oxlint-disable-next-line react/exhaustive-deps -- refocus per opened step
  useEffect(() => {
    heading.current?.focus();
  }, [step.id]);
  const took =
    step.startedAt !== undefined && step.endedAt !== undefined
      ? step.endedAt - step.startedAt
      : undefined;
  return (
    <Card
      headingLevel={3}
      title={
        <span
          ref={heading}
          tabIndex={-1}
          className={/* mono-ok: id */ 'font-mono text-body break-all'}
        >
          {step.name}
        </span>
      }
      actions={
        <Button size="sm" variant="quiet" aria-label="Close step detail" onClick={onClose}>
          Close
        </Button>
      }
    >
      <section className="flex flex-col gap-4" aria-label={`Detail of step ${step.name}`}>
        <dl className="grid grid-cols-2 gap-2 m-0 text-body">
          <div>
            <dt className="text-muted text-small">Status</dt>
            <dd className="m-0">
              <StatusTag status={statusTone(step.status)}>{STATUS_LABEL[step.status]}</StatusTag>
            </dd>
          </div>
          <div>
            <dt className="text-muted text-small">Kind</dt>
            <dd className="m-0">{step.kind}</dd>
          </div>
          <div>
            <dt className="text-muted text-small">Started</dt>
            <dd className="m-0">{when(step.startedAt)}</dd>
          </div>
          <div>
            <dt className="text-muted text-small">Took</dt>
            <dd className="m-0">{formatMs(took)}</dd>
          </div>
          {step.wakeAt !== undefined && (
            <div className="col-span-2">
              <dt className="text-muted text-small">
                {step.kind === 'timer' ? 'Wakes' : 'Deadline'}
              </dt>
              <dd className="m-0">{new Date(step.wakeAt).toLocaleString()}</dd>
            </div>
          )}
          <div className="col-span-2">
            <dt className="text-muted text-small">Id</dt>
            <dd className={/* mono-ok: id */ 'm-0 font-mono text-small break-all'}>{step.id}</dd>
          </div>
        </dl>
        {step.error && (
          <Notice tone={step.status === 'failed' ? 'danger' : 'warn'} title="Error">
            <span className={/* mono-ok: code */ 'font-mono text-small break-all'}>
              {step.error}
            </span>
          </Notice>
        )}
        {step.attempts.length > 0 && (
          <Table<Attempt>
            caption={`Attempts (${step.attempts.length})`}
            rows={step.attempts}
            rowKey={(a) => String(a.n)}
            columns={[
              { key: 'n', header: '#', numeric: true, cell: (a) => a.n },
              {
                key: 'status',
                header: 'Status',
                cell: (a) => (
                  <StatusTag status={statusTone(a.status)}>{STATUS_LABEL[a.status]}</StatusTag>
                ),
              },
              {
                key: 'took',
                header: 'Took',
                numeric: true,
                cell: (a) =>
                  formatMs(a.endedAt === undefined ? undefined : a.endedAt - a.startedAt),
              },
              {
                key: 'error',
                header: 'Error',
                cell: (a) => (
                  <span className={/* mono-ok: code */ 'font-mono text-small'}>
                    {a.error ?? ''}
                  </span>
                ),
              },
            ]}
          />
        )}
        <Payload label="Input" value={step.input} />
        <Payload label="Output" value={step.output} />
      </section>
    </Card>
  );
}
