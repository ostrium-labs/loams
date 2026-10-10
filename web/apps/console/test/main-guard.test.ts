import { describe, expect, it } from 'vitest';
import desktopTs from '../src/cordis/desktop.ts?raw';
import mainTsx from '../src/cordis/main.tsx?raw';
import modulesTs from '../src/cordis/modules.ts?raw';
import startTs from '../src/cordis/start.ts?raw';

// Production builds must not contain the fake desktop bridge. Vite replaces
// `import.meta.env.DEV` with `false`, so the only safe form is a dynamic
// import inside a branch whose condition starts with that flag. (The built
// bundle is also grepped for "fake-desktop" in the task's verification.)
describe('fake desktop guard', () => {
  const src = mainTsx;

  it('main_imports_fake_desktop_only_behind_dev_flag', () => {
    const refs = src.split('\n').filter((l) => l.includes('fake-desktop'));
    expect(refs).toHaveLength(1);
    expect(refs[0]).toMatch(/import\('\.\/fake-desktop\.js'\)/);
    expect(src).toMatch(
      /if \(import\.meta\.env\.DEV && [^)]*\)[^{]*\{\s*[\s\S]*?import\('\.\/fake-desktop\.js'\)/,
    );
    expect(src).not.toMatch(/^import .*fake-desktop/m);
  });

  it('nothing_else_imports_the_fake', () => {
    const others = [startTs, desktopTs, modulesTs];
    for (const o of others) expect(o).not.toContain('fake-desktop');
  });
});
