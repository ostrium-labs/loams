import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite-plus';

export default defineConfig({
  resolve: {
    // The Live app's bundle (`live/factory.js`) runs in tests on a fake `loams:server`.
    alias: [
      {
        find: /^loams:server$/,
        replacement: fileURLToPath(new URL('./test/support/loams-server.ts', import.meta.url)),
      },
    ],
  },
  test: { include: ['test/**/*.test.ts'] },
});
