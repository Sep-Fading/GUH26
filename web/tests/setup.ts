// Adds accessible DOM assertions to tests.
import '@testing-library/jest-dom/vitest';

// jsdom has no layout; browser tests verify the actual size bindings.
globalThis.ResizeObserver ??= class {
  // Accepts observation without inventing layout measurements.
  observe() {}
  // Stops observation in this layout-free environment.
  unobserve() {}
  // Releases the test observer when components unmount.
  disconnect() {}
};

// jsdom names the machine running the tests in its user agent, so the page's first guess would
// follow it. Tests expect the macOS wording unless they set another platform themselves.
import { beforeEach } from 'vitest';
import { setHost } from '../src/platform.svelte';
beforeEach(() => setHost('macos'));
