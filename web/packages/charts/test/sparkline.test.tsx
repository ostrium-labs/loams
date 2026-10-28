import { cleanup, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it } from 'vite-plus/test';
import { Sparkline, sparkPath } from '../src/sparkline';

afterEach(cleanup);

describe('Sparkline', () => {
  it('names the trend with latest, min and max', () => {
    render(
      <Sparkline
        points={[
          { t: 0, v: 2 },
          { t: 1, v: 8 },
          { t: 2, v: 5 },
        ]}
        ariaLabel="CPU"
      />,
    );
    const img = screen.getByRole('img', { name: 'CPU: latest 5, min 2, max 8' });
    expect(img.querySelectorAll('path')).toHaveLength(1);
  });

  it('draws one path per series and says so', () => {
    render(
      <Sparkline
        points={[
          { t: 0, v: 1, series: 'a' },
          { t: 1, v: 2, series: 'a' },
          { t: 0, v: 3, series: 'b' },
          { t: 1, v: 4, series: 'b' },
        ]}
        ariaLabel="Load"
      />,
    );
    const img = screen.getByRole('img', { name: /Load: a latest 2.*; b latest 4/ });
    expect(img.querySelectorAll('path')).toHaveLength(2);
  });

  it('renders an empty, still-labelled image with no points', () => {
    render(<Sparkline points={[]} ariaLabel="Idle" />);
    expect(screen.getByRole('img', { name: 'Idle: no data' }).querySelector('path')).toBeNull();
  });

  it('maps time to x and value to inverted y inside the inset', () => {
    const d = sparkPath(
      [
        { t: 0, v: 0 },
        { t: 10, v: 10 },
      ],
      24,
      { t0: 0, t1: 10, v0: 0, v1: 10 },
    );
    expect(d).toBe('M0.00,22.00 L100.00,2.00');
  });
});
