// The side-by-side pair diff (SD1 Task 25; design §53 §5.5): two variants' results for one
// file, or for all of them, from `GetRunGroupComparison.pairDiff`, in Task 20's viewer
// (`StyledDiffCodeView`) with the split layout: the left column is the first result, the
// right the second. Same rendering path as the diff panel (D919, T0-4): @pierre/diffs writes
// escaped tokens into a shadow root, so a patch's text never becomes markup.

import { useMemo } from "react";

import {
  buildFileDiffIdentityKey,
  getRenderablePatch,
  resolveFileDiffPath,
} from "../diff/diffRendering";
import {
  StyledDiffCodeView,
  type StyledDiffCodeViewOptions,
} from "../diff/review/StyledDiffCodeView";
import { useColourScheme } from "../files/codeTheme";

const NO_ANNOTATIONS: readonly never[] = [];
const renderNothing = () => null;
const ignoreSelection = () => {};

export function PairDiff({
  patch,
  from,
  to,
  wrap = false,
}: {
  readonly patch: string;
  /** The left column's name: the first variant (or the base). */
  readonly from: string;
  /** The right column's name: the second variant. */
  readonly to: string;
  readonly wrap?: boolean;
}) {
  const scheme = useColourScheme();
  const renderable = useMemo(() => getRenderablePatch(patch, "studio-pair-diff"), [patch]);
  const options = useMemo<StyledDiffCodeViewOptions>(
    () => ({ layout: "split", wrap, wordDiff: true, scheme, commentable: false }),
    [scheme, wrap],
  );

  return (
    <div className="flex min-w-0 flex-col gap-2" data-pair-diff>
      <div className="grid grid-cols-2 gap-2 text-micro font-medium text-faint">
        <span className="truncate">{from}</span>
        <span className="truncate">{to}</span>
      </div>
      {renderable === null ? (
        <p className="m-0 py-3 text-body text-muted">No difference between these two results.</p>
      ) : renderable.kind === "raw" ? (
        <div className="flex flex-col gap-1">
          <p className="m-0 text-small text-muted">{renderable.reason}</p>
          <pre className="m-0 max-h-[28rem] overflow-auto border border-rule-soft bg-surface p-3 font-mono text-small text-ink">
            {renderable.text}
          </pre>
        </div>
      ) : (
        renderable.files.map((fileDiff) => {
          const path = resolveFileDiffPath(fileDiff);
          return (
            <section
              key={buildFileDiffIdentityKey(fileDiff)}
              data-pair-file={path}
              aria-label={path}
              className="min-w-0 border border-rule-soft"
            >
              <h4
                className={
                  /* mono-ok: path */ "m-0 border-b border-rule-soft bg-surface px-3 py-1.5 font-mono text-small font-normal text-ink"
                }
              >
                {path}
              </h4>
              <StyledDiffCodeView
                fileDiff={fileDiff}
                annotations={NO_ANNOTATIONS}
                renderAnnotation={renderNothing}
                options={options}
                selectedLines={null}
                onSelectLines={ignoreSelection}
              />
            </section>
          );
        })
      )}
    </div>
  );
}
