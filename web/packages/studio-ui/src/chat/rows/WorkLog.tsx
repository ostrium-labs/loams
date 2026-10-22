// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/chat/WorkLog.tsx at commit 6524a8d4, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/studio-ui/LICENSE.t3code). Modified by The Loams Authors:
// @loams/ui token classes, `cx` and hard corners; the row geometry and the grouped-rows context are upstream's. Loams' changes are Apache-2.0.

import { cx } from "@loams/ui";
import { type ComponentProps, createContext, type ReactNode, use } from "react";

const GroupedRows = createContext(false);

/** Groups may span virtualized timeline items. Each part owns its trailing space. */
export function WorkLogBlock({
  layout = "standalone",
  continues = false,
  children,
}: {
  layout?: "standalone" | "group-header" | "group-content";
  continues?: boolean | undefined;
  children: ReactNode;
}) {
  return (
    <div
      className={
        continues || layout === "group-header"
          ? "pb-0"
          : layout === "group-content"
            ? "pb-1"
            : "pb-2"
      }
    >
      {children}
    </div>
  );
}

/** Expanded members align with the header and use the same compact row geometry. */
export function WorkLogList({ children }: { children: ReactNode }) {
  return (
    <GroupedRows value>
      <div className="flex min-w-0 flex-col">{children}</div>
    </GroupedRows>
  );
}

type RowContent = {
  icon?: ReactNode;
  label: ReactNode;
  trailing?: ReactNode;
  wrapLabel?: boolean;
};

function WorkLogLine({ icon, label, trailing, wrapLabel }: RowContent) {
  return (
    <div
      className={cx(
        "flex min-h-6 min-w-0 gap-1.5 text-sm leading-relaxed",
        wrapLabel ? "items-start" : "items-center",
      )}
    >
      <div className="flex min-w-0 flex-1 items-center gap-1.5">
        {icon ? (
          <span className="flex size-6 shrink-0 items-center justify-center text-faint">
            {icon}
          </span>
        ) : null}
        <div
          className={cx(
            "min-w-0 flex-1 text-muted",
            !wrapLabel && "truncate [&_*]:whitespace-nowrap",
          )}
        >
          {label}
        </div>
      </div>
      {trailing}
    </div>
  );
}

const interactionClassName =
  "cursor-pointer transition-colors duration-(--op-duration-fast) ease-out hover:bg-row-hover focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-focus motion-reduce:transition-none";

function useRowClassName(interactive: boolean) {
  const grouped = use(GroupedRows);
  return cx(
    "relative w-full min-w-0 rounded-md border-0 bg-transparent px-0.5 text-left text-inherit",
    grouped ? "py-0" : "py-0.5",
    interactive && interactionClassName,
  );
}

// No className/style escape hatch: consumers supply content and behavior, not geometry.
export function WorkLogButton({
  icon,
  label,
  trailing,
  wrapLabel = false,
  ...buttonProps
}: RowContent & Omit<ComponentProps<"button">, "className" | "style" | "children">) {
  const className = useRowClassName(true);
  return (
    <button {...buttonProps} type="button" className={className}>
      <WorkLogLine icon={icon} label={label} trailing={trailing} wrapLabel={wrapLabel} />
    </button>
  );
}

/** Div-based disclosure supports selectable detail and nested actions in an entry. */
export function WorkLogRow({
  icon,
  label,
  trailing,
  wrapLabel = false,
  children,
  ...rowProps
}: RowContent & Omit<ComponentProps<"div">, "className" | "style">) {
  const className = useRowClassName(rowProps.onClick !== undefined);
  return (
    <div {...rowProps} className={className}>
      <WorkLogLine icon={icon} label={label} trailing={trailing} wrapLabel={wrapLabel} />
      {children}
    </div>
  );
}

export function WorkLogDetails({ children }: { children: ReactNode }) {
  return (
    <div className="ms-7 flex max-h-96 flex-col gap-2 overflow-auto px-0.5 py-1 select-text">
      {children}
    </div>
  );
}
