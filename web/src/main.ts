// Mounts Svelte into the page container.
import { mount } from 'svelte';
// Supplies the root chat interface component.
import App from './App.svelte';
// Loads shared styles and Tailwind utilities.
import './app.css';
// Applies the saved theme, accent, text size and density.
import { appearance } from './appearance.svelte';

// shadcn-svelte components read a .dark class; the appearance state sets it, with the accent,
// text size and density, before the app mounts, so nothing flashes.
void appearance.start();

const target = document.getElementById('app');
if (!target) throw new Error('Missing app element');

mount(App, { target });
