import { Badge, Empty, formatRelative, Meter } from '@loams/ui';
import type { PgOperation } from './types.js';

const STATE_LABEL: Record<PgOperation['state'], string> = {
  pending: 'Pending',
  awaiting_approval: 'Awaiting approval',
  running: 'Running',
  succeeded: 'Succeeded',
  failed: 'Failed',
  canceled: 'Canceled',
};

/** What an operation acts on, by the names the page knows. */
function targetText(op: PgOperation, names: Record<string, string>): string {
  return Object.entries(op.target)
    .map(([k, v]) => `${k} ${names[v] ?? v}`)
    .join(', ');
}

/** The namespace's control-plane operations, newest first, with their progress. */
export function Operations({
  operations,
  names,
}: {
  operations: PgOperation[];
  /** Resource id to display name, for the target column. */
  names: Record<string, string>;
}) {
  if (operations.length === 0) {
    return <Empty title="No operations">Creates and deletes show their progress here.</Empty>;
  }
  return (
    <div className="loams-table-wrap">
      <table className="loams-table">
        <caption className="sr-only">Operations</caption>
        <thead>
          <tr>
            <th scope="col">Operation</th>
            <th scope="col">Target</th>
            <th scope="col">State</th>
            <th scope="col">Progress</th>
            <th scope="col">Updated</th>
          </tr>
        </thead>
        <tbody>
          {operations.map((op) => {
            const p = op.progress;
            const label = `${op.kind}: ${p ? `${p.done} of ${p.total} ${p.unit}` : STATE_LABEL[op.state]}`;
            return (
              <tr key={op.id} data-operation={op.id} data-state={op.state}>
                <td>
                  <span className={/* mono-ok: id */ 'font-mono text-small'}>{op.kind}</span>
                  <span className="block text-small text-faint">{op.id}</span>
                </td>
                <td className="text-body">{targetText(op, names)}</td>
                <td>
                  <Badge>{STATE_LABEL[op.state]}</Badge>
                  {op.error && (
                    <span className="block text-small text-danger" role="note">
                      {op.error.code}: {op.error.message}
                    </span>
                  )}
                </td>
                <td style={{ minWidth: '9rem' }}>
                  {p && p.total > 0 ? (
                    <>
                      <Meter value={p.done} max={p.total} label={label} />
                      <span className="text-small text-muted">
                        {p.done}/{p.total} {p.unit}
                        {p.phase ? ` · ${p.phase}` : ''}
                      </span>
                    </>
                  ) : (
                    <span className="text-small text-muted">—</span>
                  )}
                </td>
                <td className="text-small text-muted">
                  <time dateTime={op.updatedAt}>{formatRelative(op.updatedAt)}</time>
                </td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}
