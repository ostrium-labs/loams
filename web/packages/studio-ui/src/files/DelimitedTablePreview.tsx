// Portions of this file are from T3 Code (https://github.com/pingdotgg/t3code),
// apps/web/src/components/files/DelimitedTablePreview.tsx at commit 6524a8d4, Copyright (c) 2026 T3 Tools Inc., MIT licence
// (see web/packages/studio-ui/LICENSE.t3code). Modified by The Loams Authors:
// ported to the Loams tokens; the parser is vendored beside it. Loams' changes are Apache-2.0.
import { useMemo } from "react";

import { parseDelimitedPreview } from "./delimitedPreview";
import { FileSurfaceNotice } from "./fileSurfaceChrome";

/** A bounded, readable table for CSV and TSV text; the source view keeps every byte. */
export function DelimitedTablePreview(props: {
  readonly name: string;
  readonly text: string;
  readonly delimiter: "," | "\t";
}) {
  const table = useMemo(
    () => parseDelimitedPreview(props.text, props.delimiter),
    [props.text, props.delimiter],
  );
  const [header, ...body] = table.rows;
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {table.truncated ? (
        <FileSurfaceNotice>
          Table limited to the first 100 rows and 30 columns. Switch to source for the rest.
        </FileSurfaceNotice>
      ) : null}
      <div className="min-h-0 flex-1 overflow-auto">
        <table
          className="min-w-full border-separate border-spacing-0 text-[0.78rem]"
          aria-label={props.name}
        >
          {header ? (
            <thead className="sticky top-0 z-10">
              <tr>
                {header.map((cell, columnIndex) => (
                  <th
                    // oxlint-disable-next-line react/no-array-index-key -- columns have stable positions and may repeat a value
                    key={columnIndex}
                    scope="col"
                    className="max-w-80 border-b border-rule bg-raised px-3 py-1.5 text-left align-bottom font-medium break-words whitespace-pre-wrap"
                  >
                    {cell}
                  </th>
                ))}
              </tr>
            </thead>
          ) : null}
          <tbody>
            {body.map((row, rowIndex) => (
              // oxlint-disable-next-line react/no-array-index-key -- rows have stable positions and may repeat
              <tr key={rowIndex} className="even:bg-surface">
                {row.map((cell, columnIndex) => (
                  <td
                    // oxlint-disable-next-line react/no-array-index-key -- columns have stable positions and may repeat a value
                    key={columnIndex}
                    className="max-w-80 border-b border-rule-soft px-3 py-1.5 align-top break-words whitespace-pre-wrap tabular-nums"
                  >
                    {cell}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </div>
  );
}
