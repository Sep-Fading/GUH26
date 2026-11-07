// How the page looks. MiniClaw keeps the choices on the Mac, so every browser shows the same.
// A copy in this browser is applied before the first paint, so a reload does not flash the
// wrong theme; the Mac's copy wins whenever the two differ.

export type Appearance = { theme: 'system' | 'light' | 'dark';
  textSize: 'small' | 'default' | 'large';
  accent: 'blue' | 'green' | 'purple' | 'orange' | 'pink' | 'graphite' | 'white'; compact: boolean };

const fallback: Appearance = { theme: 'system', textSize: 'default', accent: 'blue', compact: false };
const key = 'miniclaw.appearance';
// Absent in the unit tests' DOM, where the theme then simply does not follow the system.
const system = typeof window.matchMedia === 'function'
  ? window.matchMedia('(prefers-color-scheme: dark)') : null;

// Reads this browser's copy; anything unexpected means the defaults.
function cached(): Appearance {
  try { return { ...fallback, ...JSON.parse(localStorage.getItem(key) ?? '{}') }; }
  catch { return fallback; }
}

class AppearanceState {
  value = $state<Appearance>(cached());
  error = $state('');

  // Writes the choices onto the document root, where the stylesheet reads them.
  apply() {
    const root = document.documentElement;
    const dark = this.value.theme === 'dark' || (this.value.theme === 'system' && system?.matches === true);
    root.classList.toggle('dark', dark);
    root.dataset.accent = this.value.accent;
    root.dataset.text = this.value.textSize;
    root.toggleAttribute('data-compact', this.value.compact);
  }

  // Applies this browser's copy at once, then takes the Mac's copy as the truth.
  async start() {
    this.apply();
    system?.addEventListener('change', () => this.apply());
    try {
      const response = await fetch('/api/settings/appearance');
      if (response.ok) this.keep(await response.json());
    } catch { /* The cached choices stay until the backend answers. */ }
  }

  // Shows a change at once and saves it on the Mac; a refused save is rolled back.
  async set(change: Partial<Appearance>) {
    const previous = this.value;
    this.keep({ ...previous, ...change });
    this.error = '';
    try {
      const response = await fetch('/api/settings/appearance', { method: 'POST',
        headers: { 'Content-Type': 'application/json', 'X-MiniClaw-Request': '1' },
        body: JSON.stringify(this.value) });
      if (!response.ok) throw new Error();
    } catch {
      this.keep(previous);
      this.error = 'Could not save appearance. The previous choice is kept.';
    }
  }

  private keep(next: Appearance) {
    this.value = next;
    try { localStorage.setItem(key, JSON.stringify(next)); } catch { /* Private mode. */ }
    this.apply();
  }
}

export const appearance = new AppearanceState();
