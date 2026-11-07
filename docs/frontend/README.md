# Frontend components and theme

The page is Svelte 5 with Tailwind v4. Its components come from
[shadcn-svelte](https://www.shadcn-svelte.com/docs/installation/vite), which is built on
Bits UI, a library the app already used. shadcn-svelte copies component source into the
repository; at runtime it adds only small helpers (`tailwind-variants`, `tw-animate-css`,
`cn`, and the `shadcn-svelte` Tailwind stylesheet).

See the [appearance sequence](sequence.md).

## Setup

[components.json](../../components.json) sits at the repository root, because the CLI reads
`package.json` from its working directory. It points the CLI at the web workspace:

```json
"typescript": {
	"config": "web/tsconfig.json"
},
```

Components are generated into `web/src/lib/components/ui` and imported through the `$lib`
alias, which is declared in [web/tsconfig.json](../../web/tsconfig.json),
[vite.config.ts](../../web/vite.config.ts) and
[vitest.config.ts](../../web/vitest.config.ts).

`npx shadcn-svelte@latest add <name>` adds a component without prompts. `init` asks for a
preset and has no flag for it, so it only runs in a terminal. It was run once with style
`vega` and base colour `neutral`; it also switched the font to Inter, which was reverted.

Generated components are project source. One is changed on purpose: the
[switch](../../web/src/lib/components/ui/switch/switch.svelte) thumb is white in both
themes.

## Tokens

[app.css](../../web/src/app.css) fills shadcn-svelte's variable contract (`--background`,
`--foreground`, `--primary`, `--muted`, `--accent`, `--border`, `--ring` and the rest) with
a macOS-like palette. Values are hex so that computed styles stay in `rgb()`, which the
browser tests compare.

Two variables are MiniClaw's own: `--brand`, a blue used for the user's message bubble, the
send button and focus rings, and `--success`. Radii derive from `--radius: 0.75rem`. The
font is the system stack, starting with `-apple-system`.

shadcn's names differ from the ones the app used before. `muted` and `accent` are
background colours here; secondary text is `text-muted-foreground`. Component-scoped CSS
reads the variables directly, for example:

```css
/* web/src/ConversationRail.svelte */
.conversation-rail :global(.rail-mark:focus-visible span) { background: var(--foreground); }
```

## Dark mode

shadcn-svelte components switch on a `.dark` class. [main.ts](../../web/src/main.ts) keeps
that class in step with the system appearance:

```ts
// web/src/main.ts
const applyAppearance = () => document.documentElement.classList.toggle('dark', appearance.matches);
```

`.dark` redefines the same variables, so components need no separate dark styles.

## Settings

One gear in the header opens [Settings](../../web/src/Settings.svelte), a sheet with a
category sidebar: Appearance, Providers, Agents, Skills and Virtualisation. Agents, skills
and providers are panels that load their own data when shown; they are no longer separate
dialogs. The header keeps the gear, notifications and the current conversation's
permissions. A form that is saving keeps the sheet open.

[Appearance](../../web/src/AppearanceSettings.svelte) offers theme (match macOS, light,
dark), accent colour, text size and a compact layout.
[appearance.svelte.ts](../../web/src/appearance.svelte.ts) writes the choices onto the
document root as the `dark` class and `data-accent`, `data-text` and `data-compact`
attributes, which [app.css](../../web/src/app.css) reads. The choices are saved on the Mac
(`GET` and `POST /api/settings/appearance`, [appearance.rs](../../server/src/appearance.rs),
a closed set of names in `<data>/appearance.json`). A copy in the browser is applied before
the app mounts, so a reload does not flash the wrong theme; the Mac's copy wins when they
differ. The route needs no sign-in, so the page can theme itself first.

[Virtualisation](../../web/src/Virtualisation.svelte) shows the Master VM's state, the
share of the Mac that all VMs together may use with editable limits, the recorded VMs, and
buttons for the ledger's reconcile and forget. It says when the VM's worker is older than
this MiniClaw and names the `miniclaw update-vm` steps. The backend reaches the ledger
through four signed requests to the VM session (see
[Host resource admission](../resource-admission/README.md)).

## Layout rules

- The VM panel is a full-height column beside the chat from 1024 px; below that it stacks
  above the chat. See [VM desktop stream](../desktop-stream/README.md).
- The conversation rail sits beside the chat at every width. Stacked above it, the rail
  took the chat's height and pushed the last message under the composer.
- The header is one row at every width. Below 640 px its tools scroll sideways, and the
  connection text is left to screen readers below 768 px.
- The unread badge sits inside the notification button, because a scrolling container
  clips anything that extends beyond it.
- Messages fade out above the composer (`.conversation-scroll`).

## Sidebar names

[TaskNav.svelte](../../web/src/TaskNav.svelte) uses
[ScrollingName.svelte](../../web/src/ScrollingName.svelte) for project names and both
grouped and independent conversations. The resting label uses an ellipsis. Svelte's
[dimension bindings](https://svelte.dev/docs/svelte/bind#Dimensions) measure the available width and the text's natural width, updating
when the label or sidebar size changes:

```ts
  const distance = $derived(Math.max(0, fullWidth - available));
```

When a name overflows, hovering its button or focusing it with the keyboard starts a
horizontal animation. The ending reaches the label's right edge; the animation pauses
at either end and reverses. Leaving or blurring restores the ellipsis. Short names do
not animate. The moving copy is hidden from accessibility APIs, while the static copy
keeps the button's full accessible name. With reduced motion, the static label wraps
to reveal the full text without animation. See the [sequence](sequence.md#sidebar-names).

The four [sidebar name tests](../../tests/browser/sidebar-names.spec.ts) pass at desktop
and mobile sizes. They check actual movement and the ending's position, reset, short
names, keyboard focus, accessible names, reduced motion and horizontal overflow.

## Verification

Browser tests pin the colours and shapes that matter:
[message-bubbles.spec.ts](../../tests/browser/message-bubbles.spec.ts) (bubble colour and
radius), [conversation-rail.spec.ts](../../tests/browser/conversation-rail.spec.ts)
(current mark and preview text colour),
[notifications.spec.ts](../../tests/browser/notifications.spec.ts) (badge inside its
button), [projects.spec.ts](../../tests/browser/projects.spec.ts) (destructive colour) and
[environments.spec.ts](../../tests/browser/environments.spec.ts) (start prompt button).
Layout was checked by screenshot at 1440, 1100, 900, 735, 560 and 390 px; those
screenshots are not kept as tests.

The settings screens use shadcn-svelte `Button`, `Input`, `Textarea`, `Switch` and
`Dialog`. Native checkboxes, radios and selects stay native and take the theme's colours
through `accent-color`. The remaining Bits UI uses are tooltips, the project and
confirmation dialogs, the composer's `Choice` menus and the conversation rail.
