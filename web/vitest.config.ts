// Defines frontend test discovery and environment.
import { defineConfig } from 'vitest/config';
// Compiles Svelte components for unit tests.
import { svelte } from '@sveltejs/vite-plugin-svelte';
// Enables browser resolution and automatic test cleanup.
import { svelteTesting } from '@testing-library/svelte/vite';
// Resolves the shared component library alias.
import { fileURLToPath } from 'node:url';

const lib = fileURLToPath(new URL('./src/lib', import.meta.url));

export default defineConfig({
  resolve: { alias: { $lib: lib } },
  plugins: [svelte(), svelteTesting()],
  test: {
    environment: 'jsdom',
    include: ['web/tests/**/*.test.ts'],
    setupFiles: ['web/tests/setup.ts'],
  },
});
