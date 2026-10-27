// Demo mode and the "Demo data" toggle of the browser console.
//
// The toggle is off by default, remembered per browser: on, the console also loads
// the pages that still run on built-in sample data (catalog/demo.yml); off, every
// page talks to the connected engine.
//
// Demo mode is the other way round, and it is per address, not remembered: `?demo`
// boots the console with no engine at all, over the in-browser mock of the app
// protos (`@loams/console-host/testing`). It is the one way into console.loams.dev
// that needs no tailnet, so the gate screens offer it and the header says so.

const KEY = 'loams.console.demo-data';

/** The query parameter that boots demo mode: no engine, sample data, nothing stored. */
export const DEMO_PARAM = 'demo';

/** The pages demo.yml adds or turns on, as their nav labels, for the toggle's explanation. */
export const DEMO_PAGES = ['Deployments', 'Loams SQL', 'Metrics', 'Pipelines', 'Git', 'Billing'];

export function demoEnabled(): boolean {
  try {
    return globalThis.localStorage?.getItem(KEY) === '1';
  } catch {
    return false;
  }
}

export function setDemoEnabled(on: boolean) {
  try {
    if (on) globalThis.localStorage?.setItem(KEY, '1');
    else globalThis.localStorage?.removeItem(KEY);
  } catch {
    // Private mode: the toggle lasts for this page only.
  }
}

/** Whether this address asks for demo mode. */
export function demoRequested(search: string): boolean {
  return new URLSearchParams(search).has(DEMO_PARAM);
}

/** This page's address with demo mode on or off, keeping whatever else it carries. */
export function demoHref(
  on: boolean,
  loc: Pick<Location, 'pathname' | 'search' | 'hash'> = globalThis.location,
): string {
  const params = new URLSearchParams(loc.search);
  if (on) params.set(DEMO_PARAM, '1');
  else params.delete(DEMO_PARAM);
  const query = params.toString();
  return `${loc.pathname}${query === '' ? '' : `?${query}`}${loc.hash}`;
}
