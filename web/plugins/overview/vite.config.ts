import { defineConfig } from 'vite-plus';

export default defineConfig({ test: { environment: 'jsdom', setupFiles: ['./test/setup.ts'] } });
