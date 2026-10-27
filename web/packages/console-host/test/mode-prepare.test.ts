import { describe, expect, it, vi } from 'vite-plus/test';
import { ModeController } from '../src/mode.js';

describe('ModeController.prepare (W10)', () => {
  it("loads another mode's plugins once without switching to it", () => {
    const enter = vi.fn();
    const navigate = vi.fn();
    const mode = new ModeController({
      current: 'console',
      available: ['console', 'studio'],
      modes: new Map(),
      navigate,
      enter,
    });
    mode.prepare('studio');
    mode.prepare('studio');
    expect(enter).toHaveBeenCalledOnce();
    expect(mode.current).toBe('console');
    expect(navigate).not.toHaveBeenCalled();
    mode.set('studio');
    expect(enter).toHaveBeenCalledOnce(); // already entered
  });

  it('does nothing for a mode that is not available (the web gate, SD1 Task 15)', () => {
    const enter = vi.fn();
    const mode = new ModeController({
      current: 'console',
      available: ['console'],
      modes: new Map(),
      navigate: vi.fn(),
      enter,
    });
    mode.prepare('studio');
    expect(enter).not.toHaveBeenCalled();
  });
});
