import { describe, expect, it } from 'vitest';
import {
  addAmounts,
  formatAmount,
  formatMoney,
  formatPercent,
  fromNanos,
  money,
  moneyInputValue,
  parseMoneyInput,
  ratio,
  toNanos,
} from '../src/format.js';

describe('amounts', () => {
  it('parse and print exactly, as loams-meter-core does', () => {
    expect(toNanos('12')).toBe(12_000_000_000n);
    expect(toNanos('0.5')).toBe(500_000_000n);
    expect(toNanos('-3.000000001')).toBe(-3_000_000_001n);
    expect(fromNanos(toNanos('-3.000000001'))).toBe('-3.000000001');
    expect(fromNanos(12_500_000_000n)).toBe('12.5');
    expect(addAmounts('0.1', '0.2')).toBe('0.3');
    expect(() => toNanos('1e3')).toThrow();
    expect(() => toNanos('0.0000000001')).toThrow();
  });

  it('show units a person reads', () => {
    expect(formatAmount('36000000', 'cu_ms', 'en')).toBe('10 CPU-h');
    expect(formatAmount('1500000', 'tokens', 'en')).toBe('1,500,000 tokens');
    expect(formatAmount('2500000000', 'bytes', 'en')).toBe('2.5 GB');
    expect(formatAmount('7', 'widgets', 'en')).toBe('7 widgets');
  });

  it('take ratios for bars, never above what was used', () => {
    expect(ratio('30600000', '36000000')).toBeCloseTo(0.85, 6);
    expect(ratio('5', '0')).toBe(0);
    expect(formatPercent(0.7999)).toBe('79%');
    expect(formatPercent(1)).toBe('100%');
  });
});

describe('money', () => {
  it('formats micros with the currency, exactly', () => {
    expect(formatMoney({ micros: '12500000', currency: 'EUR' }, 'en')).toBe('€12.50');
    expect(formatMoney({ micros: '1', currency: 'USD' }, 'en')).toBe('$0.000001');
    expect(formatMoney({ micros: '9007199254740993000000', currency: 'USD' }, 'en')).toBe(
      '$9,007,199,254,740,993.00',
    );
  });

  it('parses what a person types into micros', () => {
    expect(parseMoneyInput('99.5', 'EUR')).toEqual({ micros: '99500000', currency: 'EUR' });
    expect(parseMoneyInput('1,200', 'EUR')).toEqual({ micros: '1200000000', currency: 'EUR' });
    expect(parseMoneyInput('-1', 'EUR')).toBeUndefined();
    expect(parseMoneyInput('abc', 'EUR')).toBeUndefined();
    expect(moneyInputValue(money('40.25'))).toBe('40.25');
    expect(moneyInputValue(money(40))).toBe('40');
  });
});
