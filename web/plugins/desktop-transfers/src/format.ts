// Labels for the Transfers page (design §85 §12, D2436). Pure, so tested alone.
import type { TransferJob, TransferProgress } from '@loams/desktop/contracts';
import { formatBytes } from '@loams/ui';

/** "12.3 MB/s"; a dash when there is no rate yet. */
export function formatRate(bps: number): string {
  return bps > 0 ? `${formatBytes(bps)}/s` : '—';
}

/** "45 s", "4 min 05 s", "2 h 10 min"; a dash when unknown. */
export function formatEta(s: number | undefined): string {
  if (s === undefined || !Number.isFinite(s) || s < 0) return '—';
  const t = Math.round(s);
  if (t < 60) return `${t} s`;
  if (t < 3600) return `${Math.floor(t / 60)} min ${String(t % 60).padStart(2, '0')} s`;
  return `${Math.floor(t / 3600)} h ${String(Math.floor((t % 3600) / 60)).padStart(2, '0')} min`;
}

/** The share done, 0..1. */
export function fraction(p: TransferProgress | null): number {
  if (!p || p.bytes_total <= 0) return 0;
  return Math.min(1, p.bytes_done / p.bytes_total);
}

/** "HTTP/3", "HTTP/2 (fallback)", "HTTP/2 (local)". */
export function transportLabel(t: TransferProgress['transport']): string {
  switch (t) {
    case 'h3':
      return 'HTTP/3';
    case 'h2':
      return 'HTTP/2 (fallback)';
    case 'h2c':
      return 'HTTP/2 (local)';
    default:
      return '—';
  }
}

/** What the row says about where the transfer stands. */
export function stateLabel(j: TransferJob): string {
  const done = j.progress?.bytes_done ?? 0;
  switch (j.state) {
    case 'queued':
      return 'Queued';
    case 'running':
      if (j.kind === 'run') return 'Following';
      if (j.progress?.phase === 'verifying') return 'Verifying';
      if (j.progress && j.progress.resumed_from > 0)
        return `Resumed from ${formatBytes(j.progress.resumed_from)}`;
      return j.progress?.phase === 'connecting' ? 'Connecting' : 'Transferring';
    case 'paused':
      return done > 0 || j.resumed
        ? `Paused, resumable${done > 0 ? ` from ${formatBytes(done)}` : ''}`
        : 'Paused';
    case 'done':
      return j.kind === 'run' ? 'Ended' : 'Done';
    case 'failed':
      if (j.error?.code === 'lease_held') return 'Another device holds this transfer';
      if (j.error?.code === 'source_changed') return 'The file changed; start again';
      return 'Failed';
    case 'cancelled':
      return 'Cancelled';
  }
}

/** The status tag's tone. */
export function stateTone(j: TransferJob): 'done' | 'progress' | 'failed' | 'neutral' {
  switch (j.state) {
    case 'done':
      return 'done';
    case 'running':
    case 'queued':
      return 'progress';
    case 'failed':
      return 'failed';
    default:
      return 'neutral';
  }
}

/** `REF=main` lines into params; blank lines are skipped, bad ones reported. */
export function parseParams(text: string): { params: Record<string, string>; error?: string } {
  const params: Record<string, string> = {};
  for (const raw of text.split('\n')) {
    const line = raw.trim();
    if (!line) continue;
    const eq = line.indexOf('=');
    const key = eq > 0 ? line.slice(0, eq).trim() : '';
    if (!/^[A-Z0-9_]{1,64}$/.test(key)) {
      return { params, error: `"${line}": use NAME=value with NAME in A-Z, 0-9 and _` };
    }
    params[key] = line.slice(eq + 1);
  }
  return { params };
}
