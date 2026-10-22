// A refused statement (§49 §18.2): the server's message, the reason in words, and the
// ClickHouse code and name from `ErrorInfo.metadata`. A deny-list refusal (INTO
// OUTFILE, FROM INFILE, a host table function) and a read-only refusal each say what
// the person can do next.

import type { HouseFailure } from './client.js';
import { reasonText } from './client.js';

export function FailureView({
  failure,
  id,
  writesAllowed,
}: {
  failure: HouseFailure;
  id?: string;
  writesAllowed: boolean;
}) {
  const code = failure.clickhouseCode ?? Number(failure.metadata.clickhouse_code ?? NaN);
  const name = failure.clickhouseName ?? failure.metadata.clickhouse_name;
  const words = reasonText(failure.reason);
  return (
    <div
      id={id}
      className="flex flex-col gap-2 rounded-md border border-solid border-danger bg-danger-soft p-3"
      role="alert"
      data-testid="house-failure"
    >
      <p className="m-0 font-medium">{words ?? 'The House refused the statement.'}</p>
      <pre className="m-0 whitespace-pre-wrap break-words font-mono text-[13px]">
        {failure.message}
      </pre>
      <p className="m-0 text-body text-muted">
        {[
          Number.isFinite(code) && `Code ${code}`,
          name,
          failure.reason ?? failure.code,
          failure.metadata.clause,
          failure.metadata.table_function && `table function ${failure.metadata.table_function}`,
          failure.metadata.setting && `setting ${failure.metadata.setting}`,
        ]
          .filter(Boolean)
          .join(' · ')}
      </p>
      {failure.reason === 'house_disabled_function' && (
        <p className="m-0 text-body">
          The House never reads or writes files on the server. Remove the clause and the results
          come back to this page.
        </p>
      )}
      {failure.reason === 'house_read_only' && !writesAllowed && (
        <p className="m-0 text-body">Turn on "Allow writes" to run statements that change data.</p>
      )}
    </div>
  );
}
