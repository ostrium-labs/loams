// A small human-readable preview for the common cron shapes (5 fields:
// minute hour day-of-month month day-of-week). Anything else falls back to
// "Custom schedule" so the preview never claims more than it knows.

const DAYS = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];
const pad = (n: string) => n.padStart(2, '0');

export function describeCron(expr: string): string | undefined {
  const f = expr.trim().split(/\s+/);
  if (f.length !== 5) return undefined;
  const [min = '', hour = '', dom = '', mon = '', dow = ''] = f;
  if (!f.every((x) => /^[\d*/,-]+$/.test(x))) return undefined;
  if (f.every((x) => x === '*')) return 'Every minute';
  const step = /^\*\/(\d+)$/.exec(min);
  if (step && hour === '*' && dom === '*' && mon === '*' && dow === '*')
    return `Every ${step[1]} minutes`;
  const at = /^\d+$/.test(min) && /^\d+$/.test(hour) ? `${pad(hour)}:${pad(min)}` : undefined;
  if (/^\d+$/.test(min) && hour === '*' && dom === '*' && mon === '*' && dow === '*')
    return `Every hour at minute ${min}`;
  if (at && dom === '*' && mon === '*') {
    if (dow === '*') return `Every day at ${at}`;
    if (/^[0-7]$/.test(dow)) return `Every ${DAYS[Number(dow) % 7]} at ${at}`;
    if (dow === '1-5') return `Weekdays at ${at}`;
  }
  if (at && /^\d+$/.test(dom) && mon === '*' && dow === '*')
    return `Day ${dom} of every month at ${at}`;
  return 'Custom schedule';
}
