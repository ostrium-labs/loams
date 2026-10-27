import { describe, expect, it } from 'vite-plus/test';
import { type Clock, createThrottle } from '../src/throttle';

function fakeClock() {
  let now = 0;
  const timers: { at: number; fn: () => void; id: number }[] = [];
  let nextId = 1;
  const clock: Clock = {
    now: () => now,
    setTimeout(fn, ms) {
      const id = nextId++;
      timers.push({ at: now + ms, fn, id });
      return id;
    },
    clearTimeout(id) {
      const i = timers.findIndex((t) => t.id === id);
      if (i >= 0) timers.splice(i, 1);
    },
  };
  const advance = (ms: number) => {
    now += ms;
    for (const t of [...timers].sort((a, b) => a.at - b.at)) {
      if (t.at <= now) {
        timers.splice(timers.indexOf(t), 1);
        t.fn();
      }
    }
  };
  return { clock, advance, pending: () => timers.length };
}

describe('createThrottle', () => {
  it('runs the first call at once and collapses the rest into one trailing run', () => {
    const { clock, advance } = fakeClock();
    let runs = 0;
    const t = createThrottle(() => runs++, 250, clock);
    t.schedule();
    expect(runs).toBe(1);
    advance(50);
    t.schedule();
    t.schedule();
    t.schedule();
    expect(runs).toBe(1);
    advance(200);
    expect(runs).toBe(2);
  });

  it('flush runs a pending call now; cancel drops it', () => {
    const { clock, advance, pending } = fakeClock();
    let runs = 0;
    const t = createThrottle(() => runs++, 250, clock);
    t.schedule();
    advance(10);
    t.schedule();
    t.flush();
    expect(runs).toBe(2);
    expect(pending()).toBe(0);
    advance(10);
    t.schedule();
    t.cancel();
    advance(1_000);
    expect(runs).toBe(2);
  });
});
