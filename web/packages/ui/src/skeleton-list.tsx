// Loading placeholders built from `Skeleton` (UX-4): rows for a list or a table, cards for a
// page that is still asking the server what it has. Each is one polite status with its own
// words for assistive technology; the bars themselves are hidden from it.

import { cx } from './cx';
import { Skeleton } from './skeleton';

/** Row widths, so a list of placeholders does not read as one solid block. */
const WIDTHS = ['w-full', 'w-11/12', 'w-4/5', 'w-full', 'w-3/4', 'w-5/6'] as const;

export function SkeletonList({
  label,
  rows = 4,
  layout = 'rows',
  className,
}: {
  /** What is loading, said to assistive technology ("Loading collections"). */
  label: string;
  /** How many placeholders. */
  rows?: number;
  /** `rows`: 28px bars, as list rows; `cards`: a grid of 96px blocks, as a page's cards. */
  layout?: 'rows' | 'cards';
  className?: string;
}) {
  return (
    <div
      role="status"
      aria-live="polite"
      aria-label={label}
      data-slot="skeleton-list"
      className={cx(
        layout === 'cards'
          ? 'grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-3'
          : 'flex flex-col gap-2',
        className,
      )}
    >
      <span className="sr-only">{label}</span>
      {Array.from({ length: rows }, (_, i) => (
        <Skeleton
          // oxlint-disable-next-line react/no-array-index-key -- placeholders have no identity
          key={i}
          className={layout === 'cards' ? 'h-24' : cx('h-7', WIDTHS[i % WIDTHS.length])}
        />
      ))}
    </div>
  );
}
