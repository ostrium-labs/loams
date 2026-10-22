import {
  Badge,
  Button,
  Checkbox,
  Dialog,
  Field,
  formatBytes,
  formatDate,
  Input,
  Notice,
} from '@loams/ui';
import { useState } from 'react';
import { type BranchRow, branchTree, deleteBlocker, LSN_PATTERN } from './tree.js';
import { type BranchPoint, type CreateBranchInput, failureText, type PgBranch } from './types.js';

type PointMode = 'head' | 'lsn' | 'time';

const STATE_LABEL: Record<PgBranch['state'], string> = {
  creating: 'Creating',
  ready: 'Ready',
  restoring: 'Restoring',
  deleting: 'Deleting',
  failed: 'Failed',
};

export interface BranchTreeProps {
  branches: PgBranch[];
  selected?: string;
  onSelect: (branchId: string) => void;
  onCreate: (input: CreateBranchInput) => Promise<void>;
  onProtect: (branch: PgBranch, value: boolean) => Promise<void>;
  onDelete: (branch: PgBranch) => Promise<void>;
}

/**
 * The project's branches as a tree by parent, with each branch point (LSN and time), its
 * WAL head, the protected flag, and create and delete. Deleting is not offered for the
 * default branch or a branch with children (the server refuses with
 * `branch_has_children`); deleting a protected branch asks for its name.
 */
export function BranchTree({
  branches,
  selected,
  onSelect,
  onCreate,
  onProtect,
  onDelete,
}: BranchTreeProps) {
  const rows = branchTree(branches);
  const [from, setFrom] = useState<PgBranch>();
  const [doomed, setDoomed] = useState<BranchRow>();
  const [rowError, setRowError] = useState<string>();

  return (
    <div className="flex flex-col gap-3">
      {rowError && (
        <Notice tone="danger" title="The branch was not changed">
          {rowError}
        </Notice>
      )}
      <div className="loams-table-wrap">
        <table className="loams-table">
          <caption className="sr-only">Branches</caption>
          <thead>
            <tr>
              <th scope="col">Branch</th>
              <th scope="col">Branch point</th>
              <th scope="col">WAL head</th>
              <th scope="col">Size</th>
              <th scope="col">Protected</th>
              <th scope="col">
                <span className="sr-only">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {rows.map((row) => {
              const { branch: b, depth } = row;
              const blocker = deleteBlocker(row);
              const isSelected = b.id === selected;
              return (
                <tr
                  key={b.id}
                  data-branch={b.id}
                  data-depth={depth}
                  data-selected={isSelected || undefined}
                  className={isSelected ? 'is-selected' : undefined}
                >
                  <td>
                    <span
                      style={{ paddingLeft: `${depth * 1.25}rem` }}
                      className="inline-flex items-center gap-2 whitespace-nowrap"
                    >
                      {depth > 0 && (
                        <span className="text-faint" aria-hidden="true">
                          └
                        </span>
                      )}
                      <Button
                        size="sm"
                        variant="quiet"
                        aria-pressed={isSelected}
                        aria-label={`Open branch ${b.name}`}
                        onClick={() => onSelect(b.id)}
                      >
                        <strong>{b.name}</strong>
                      </Button>
                      {b.isDefault && <Badge>Default</Badge>}
                      {b.state !== 'ready' && <Badge>{STATE_LABEL[b.state]}</Badge>}
                      {b.expireTime && (
                        <Badge title={b.expireTime}>Expires {formatDate(b.expireTime)}</Badge>
                      )}
                    </span>
                  </td>
                  <td>
                    {b.parentId ? (
                      <>
                        <span className={/* mono-ok: id */ 'font-mono text-small'}>
                          {b.parentLsn || '—'}
                        </span>
                        {b.parentTime && (
                          <time dateTime={b.parentTime} className="block text-small text-muted">
                            {new Date(b.parentTime).toLocaleString()}
                          </time>
                        )}
                      </>
                    ) : (
                      <span className="text-small text-muted">Root</span>
                    )}
                  </td>
                  <td className={/* mono-ok: id */ 'font-mono text-small'}>
                    {b.wal?.commitLsn || '—'}
                  </td>
                  <td className="text-small">{formatBytes(b.logicalSizeBytes)}</td>
                  <td>
                    <Checkbox
                      label={<span className="sr-only">Protect {b.name}</span>}
                      checked={b.protected}
                      disabled={b.state === 'deleting'}
                      onChange={async (e) => {
                        setRowError(undefined);
                        try {
                          await onProtect(b, e.currentTarget.checked);
                        } catch (err) {
                          setRowError(failureText(err));
                        }
                      }}
                    />
                  </td>
                  <td>
                    <span className="inline-flex gap-1">
                      <Button
                        size="sm"
                        disabled={b.state !== 'ready'}
                        onClick={() => setFrom(b)}
                        aria-label={`Create branch from ${b.name}`}
                      >
                        Branch
                      </Button>
                      <Button
                        size="sm"
                        variant="danger"
                        disabled={blocker !== undefined}
                        title={blocker}
                        aria-label={`Delete branch ${b.name}`}
                        onClick={() => setDoomed(row)}
                      >
                        Delete
                      </Button>
                    </span>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
      <CreateBranchDialog from={from} onClose={() => setFrom(undefined)} onCreate={onCreate} />
      <DeleteBranchDialog row={doomed} onClose={() => setDoomed(undefined)} onDelete={onDelete} />
    </div>
  );
}

function CreateBranchDialog({
  from,
  onClose,
  onCreate,
}: {
  from: PgBranch | undefined;
  onClose: () => void;
  onCreate: (input: CreateBranchInput) => Promise<void>;
}) {
  const [name, setName] = useState('');
  const [mode, setMode] = useState<PointMode>('head');
  const [lsn, setLsn] = useState('');
  const [time, setTime] = useState('');
  const [prot, setProt] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();

  const close = () => {
    setName('');
    setMode('head');
    setLsn('');
    setTime('');
    setProt(false);
    setError(undefined);
    onClose();
  };
  const lsnBad = mode === 'lsn' && lsn.trim() !== '' && !LSN_PATTERN.test(lsn.trim());
  const ready =
    name.trim() !== '' &&
    (mode === 'head' ||
      (mode === 'lsn' && LSN_PATTERN.test(lsn.trim())) ||
      (mode === 'time' && time !== '' && !Number.isNaN(new Date(time).getTime())));

  async function submit() {
    if (!from || !ready || busy) return;
    const point: BranchPoint | undefined =
      mode === 'lsn'
        ? { lsn: lsn.trim().toUpperCase() }
        : mode === 'time'
          ? { time: new Date(time).toISOString() }
          : undefined;
    setBusy(true);
    setError(undefined);
    try {
      await onCreate({
        name: name.trim(),
        parentId: from.id,
        protected: prot,
        ...(point ? { point } : {}),
      });
      close();
    } catch (e) {
      setError(failureText(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog
      open={from !== undefined}
      onClose={close}
      title={`New branch from ${from?.name ?? ''}`}
      footer={
        <>
          <Button onClick={close}>Cancel</Button>
          <Button variant="primary" disabled={busy || !ready} onClick={submit}>
            {busy ? 'Creating…' : 'Create branch'}
          </Button>
        </>
      }
    >
      <form
        className="flex flex-col gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <Field label="Branch name">
          {(p) => <Input {...p} value={name} autoFocus onChange={(e) => setName(e.target.value)} />}
        </Field>
        <fieldset className="flex flex-col gap-1">
          <legend className="text-body">Branch point</legend>
          {(
            [
              ['head', `The current head of ${from?.name ?? 'the parent'}`],
              ['lsn', 'An LSN'],
              ['time', 'A point in time'],
            ] as const
          ).map(([value, label]) => (
            <label key={value} className="loams-check">
              <input
                type="radio"
                name="branch-point"
                value={value}
                checked={mode === value}
                onChange={() => setMode(value)}
              />
              {label}
            </label>
          ))}
        </fieldset>
        {mode === 'lsn' && (
          <Field
            label="LSN"
            hint="Within the project's history retention."
            error={lsnBad ? 'An LSN looks like 0/16B3748.' : undefined}
          >
            {(p) => (
              <Input
                {...p}
                className={/* mono-ok: id */ 'font-mono'}
                placeholder="0/16B3748"
                value={lsn}
                onChange={(e) => setLsn(e.target.value)}
              />
            )}
          </Field>
        )}
        {mode === 'time' && (
          <Field label="Time" hint="Your local time; the server resolves it to an LSN.">
            {(p) => (
              <Input
                {...p}
                type="datetime-local"
                step={1}
                value={time}
                onChange={(e) => setTime(e.target.value)}
              />
            )}
          </Field>
        )}
        <Checkbox
          label="Protected (only an admin can delete it)"
          checked={prot}
          onChange={(e) => setProt(e.currentTarget.checked)}
        />
        {error && (
          <Notice tone="danger" title="Could not create the branch">
            {error}
          </Notice>
        )}
      </form>
    </Dialog>
  );
}

function DeleteBranchDialog({
  row,
  onClose,
  onDelete,
}: {
  row: BranchRow | undefined;
  onClose: () => void;
  onDelete: (branch: PgBranch) => Promise<void>;
}) {
  const [typed, setTyped] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string>();
  const b = row?.branch;
  const close = () => {
    setTyped('');
    setError(undefined);
    onClose();
  };
  const confirmed = b !== undefined && (!b.protected || typed === b.name);
  async function submit() {
    if (!b || !confirmed || busy) return;
    setBusy(true);
    setError(undefined);
    try {
      await onDelete(b);
      close();
    } catch (e) {
      setError(failureText(e));
    } finally {
      setBusy(false);
    }
  }
  return (
    <Dialog
      open={b !== undefined}
      onClose={close}
      title={`Delete branch ${b?.name ?? ''}?`}
      footer={
        <>
          <Button onClick={close}>Cancel</Button>
          <Button variant="danger" disabled={busy || !confirmed} onClick={submit}>
            {busy ? 'Deleting…' : 'Delete branch'}
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-3">
        <p className="text-body">
          Its endpoints are stopped first, then its timeline, roles and databases are removed. This
          cannot be undone.
        </p>
        {b?.protected && (
          <Field label={`Type ${b.name} to confirm`} hint="This branch is protected.">
            {(p) => (
              <Input
                {...p}
                value={typed}
                autoComplete="off"
                onChange={(e) => setTyped(e.target.value)}
              />
            )}
          </Field>
        )}
        {error && (
          <Notice tone="danger" title="Could not delete the branch">
            {error}
          </Notice>
        )}
      </div>
    </Dialog>
  );
}
