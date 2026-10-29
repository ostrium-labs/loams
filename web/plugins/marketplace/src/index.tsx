// @loams/plugin-marketplace: the Seed Exchange, the Loams marketplace, in the console
// and in Studio (design §73 §7.1–§7.2, D1465, D1466; plan MK1 Task 9). Browse with
// filters, a listing in plain words, the consent screen made from agentd's
// `PlanInstall`, the installed listings with updates and revocation banners, and the
// sources. Experimental: MK0 lists first-party listings only.
//
// The same pages answer at `/marketplace…` in Console and `/studio/marketplace…` in
// Studio, whose harness picker links to `/studio/marketplace?kind=harness`.

import { type PluginModule, service } from '@loams/console-host';
import type { Context } from '@loams/cordis';
import { CONSOLE_BASE, PAGES, STUDIO_BASE } from './paths.js';
import { MarketplaceScope, MarketplaceView } from './view.js';

export { connectMarketplace, type MarketplaceApi } from './api.js';
export { ConsentScreen, type ConsentScreenProps, dataScopes } from './consent.js';
export { MarketplaceApiProvider } from './data.js';
export { type MarketplaceError, marketplaceError } from './errors.js';
export {
  BrowsePage,
  filtersOf,
  InstalledPage,
  InstallPage,
  ListingPage,
  RevocationBanner,
  SourcesPage,
} from './pages.js';
export { CONSOLE_BASE, href, PAGES, type Page, STUDIO_BASE } from './paths.js';
export { MarketplaceScope, MarketplaceView } from './view.js';
export * from './wire.js';

const plugin: PluginModule = {
  name: 'marketplace',
  inject: ['router', 'slots', 'agentd'],
  apply(ctx: Context) {
    const router = service(ctx, 'router');
    const slots = service(ctx, 'slots');
    const agentd = service(ctx, 'agentd');
    const navigate = (to: string) => router.navigate(to);
    for (const [base, mode] of [
      [STUDIO_BASE, 'studio'],
      [CONSOLE_BASE, 'console'],
    ] as const) {
      for (const [suffix, page, title] of PAGES) {
        ctx.effect(() =>
          router.page(
            {
              id: `marketplace-${mode}-${page}`,
              path: `${base}${suffix}`,
              title,
              plugin: 'marketplace',
            },
            ({ params }) => (
              <MarketplaceScope agentd={agentd}>
                <MarketplaceView page={page} base={base} params={params} navigate={navigate} />
              </MarketplaceScope>
            ),
          ),
        );
      }
      ctx.effect(() =>
        slots.register(
          {
            name: 'shell.nav.section',
            plugin: 'marketplace',
            order: mode === 'studio' ? 80 : 60,
            meta: {
              id: `marketplace-${mode}`,
              label: 'The Seed Exchange',
              href: base,
              icon: 'plugin',
              group: mode === 'studio' ? 'Studio' : 'Integrate',
              mode,
            },
          },
          () => null,
        ),
      );
    }
  },
};

export default plugin;
