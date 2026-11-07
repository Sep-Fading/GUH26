# Appearance sequence

```mermaid
sequenceDiagram
    participant macOS
    participant Main as main.ts
    participant Root as document root
    participant CSS as app.css variables
    participant UI as components

    Main->>macOS: matchMedia('(prefers-color-scheme: dark)')
    Main->>Root: toggle class "dark"
    Root->>CSS: .dark redefines the variable contract
    CSS-->>UI: Tailwind utilities and scoped CSS read the variables
    macOS-->>Main: change event
    Main->>Root: toggle class "dark"
```

| Participant | Source |
| --- | --- |
| main.ts | [main.ts](../../web/src/main.ts) `applyAppearance` |
| app.css variables | [app.css](../../web/src/app.css) `:root`, `.dark`, `@theme inline` |
| components | [web/src/lib/components/ui](../../web/src/lib/components/ui), feature components in [web/src](../../web/src) |

Back to [Frontend components and theme](README.md).

## Sidebar names

```mermaid
sequenceDiagram
    actor User
    participant Nav as TaskNav
    participant Name as ScrollingName
    participant Size as Svelte dimension bindings
    participant CSS as Component CSS

    Nav->>Name: Render project or conversation name
    Size-->>Name: Available width and natural text width
    Name->>CSS: Overflow distance and duration
    User->>Nav: Hover or keyboard focus
    alt Name fits
        CSS-->>User: Stationary name
    else Reduced motion requested
        CSS-->>User: Full name wrapped without animation
    else Name overflows
        CSS-->>User: Scroll to the end, pause and reverse
    end
    User->>Nav: Leave or blur
    CSS-->>User: Resting label with ellipsis
```

[TaskNav.svelte](../../web/src/TaskNav.svelte) supplies the names and buttons.
[ScrollingName.svelte](../../web/src/ScrollingName.svelte) owns the size bindings,
overflow calculation and scoped CSS. Svelte updates the bindings when the label or
its container changes size.
