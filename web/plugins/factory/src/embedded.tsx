// `/studio/factory/:app/app`: the app's own UI, embedded in the main window (D678).
// The page owns a placeholder element; main positions a native view over the
// rect we report. The view hides on unmount and while any overlay is open.

import type { FactoryAppId, FactoryAppInfo, LoamsDesktopApi } from '@loams/desktop/contracts';
import { Button, Empty, Notice } from '@loams/ui';
import { ExternalLink, RefreshCw, SquareArrowOutUpRight } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { APP_LABELS, HealthPill, isConfigured, PreviewTag } from './model.js';

/** A popover or modal that would be drawn under the native view. */
const OVERLAY = '[data-overlay-open], dialog[open]';

type Rect = { x: number; y: number; width: number; height: number };
const same = (a: Rect | undefined, b: Rect) =>
  !!a && a.x === b.x && a.y === b.y && a.width === b.width && a.height === b.height;

export function EmbeddedApp({
  desktop,
  app,
  navigate,
  embeds = true,
}: {
  desktop: LoamsDesktopApi;
  app: FactoryAppId;
  navigate: (to: string) => void;
  /** False in a browser: no embedded view, the app opens in a tab (SF1 amendment F5). */
  embeds?: boolean;
}) {
  const slot = useRef<HTMLDivElement>(null);
  const [info, setInfo] = useState<FactoryAppInfo>();
  const [error, setError] = useState<string>();
  const label = info?.label ?? APP_LABELS[app];
  // The app's own UI: its App URL when it has one (ItsAPlan), else its URL.
  const uiUrl = info?.fields?.appUrl || info?.url;

  useEffect(() => {
    let live = true;
    setInfo(undefined);
    desktop.factory
      .list()
      .then((a) => live && setInfo(a.find((x) => x.id === app)))
      .catch((e) => live && setError(e instanceof Error ? e.message : String(e)));
    return () => {
      live = false;
    };
  }, [desktop, app]);

  const configured = !!info && isConfigured(info.health);
  useEffect(() => {
    const el = slot.current;
    if (!el || !configured || !embeds) return;
    let frame = 0;
    let shown: Rect | undefined;
    let hidden = false;
    let alive = true;
    const hide = () => {
      if (hidden) return;
      hidden = true;
      shown = undefined;
      void desktop.factory.hideEmbedded().catch(() => undefined);
    };
    const report = () => {
      frame = 0;
      if (!alive) return;
      if (document.querySelector(OVERLAY)) return hide();
      const r = el.getBoundingClientRect();
      const rect = { x: r.x, y: r.y, width: r.width, height: r.height };
      if (same(shown, rect)) return;
      shown = rect;
      hidden = false;
      desktop.factory
        .showEmbedded(app, rect)
        .then((res) => {
          // A failed report forgets the rect so the next layout event retries.
          if (!res.ok) shown = undefined;
          if (alive) setError(res.ok ? undefined : res.message);
        })
        .catch((e) => {
          shown = undefined;
          if (alive) setError(e instanceof Error ? e.message : String(e));
        });
    };
    // One report per animation frame, however many layout events arrive.
    const schedule = () => {
      if (!frame) frame = requestAnimationFrame(report);
    };
    const ro = new ResizeObserver(schedule);
    ro.observe(el);
    const mo = new MutationObserver(schedule);
    mo.observe(document.body, {
      subtree: true,
      childList: true,
      attributes: true,
      attributeFilter: ['open', 'data-overlay-open'],
    });
    window.addEventListener('resize', schedule);
    window.addEventListener('scroll', schedule, true);
    schedule();
    return () => {
      alive = false;
      if (frame) cancelAnimationFrame(frame);
      ro.disconnect();
      mo.disconnect();
      window.removeEventListener('resize', schedule);
      window.removeEventListener('scroll', schedule, true);
      void desktop.factory.hideEmbedded().catch(() => undefined);
    };
  }, [desktop, app, configured, embeds]);

  if (info && !configured) {
    return (
      <div className="lc-page">
        <Empty
          title={`${label} is not configured`}
          actions={
            <Button onClick={() => navigate(`/studio/factory/${app}/configure`)}>Configure</Button>
          }
        />
      </div>
    );
  }
  if (!embeds) {
    return (
      <div className="lc-page">
        <Empty
          title={label}
          actions={
            <>
              <Button
                variant="primary"
                disabled={!info}
                onClick={() =>
                  void desktop.factory.openApp(app).then((r) => {
                    if (!r.ok) setError(r.message);
                  })
                }
              >
                <ExternalLink aria-hidden="true" size={14} /> Open {label} in a new tab
              </Button>
              <Button variant="quiet" onClick={() => navigate('/studio/factory')}>
                All apps
              </Button>
            </>
          }
        >
          In a browser, an app opens in its own tab.
        </Empty>
        {error && <Notice tone="danger" title={error} />}
      </div>
    );
  }
  return (
    <div className="box-border flex h-full min-h-0 flex-col gap-3 p-4">
      <div className="flex flex-wrap items-center gap-2">
        <h1 className="m-0 mr-auto flex items-center gap-3 text-lg font-medium">
          {label} {info && <HealthPill health={info.health} />}
          <PreviewTag preview={info?.preview} />
        </h1>
        <Button size="sm" variant="quiet" onClick={() => navigate('/studio/factory')}>
          All apps
        </Button>
        <Button size="sm" variant="quiet" onClick={() => void desktop.factory.reloadEmbedded(app)}>
          <RefreshCw aria-hidden="true" size={14} /> Reload
        </Button>
        <Button
          size="sm"
          variant="quiet"
          onClick={() =>
            void desktop.factory.popOut(app).then((r) => {
              if (r.ok) navigate('/studio/factory');
              else setError(r.message);
            })
          }
        >
          <SquareArrowOutUpRight aria-hidden="true" size={14} /> Pop out
        </Button>
        <Button
          size="sm"
          variant="quiet"
          disabled={!uiUrl}
          onClick={() => uiUrl && void desktop.shell.openExternal(uiUrl)}
        >
          <ExternalLink aria-hidden="true" size={14} /> Open in browser
        </Button>
      </div>
      {error && <Notice tone="danger" title={error} />}
      <div
        ref={slot}
        data-embedded-placeholder=""
        title={`${label} app`}
        className="box-border flex min-h-80 flex-1 items-center justify-center border border-rule bg-surface text-sm text-muted"
      />
    </div>
  );
}
