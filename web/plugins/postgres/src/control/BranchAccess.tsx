import { Badge, Button, Field, Input, Notice, Select } from '@loams/ui';
import { type FormEvent, useCallback, useEffect, useState } from 'react';
import { PasswordOnce } from './PasswordOnce.js';
import {
  failureText,
  type IssuedPassword,
  type PgBranch,
  type PgControlClient,
  type PgDatabase,
  type PgRole,
} from './types.js';

type Load =
  | { state: 'loading' }
  | { state: 'error'; message: string }
  | { state: 'ready'; roles: PgRole[]; databases: PgDatabase[] };

/**
 * Roles and databases on one branch (PG2 Task 6). Creating a role, or resetting its
 * password, shows the password once. A database's owner is one of the branch's roles.
 */
export function BranchAccess({
  client,
  namespace,
  branch,
  copy,
}: {
  client: PgControlClient;
  namespace: string;
  branch: PgBranch;
  copy?: (text: string) => Promise<void> | void;
}) {
  const [load, setLoad] = useState<Load>({ state: 'loading' });
  const [issued, setIssued] = useState<IssuedPassword & { reset: boolean }>();
  const [error, setError] = useState<string>();
  const [roleName, setRoleName] = useState('');
  const [dbName, setDbName] = useState('');
  const [owner, setOwner] = useState('');
  const [busy, setBusy] = useState(false);
  const { projectId, id: branchId } = branch;

  const refresh = useCallback(async () => {
    try {
      const [roles, databases] = await Promise.all([
        client.listRoles(namespace, projectId, branchId),
        client.listDatabases(namespace, projectId, branchId),
      ]);
      setLoad({ state: 'ready', roles, databases });
    } catch (e) {
      setLoad({ state: 'error', message: failureText(e) });
    }
  }, [client, namespace, projectId, branchId]);

  useEffect(() => {
    setLoad({ state: 'loading' });
    setError(undefined);
    void refresh();
  }, [refresh]);

  async function act(fn: () => Promise<void>) {
    setBusy(true);
    setError(undefined);
    try {
      await fn();
      await refresh();
    } catch (e) {
      setError(failureText(e));
    } finally {
      setBusy(false);
    }
  }

  // The owner picked, or the first non-system role, as the select shows it.
  const roles = load.state === 'ready' ? load.roles : [];
  const ownerValue =
    (roles.some((r) => r.name === owner) ? owner : '') ||
    roles.find((r) => !r.system)?.name ||
    roles[0]?.name ||
    '';

  const createRole = (e: FormEvent) => {
    e.preventDefault();
    const name = roleName.trim();
    if (!name) return;
    void act(async () => {
      const r = await client.createRole(namespace, projectId, branchId, name);
      setRoleName('');
      setIssued({ ...r, reset: false });
    });
  };
  const createDatabase = (e: FormEvent) => {
    e.preventDefault();
    const name = dbName.trim();
    if (!name || !ownerValue) return;
    void act(async () => {
      await client.createDatabase(namespace, projectId, branchId, name, ownerValue);
      setDbName('');
    });
  };

  if (load.state === 'loading') {
    return <p className="text-body text-muted">Loading roles and databases…</p>;
  }
  if (load.state === 'error') {
    return (
      <Notice tone="danger" title="Could not load roles and databases">
        {load.message}
      </Notice>
    );
  }

  return (
    <section aria-labelledby={`access-${branchId}`} className="flex flex-col gap-4">
      <h3 id={`access-${branchId}`} className="text-emphasis font-semibold">
        Roles and databases on {branch.name}
      </h3>
      {error && (
        <Notice tone="danger" title="That did not work">
          {error}
        </Notice>
      )}
      <div className="grid gap-4 md:grid-cols-2">
        <div className="flex flex-col gap-2">
          <div className="loams-table-wrap">
            <table className="loams-table">
              <caption className="sr-only">Roles on {branch.name}</caption>
              <thead>
                <tr>
                  <th scope="col">Role</th>
                  <th scope="col">
                    <span className="sr-only">Actions</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {load.roles.map((r) => (
                  <tr key={r.name} data-role={r.name}>
                    <td>
                      <span className={/* mono-ok: id */ 'font-mono text-body'}>{r.name}</span>{' '}
                      {r.system && <Badge>System</Badge>}
                    </td>
                    <td>
                      {!r.system && (
                        <span className="inline-flex gap-1">
                          <Button
                            size="sm"
                            disabled={busy}
                            aria-label={`Reset password for ${r.name}`}
                            onClick={() =>
                              act(async () => {
                                const x = await client.resetRolePassword(
                                  namespace,
                                  projectId,
                                  branchId,
                                  r.name,
                                );
                                setIssued({ ...x, reset: true });
                              })
                            }
                          >
                            Reset password
                          </Button>
                          <Button
                            size="sm"
                            variant="danger"
                            disabled={busy}
                            aria-label={`Delete role ${r.name}`}
                            onClick={() =>
                              act(() => client.deleteRole(namespace, projectId, branchId, r.name))
                            }
                          >
                            Delete
                          </Button>
                        </span>
                      )}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <form className="flex items-end gap-2" onSubmit={createRole}>
            <Field label="New role">
              {(p) => (
                <Input
                  {...p}
                  value={roleName}
                  autoComplete="off"
                  onChange={(e) => setRoleName(e.target.value)}
                />
              )}
            </Field>
            <Button type="submit" disabled={busy || !roleName.trim()}>
              Create role
            </Button>
          </form>
        </div>

        <div className="flex flex-col gap-2">
          <div className="loams-table-wrap">
            <table className="loams-table">
              <caption className="sr-only">Databases on {branch.name}</caption>
              <thead>
                <tr>
                  <th scope="col">Database</th>
                  <th scope="col">Owner</th>
                  <th scope="col">
                    <span className="sr-only">Actions</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {load.databases.map((d) => (
                  <tr key={d.name} data-database={d.name}>
                    <td className={/* mono-ok: id */ 'font-mono text-body'}>{d.name}</td>
                    <td className={/* mono-ok: id */ 'font-mono text-small'}>{d.ownerRole}</td>
                    <td>
                      <Button
                        size="sm"
                        variant="danger"
                        disabled={busy}
                        aria-label={`Delete database ${d.name}`}
                        onClick={() =>
                          act(() => client.deleteDatabase(namespace, projectId, branchId, d.name))
                        }
                      >
                        Delete
                      </Button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <form className="flex flex-wrap items-end gap-2" onSubmit={createDatabase}>
            <Field label="New database">
              {(p) => (
                <Input
                  {...p}
                  value={dbName}
                  autoComplete="off"
                  onChange={(e) => setDbName(e.target.value)}
                />
              )}
            </Field>
            <Field label="Owner">
              {(p) => (
                <Select {...p} value={ownerValue} onChange={(e) => setOwner(e.target.value)}>
                  {load.roles.map((r) => (
                    <option key={r.name} value={r.name}>
                      {r.name}
                    </option>
                  ))}
                </Select>
              )}
            </Field>
            <Button type="submit" disabled={busy || !dbName.trim() || !ownerValue}>
              Create database
            </Button>
          </form>
        </div>
      </div>
      <PasswordOnce issued={issued} onClose={() => setIssued(undefined)} copy={copy} />
    </section>
  );
}
