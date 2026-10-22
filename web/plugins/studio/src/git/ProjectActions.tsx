// The project actions editor (§53 §5.7; SD1 Task 23): a project's named commands
// (build, test, the dev server), listed, added, edited and deleted through
// loams-agentd's ListProjectActions, UpsertProjectAction and DeleteProjectAction.
// They run from the terminal drawer's menu, which owns their terminals (T22-11); an
// action marked "run on worktree create" is the setup a new worktree runs.
// A project action belongs to a space, so the editor shows only where a thread has one.

import { createConnectQueryKey } from '@connectrpc/connect-query';
import { useAgentdMutation } from '@loams/console-host/query';
import { type ProjectAction, ProjectActionIcon, ProjectService } from '@loams/proto/agentd';
import {
  Badge,
  Button,
  Checkbox,
  ConfirmDialog,
  Dialog,
  Field,
  Input,
  Notice,
  SelectMenu,
} from '@loams/ui';
import { useQueryClient } from '@tanstack/react-query';
import { Pencil, Plus, SquareTerminal, Trash2 } from 'lucide-react';
import { useId, useState } from 'react';
import { gitErrorOf } from './errors.js';
import { useProjectActions } from './queries.js';

const ICONS: { value: string; label: string; icon: ProjectActionIcon }[] = [
  { value: 'play', label: 'Run', icon: ProjectActionIcon.PLAY },
  { value: 'test', label: 'Test', icon: ProjectActionIcon.TEST },
  { value: 'lint', label: 'Lint', icon: ProjectActionIcon.LINT },
  { value: 'build', label: 'Build', icon: ProjectActionIcon.BUILD },
  { value: 'configure', label: 'Configure', icon: ProjectActionIcon.CONFIGURE },
  { value: 'debug', label: 'Debug', icon: ProjectActionIcon.DEBUG },
];

const iconValue = (icon: ProjectActionIcon) => ICONS.find((i) => i.icon === icon)?.value ?? 'play';

interface Draft {
  id?: string;
  name: string;
  command: string;
  icon: string;
  runOnWorktreeCreate: boolean;
}

const EMPTY: Draft = { name: '', command: '', icon: 'play', runOnWorktreeCreate: false };

function ActionForm({
  draft,
  pending,
  error,
  onCancel,
  onSave,
}: {
  draft: Draft;
  pending: boolean;
  error: string | null;
  onCancel: () => void;
  onSave: (draft: Draft) => void;
}) {
  const [value, setValue] = useState(draft);
  const ready = value.name.trim() !== '' && value.command.trim() !== '' && !pending;
  return (
    <form
      aria-label={draft.id ? `Edit ${draft.name}` : 'New project action'}
      className="flex flex-col gap-2 border border-rule-soft p-3"
      onSubmit={(event) => {
        event.preventDefault();
        if (ready) onSave({ ...value, name: value.name.trim(), command: value.command.trim() });
      }}
    >
      <Field label="Name">
        {(p) => (
          <Input
            {...p}
            value={value.name}
            onChange={(e) => setValue({ ...value, name: e.target.value })}
          />
        )}
      </Field>
      <Field label="Command" hint="Runs in the thread's checkout, in a terminal.">
        {(p) => (
          <Input
            {...p}
            className={/* mono-ok: code */ 'font-mono'}
            value={value.command}
            onChange={(e) => setValue({ ...value, command: e.target.value })}
          />
        )}
      </Field>
      <div className="flex flex-col gap-1">
        <span className="text-[0.84rem] text-muted">Icon</span>
        <SelectMenu
          label="Icon"
          value={value.icon}
          onValueChange={(icon) => setValue({ ...value, icon })}
          options={ICONS.map((i) => ({ value: i.value, label: i.label }))}
        />
      </div>
      <Checkbox
        label="Run when a worktree is created (setup)"
        checked={value.runOnWorktreeCreate}
        onChange={(e) => setValue({ ...value, runOnWorktreeCreate: e.target.checked })}
      />
      {error && (
        <Notice tone="danger" title="Could not save the action">
          {error}
        </Notice>
      )}
      <div className="flex justify-end gap-2">
        <Button variant="quiet" onClick={onCancel} disabled={pending}>
          Cancel
        </Button>
        <Button variant="primary" type="submit" disabled={!ready}>
          Save
        </Button>
      </div>
    </form>
  );
}

/** The toolbar's "Actions" button and its editor dialog. */
export function ProjectActionsEditor({ spaceId }: { spaceId: string }) {
  const [open, setOpen] = useState(false);
  const [editing, setEditing] = useState<Draft | null>(null);
  const [deleting, setDeleting] = useState<ProjectAction | null>(null);
  const [error, setError] = useState<string | null>(null);
  const listed = useProjectActions(open ? spaceId : undefined);
  const client = useQueryClient();
  const listId = useId();
  const refresh = () =>
    client.invalidateQueries({
      queryKey: createConnectQueryKey({
        schema: ProjectService.method.listProjectActions,
        cardinality: 'finite',
      }),
    });
  const upsert = useAgentdMutation(ProjectService.method.upsertProjectAction, {
    onSuccess: () => {
      setEditing(null);
      return refresh();
    },
    onError: (e) => setError(gitErrorOf(e).message),
  });
  const remove = useAgentdMutation(ProjectService.method.deleteProjectAction, {
    onSuccess: () => refresh(),
    onError: (e) => setError(gitErrorOf(e).message),
  });
  const actions = listed.data?.snapshot?.actions ?? [];

  return (
    <>
      <Button size="sm" variant="quiet" onClick={() => setOpen(true)} aria-label="Project actions">
        <SquareTerminal size={14} aria-hidden="true" />
        Actions
      </Button>
      <Dialog open={open} onClose={() => setOpen(false)} title="Project actions">
        <div className="flex w-full min-w-0 flex-col gap-3">
          <p className="m-0 text-[0.84rem] text-muted">
            Named commands for this project. Run them from the terminal drawer.
          </p>
          {listed.isPending ? (
            <p className="m-0 text-[0.84rem] text-muted">Loading actions…</p>
          ) : listed.isError ? (
            <Notice tone="danger" title="Could not list the actions">
              {gitErrorOf(listed.error).message}
            </Notice>
          ) : actions.length === 0 ? (
            <p className="m-0 text-[0.84rem] text-muted">No actions yet.</p>
          ) : (
            <ul id={listId} aria-label="Actions" className="m-0 flex list-none flex-col gap-1 p-0">
              {actions.map((a) => (
                <li
                  key={a.id}
                  className="flex min-w-0 items-center gap-2 border border-rule-soft px-2 py-1.5"
                >
                  <span className="flex min-w-0 flex-1 flex-col">
                    <span className="truncate text-[0.86rem] text-ink">{a.name}</span>
                    <code className="truncate text-[0.76rem] text-muted">{a.command}</code>
                  </span>
                  {a.runOnWorktreeCreate && <Badge>setup</Badge>}
                  <Button
                    size="icon"
                    variant="quiet"
                    aria-label={`Edit ${a.name}`}
                    onClick={() => {
                      setError(null);
                      setEditing({
                        id: a.id,
                        name: a.name,
                        command: a.command,
                        icon: iconValue(a.icon),
                        runOnWorktreeCreate: a.runOnWorktreeCreate,
                      });
                    }}
                  >
                    <Pencil size={13} aria-hidden="true" />
                  </Button>
                  <Button
                    size="icon"
                    variant="quiet"
                    aria-label={`Delete ${a.name}`}
                    onClick={() => setDeleting(a)}
                  >
                    <Trash2 size={13} aria-hidden="true" />
                  </Button>
                </li>
              ))}
            </ul>
          )}
          {editing ? (
            <ActionForm
              key={editing.id ?? 'new'}
              draft={editing}
              pending={upsert.isPending}
              error={error}
              onCancel={() => setEditing(null)}
              onSave={(d) => {
                setError(null);
                upsert.mutate({
                  spaceId,
                  ...(d.id ? { actionId: d.id } : {}),
                  action: {
                    name: d.name,
                    command: d.command,
                    icon: ICONS.find((i) => i.value === d.icon)?.icon ?? ProjectActionIcon.PLAY,
                    runOnWorktreeCreate: d.runOnWorktreeCreate,
                  },
                });
              }}
            />
          ) : (
            <div>
              <Button
                size="sm"
                onClick={() => {
                  setError(null);
                  setEditing(EMPTY);
                }}
              >
                <Plus size={13} aria-hidden="true" />
                Add action
              </Button>
            </div>
          )}
          {!editing && error && (
            <Notice tone="danger" title="The change failed">
              {error}
            </Notice>
          )}
        </div>
      </Dialog>
      <ConfirmDialog
        open={deleting !== null}
        onClose={() => setDeleting(null)}
        object={deleting?.name ?? 'this action'}
        confirm="Delete"
        impact="The action is removed from this project for every thread."
        onConfirm={() => {
          const a = deleting;
          setDeleting(null);
          if (a) remove.mutate({ spaceId, actionId: a.id });
        }}
      />
    </>
  );
}
