// The Electron preload's bridge; absent in a browser.
import type { LoamsDesktopApi } from '@loams/platform-electron';

declare global {
  var loamsDesktop: LoamsDesktopApi | undefined;
}
