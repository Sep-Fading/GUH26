// Defines frontend build plugins and server restrictions.
import { defineConfig } from 'vite';
// Compiles Svelte components during frontend builds.
import { svelte } from '@sveltejs/vite-plugin-svelte';
// Generates Tailwind styles during frontend builds.
import tailwindcss from '@tailwindcss/vite';
// Resolves the shared component library alias.
import { fileURLToPath } from 'node:url';

const lib = fileURLToPath(new URL('./src/lib', import.meta.url));

function separateServer(): never {
  throw new Error('Use npm start or npm run dev. Rust serves the UI and API together.');
}

export default defineConfig({
  resolve: { alias: { $lib: lib } },
  plugins: [
    svelte(),
    tailwindcss(),
    {
      name: 'miniclaw-local-only',
      configureServer: separateServer,
      configurePreviewServer: separateServer,
    },
  ],
});
