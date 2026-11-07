# Markdown in assistant messages

Assistant replies render as Markdown. Your own messages stay plain text, exactly as typed.
The renderer is [`message-markdown.ts`](../../web/src/message-markdown.ts); the component is
[`MessageMarkdown.svelte`](../../web/src/MessageMarkdown.svelte).

## Choices and their sources

- **Parser: markdown-it**, which the conversation rail already used. Verified in the installed
  package (`node_modules/markdown-it/dist/markdown-it.mjs`): with `html: false` every tag is
  escaped; `validateLink` refuses `javascript:`, `vbscript:`, `file:` and `data:` addresses
  except small image types; the `highlight` hook's result is inserted as is, so it must be
  escaped by the highlighter.
- **Highlighter: highlight.js 11** (BSD-3-Clause), loaded as `highlight.js/lib/core` with 13
  languages and common aliases, so the bundle does not carry every grammar. It escapes what it
  colours. An unknown language is escaped by markdown-it's own `escapeHtml`. This is a
  recommendation, not a measured comparison with other highlighters.

## Rules

| Input | Result |
|---|---|
| Raw HTML | Shown as text. It can never become an element. |
| Single line break | Kept (`breaks: true`), so a reply without Markdown looks as it did before. |
| Link | Only a well-formed `https:` address becomes a link. It opens in a new tab with `rel="noopener noreferrer nofollow"`, and its `title` is the real address, whatever the link text says. Anything else stays text. |
| Bare address | Linked only when it spells out `https://`. |
| Image | A button "Load image from <host>". The image loads only after a click, with no referrer. Non-https images show their alt text. |
| Code block | A bar with the language and a Copy button; highlighted when the language is known. |
| More than 200,000 characters | Escaped plain text, so one reply cannot stall the page. |

Images load on a click, not by themselves. A remote image makes the browser contact a server
with an address the model wrote, which can carry conversation content. The click keeps that
the user's decision, and keeps acceptance test UI-08 true. The page's Content-Security-Policy
now has `img-src 'self' data: https:` ([`api.rs`](../../server/src/api.rs)); scripts, frames
and connections are still limited to the app itself.

## Tests

- `web/tests/message-markdown.test.ts`: formatting, escaped HTML, link rules, image buttons,
  code escaping, language-name injection, the length fallback.
- `tests/browser/chat.spec.ts`: "assistant Markdown renders formatting, copyable code, real
  links and images only on request", and UI-08.
