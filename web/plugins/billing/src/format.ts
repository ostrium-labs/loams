// Amounts and money, exactly: an Amount is a decimal string with up to 9 decimals and money is
// integer micros (D374). Both are parsed into bigints, so nothing billed passes through a float;
// only ratios for bars and banners do.

import type { Amount, Money, Unit } from './types.js';

const SCALE = 1_000_000_000n;

/** `amount` × 10^9, as loams-meter-core's `Amount::raw`. Throws on anything but a decimal. */
export function toNanos(amount: Amount): bigint {
  const m = /^(-)?(\d+)(?:\.(\d{1,9}))?$/.exec(amount.trim());
  if (!m) throw new Error(`not an amount: ${JSON.stringify(amount)}`);
  const whole = BigInt(m[2] ?? '0');
  const frac = BigInt((m[3] ?? '').padEnd(9, '0'));
  const value = whole * SCALE + frac;
  return m[1] ? -value : value;
}

/** The canonical decimal of `nanos` / 10^9, as `Amount`'s Display. */
export function fromNanos(nanos: bigint): Amount {
  const negative = nanos < 0n;
  const abs = negative ? -nanos : nanos;
  const whole = abs / SCALE;
  const frac = (abs % SCALE).toString().padStart(9, '0').replace(/0+$/, '');
  return `${negative ? '-' : ''}${whole}${frac ? `.${frac}` : ''}`;
}

export function addAmounts(...amounts: Amount[]): Amount {
  return fromNanos(amounts.reduce((sum, a) => sum + toNanos(a), 0n));
}

/** `part / whole`, 0 when `whole` is not positive. For bars and thresholds only. */
export function ratio(part: Amount, whole: Amount): number {
  const w = toNanos(whole);
  if (w <= 0n) return 0;
  // Basis points of basis points keep four decimals of the ratio without a float in the division.
  return Number((toNanos(part) * 1_000_000n) / w) / 1_000_000;
}

export function moneyRatio(part: Money, whole: Money): number {
  const w = BigInt(whole.micros);
  if (w <= 0n || part.currency !== whole.currency) return 0;
  return Number((BigInt(part.micros) * 1_000_000n) / w) / 1_000_000;
}

/** "€12.50": micros formatted with the currency, exactly (Intl formats a decimal string). */
export function formatMoney(money: Money, locale?: string): string {
  const micros = BigInt(money.micros);
  const negative = micros < 0n;
  const abs = negative ? -micros : micros;
  const decimal = `${negative ? '-' : ''}${abs / 1_000_000n}.${(abs % 1_000_000n)
    .toString()
    .padStart(6, '0')}` as Intl.StringNumericLiteral;
  return new Intl.NumberFormat(locale, {
    style: 'currency',
    currency: money.currency,
    maximumFractionDigits: 6,
  }).format(decimal);
}

/** Money from whole currency units and micros, for fixtures and inputs. */
export function money(units: number | string, currency = 'EUR'): Money {
  const nanos = toNanos(String(units));
  return { micros: (nanos / 1000n).toString(), currency };
}

/** Parses what a person typed as an amount of money ("120", "99.5") into micros. */
export function parseMoneyInput(text: string, currency: string): Money | undefined {
  const t = text.trim().replace(/,/g, '');
  if (!/^\d+(?:\.\d{1,6})?$/.test(t)) return undefined;
  return money(t, currency);
}

/** Whole currency units and decimals of `money`, for an input's value. */
export function moneyInputValue(m: Money | undefined): string {
  if (!m) return '';
  const micros = BigInt(m.micros);
  const whole = micros / 1_000_000n;
  const frac = (micros % 1_000_000n).toString().padStart(6, '0').replace(/0+$/, '');
  return frac ? `${whole}.${frac}` : `${whole}`;
}

const UNIT_LABEL: Record<string, { label: string; divisor?: bigint; suffix: string }> = {
  count: { label: 'count', suffix: '' },
  requests: { label: 'requests', suffix: ' requests' },
  tokens: { label: 'tokens', suffix: ' tokens' },
  ms: { label: 'hours', divisor: 3_600_000n, suffix: ' h' },
  cu_ms: { label: 'CPU-hours', divisor: 3_600_000n, suffix: ' CPU-h' },
  bytes: { label: 'GB', divisor: 1_000_000_000n, suffix: ' GB' },
  byte_hours: { label: 'GB-hours', divisor: 1_000_000_000n, suffix: ' GB-h' },
};

/** The unit a person reads `unit` in ("CPU-hours" for `cu_ms`). */
export function unitLabel(unit: Unit): string {
  return UNIT_LABEL[unit]?.label ?? unit;
}

/**
 * `amount` of `unit` for people: milliseconds as hours, bytes as GB, counts grouped, at most
 * two decimals. Display only; the exact amount stays the API's.
 */
export function formatAmount(amount: Amount, unit: Unit, locale?: string): string {
  const spec = UNIT_LABEL[unit];
  const divisor = spec?.divisor ?? 1n;
  const nanos = toNanos(amount);
  const negative = nanos < 0n;
  // Hundredths of the displayed unit, rounded half away from zero.
  const den = SCALE * divisor;
  const hundredths = ((negative ? -nanos : nanos) * 100n + den / 2n) / den;
  const value = `${negative ? '-' : ''}${hundredths / 100n}.${(hundredths % 100n)
    .toString()
    .padStart(2, '0')}` as Intl.StringNumericLiteral;
  const text = new Intl.NumberFormat(locale, { maximumFractionDigits: 2 }).format(value);
  return `${text}${spec ? spec.suffix : ` ${unit}`}`;
}

/** "42%", rounded down, so 79.9% never reads as 80%. */
export function formatPercent(r: number): string {
  return `${Math.floor(r * 100)}%`;
}

/** "1 Nov 2026" in UTC: periods and expiries are UTC. */
export function formatDay(iso: string, locale?: string): string {
  return new Intl.DateTimeFormat(locale, {
    day: 'numeric',
    month: 'short',
    year: 'numeric',
    timeZone: 'UTC',
  }).format(new Date(iso));
}

/** "1 Nov 2026, 14:05 UTC". */
export function formatTime(iso: string, locale?: string): string {
  return `${new Intl.DateTimeFormat(locale, {
    day: 'numeric',
    month: 'short',
    year: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
    hourCycle: 'h23',
    timeZone: 'UTC',
  }).format(new Date(iso))} UTC`;
}
