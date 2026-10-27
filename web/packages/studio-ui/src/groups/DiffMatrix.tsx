// The diff matrix (SD1 Task 25; design §53 §5.5 "which variants touched which files"): files
// down, variants across, each cell shaded by the size of that variant's change to the file.
// A touched cell opens the pair diff for that file. Paths are text.

import { cx } from "@loams/ui";
import { useMemo } from "react";

import { type ComparisonRowView, type GroupMemberView, matrixCells } from "./model";

const LEVEL_CLASS = {
  0: "",
  1: "bg-grow/10 text-ink",
  2: "bg-grow/20 text-ink",
  3: "bg-grow/35 text-ink",
} as const;

export function DiffMatrix({
  rows,
  members,
  selected,
  onSelect,
}: {
  readonly rows: readonly ComparisonRowView[];
  readonly members: readonly Pick<GroupMemberView, "chatId" | "label">[];
  /** The cell whose pair diff is shown. */
  readonly selected?: { path: string; chatId: string } | null;
  readonly onSelect: (cell: { path: string; chatId: string }) => void;
}) {
  const cells = useMemo(
    () =>
      matrixCells(
        rows,
        members.map((m) => m.chatId),
      ),
    [rows, members],
  );
  if (cells.length === 0) {
    return <p className="m-0 py-3 text-body text-muted">No variant has changed a file yet.</p>;
  }
  return (
    <div className="overflow-x-auto">
      <table
        aria-label="Files changed by each variant"
        className="w-full border-collapse text-body"
      >
        <thead>
          <tr>
            <th
              scope="col"
              className="border-b border-rule px-2 py-1.5 text-left text-micro font-medium text-faint"
            >
              File
            </th>
            {members.map((m) => (
              <th
                key={m.chatId}
                scope="col"
                className="border-b border-rule px-2 py-1.5 text-left text-small font-medium text-muted"
              >
                {m.label}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {cells.map((row) => (
            <tr key={row.path} className="border-b border-rule-soft">
              <th
                scope="row"
                className={
                  /* mono-ok: path */ "max-w-[28rem] truncate px-2 py-1 text-left font-mono text-small font-normal text-ink"
                }
                title={row.path}
              >
                {row.path}
              </th>
              {row.cells.map((cell, index) => {
                const label = members[index]?.label ?? cell.chatId;
                const isSelected = selected?.path === row.path && selected.chatId === cell.chatId;
                return (
                  <td
                    key={cell.chatId}
                    data-touched={cell.touched ? "true" : "false"}
                    data-level={cell.level}
                    className="p-0.5"
                  >
                    {cell.touched ? (
                      <button
                        type="button"
                        aria-label={`Compare ${row.path} from ${label}`}
                        aria-pressed={isSelected}
                        onClick={() => onSelect({ path: row.path, chatId: cell.chatId })}
                        className={cx(
                          "flex w-full cursor-pointer items-center gap-1.5 rounded-sm border-0 px-2 py-1 text-small tabular-nums",
                          LEVEL_CLASS[cell.level],
                          isSelected && "outline-2 outline-focus -outline-offset-2",
                        )}
                      >
                        <span>+{cell.additions}</span>
                        <span>−{cell.deletions}</span>
                      </button>
                    ) : (
                      <span className="block px-2 py-1 text-small text-faint">
                        <span aria-hidden="true">—</span>
                        <span className="sr-only">
                          {label} did not change {row.path}
                        </span>
                      </span>
                    )}
                  </td>
                );
              })}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
