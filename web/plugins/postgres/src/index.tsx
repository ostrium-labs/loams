// @loams/plugin-postgres: the local Neon stack page (branches, connect, SQL) and the
// loams.postgres.v1 control plane tab (projects, branches, roles, databases, operations).

import { type PluginModule, service } from '@loams/console-host';
import type { Context } from '@loams/cordis';
// Brings in the `desktop` service type (a module augmentation of `Services`).
import type {} from '@loams/platform-electron';
import { createPgControlSource, PostgresRoute } from './route.js';

export { Branches } from './branches.js';
export { BranchAccess } from './control/BranchAccess.js';
export { BranchTree } from './control/BranchTree.js';
export { ControlPlane } from './control/ControlPlane.js';
export {
  createConnectPgControl,
  PgControlUnreachable,
  resolvePgControl,
} from './control/connect.js';
export { fixtureState } from './control/fixtures.js';
export { createMockPgControl } from './control/mock.js';
export { Operations } from './control/Operations.js';
export { PasswordOnce } from './control/PasswordOnce.js';
export { branchTree, deleteBlocker } from './control/tree.js';
export * from './control/types.js';
export { PostgresPage } from './page.js';
export { createPgControlSource, type PgControlSource, PostgresRoute } from './route.js';
export { flattenTimelines } from './tree.js';

const plugin: PluginModule = {
  name: 'postgres',
  inject: ['desktop', 'router', 'slots'],
  apply(ctx: Context) {
    const desktop = service(ctx, 'desktop');
    const router = service(ctx, 'router');
    const slots = service(ctx, 'slots');
    // loams.postgres.v1 is pg-control's, on its own listener (PG2 R9.1), not the engine's:
    // Electron main forwards loams-app://console/pg-control/ there with the API key. When
    // nothing answers, the tab runs on the fixture-backed mock and says so. A change saved in
    // App settings > Postgres drops the client, and the page probes again.
    const source = createPgControlSource(
      desktop,
      `${globalThis.location?.origin ?? 'loams-app://console'}/pg-control/`,
    );
    ctx.effect(() => () => source.dispose());
    ctx.effect(() =>
      router.page(
        { id: 'postgres', path: '/postgres', title: 'Postgres', plugin: 'postgres' },
        () => <PostgresRoute desktop={desktop} source={source} />,
      ),
    );
    ctx.effect(() =>
      slots.register(
        {
          name: 'shell.nav.section',
          plugin: 'postgres',
          order: 30,
          meta: {
            id: 'postgres',
            label: 'Postgres',
            href: '/postgres',
            icon: 'postgres',
            group: 'Data',
          },
        },
        () => null,
      ),
    );
  },
};

export default plugin;
