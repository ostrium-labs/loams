// The notification surface (SD1 Task 14; §53 §6.5 G19): a bell in Studio's header with
// the unread count, and a list of the daemon's notifications, newest first, each leading
// to its thread or run group. Hidden while the daemon does not serve notifications. The
// desktop's OS notifications are Electron main's, from the same stream (SD1 Task 28).

import { NotificationKind } from '@loams/proto/agentd';
import { cx, formatRelative, Popover } from '@loams/ui';
import {
  Bell,
  CircleCheck,
  CircleQuestionMark,
  CircleX,
  Layers,
  ShieldQuestionMark,
  TriangleAlert,
} from 'lucide-react';
import { useState } from 'react';
import { KIND_LABEL, type StudioNotification, useNotifications } from './data.js';

const ICON: Record<NotificationKind, typeof Bell> = {
  [NotificationKind.UNSPECIFIED]: Bell,
  [NotificationKind.TURN_DONE]: CircleCheck,
  [NotificationKind.APPROVAL]: ShieldQuestionMark,
  [NotificationKind.QUESTION]: CircleQuestionMark,
  [NotificationKind.ERROR]: CircleX,
  [NotificationKind.GROUP_READY]: Layers,
  [NotificationKind.CHECK_FAILED]: TriangleAlert,
};

const TONE: Record<NotificationKind, string> = {
  [NotificationKind.UNSPECIFIED]: 'text-muted',
  [NotificationKind.TURN_DONE]: 'text-grow',
  [NotificationKind.APPROVAL]: 'text-accent-fg',
  [NotificationKind.QUESTION]: 'text-info',
  [NotificationKind.ERROR]: 'text-danger',
  [NotificationKind.GROUP_READY]: 'text-grow',
  [NotificationKind.CHECK_FAILED]: 'text-danger',
};

const BELL =
  'relative inline-grid size-7 shrink-0 cursor-pointer place-items-center rounded-pill border border-rule ' +
  'bg-bg text-muted hover:bg-raised hover:text-ink';

function Row({ n, onOpen }: { n: StudioNotification; onOpen: (href: string) => void }) {
  const Icon = ICON[n.kind] ?? Bell;
  const body = (
    <>
      <Icon size={14} aria-hidden="true" className={cx('mt-0.5 shrink-0', TONE[n.kind])} />
      <span className="flex min-w-0 flex-col">
        <span className="truncate text-[0.84rem] text-ink">{n.title}</span>
        <span className="text-[0.74rem] text-muted">
          {KIND_LABEL[n.kind]} · {formatRelative(new Date(n.createdAtMs).toISOString())}
        </span>
      </span>
    </>
  );
  const row = 'flex w-full items-start gap-2 rounded-md px-2 py-1.5 text-left';
  return (
    <li>
      {n.href ? (
        <a
          href={`#${n.href}`}
          className={cx(row, 'no-underline hover:bg-raised')}
          onClick={(e) => {
            e.preventDefault();
            if (n.href) onOpen(n.href);
          }}
        >
          {body}
        </a>
      ) : (
        <div className={row}>{body}</div>
      )}
    </li>
  );
}

/** The bell and its list. `navigate` opens a notification's thread or run group. */
export function NotificationsBell({
  navigate,
  onArrive,
}: {
  navigate: (to: string) => void;
  /** Called for each notification that arrives after the page opened (web OS notifications). */
  onArrive?: (n: StudioNotification) => void;
}) {
  const { items, unread, unsupported, markAllRead } = useNotifications(onArrive);
  const [open, setOpen] = useState(false);
  if (unsupported) return null;
  const label = unread > 0 ? `Notifications, ${unread} unread` : 'Notifications';
  return (
    <Popover
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) markAllRead();
      }}
      align="end"
      title="Notifications"
      trigger={
        <button type="button" className={BELL} aria-label={label} title="Notifications">
          <Bell size={14} aria-hidden="true" />
          {unread > 0 && (
            <span
              aria-hidden="true"
              className="absolute -top-1 -right-1 grid h-4 min-w-4 place-items-center rounded-pill bg-accent px-1 text-[0.62rem] font-[600] text-bg"
            >
              {unread > 99 ? '99+' : unread}
            </span>
          )}
        </button>
      }
    >
      {items.length === 0 ? (
        <p className="m-0 w-[min(80vw,340px)] text-[0.84rem] text-muted">
          Nothing yet. Finished turns, questions, approvals and errors show up here.
        </p>
      ) : (
        <ul
          aria-label="Notifications"
          className="m-0 flex max-h-[60vh] w-[min(80vw,340px)] list-none flex-col overflow-auto p-0"
        >
          {items.map((n) => (
            <Row
              key={n.id}
              n={n}
              onOpen={(href) => {
                setOpen(false);
                markAllRead();
                navigate(href);
              }}
            />
          ))}
        </ul>
      )}
    </Popover>
  );
}
