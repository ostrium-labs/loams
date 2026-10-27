// The billing banner in the shell's top bar (design 63 §18): the most severe notice from
// `bannerNotices`, with its way to resume. A warning can be dismissed for the session; a stop
// cannot, since work is being refused.

import { Button } from '@loams/ui';
import { useEffect, useState } from 'react';
import { type BannerNotice, BILLING_ROUTES, bannerNotices } from './banner.js';
import type { BillingClient } from './types.js';

/** How often the banner rereads usage. Threshold events arrive within a minute (§15). */
export const BANNER_REFRESH_MS = 60_000;

const TONE: Record<BannerNotice['level'], string> = {
  stop: 'border-danger bg-danger-soft text-ink',
  warn: 'border-warn bg-warn-soft text-ink',
  info: 'border-rule bg-info-soft text-ink',
};

export function BillingBannerView({
  notices,
  navigate,
  onDismiss,
}: {
  notices: BannerNotice[];
  navigate: (to: string) => void;
  onDismiss?: () => void;
}) {
  const top = notices[0];
  if (!top) return null;
  const more = notices.length - 1;
  return (
    <div
      role={top.level === 'stop' ? 'alert' : 'status'}
      data-billing-banner={top.level}
      className={`flex max-w-[44rem] items-center gap-2 truncate rounded-sm border px-2 py-0.5 text-small ${TONE[top.level]}`}
    >
      <span className="truncate">{top.message}</span>
      {top.actions.slice(0, 2).map((a) => (
        <Button key={a.label} size="sm" variant="link" onClick={() => navigate(a.href)}>
          {a.label}
        </Button>
      ))}
      {more > 0 && (
        <Button size="sm" variant="link" onClick={() => navigate(BILLING_ROUTES.usage)}>
          +{more} more
        </Button>
      )}
      {top.level !== 'stop' && onDismiss && (
        <Button size="sm" variant="quiet" aria-label="Dismiss" onClick={onDismiss}>
          ×
        </Button>
      )}
    </div>
  );
}

export function BillingBanner({
  client,
  navigate,
  refreshMs = BANNER_REFRESH_MS,
}: {
  client: BillingClient;
  navigate: (to: string) => void;
  refreshMs?: number;
}) {
  const [notices, setNotices] = useState<BannerNotice[]>([]);
  const [dismissed, setDismissed] = useState<string>();
  useEffect(() => {
    let live = true;
    const read = () =>
      client.usage().then(
        (report) => live && setNotices(bannerNotices(report)),
        // The banner is advisory: a failed read leaves the last answer up.
        () => undefined,
      );
    void read();
    const timer = setInterval(read, refreshMs);
    return () => {
      live = false;
      clearInterval(timer);
    };
  }, [client, refreshMs]);
  const top = notices[0];
  const id = top ? `${top.feature}/${top.threshold}/${top.level}` : undefined;
  if (!top || (top.level !== 'stop' && dismissed === id)) return null;
  return (
    <BillingBannerView notices={notices} navigate={navigate} onDismiss={() => setDismissed(id)} />
  );
}
