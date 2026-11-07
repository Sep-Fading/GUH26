# Setup guides

Every value MiniClaw setup asks people to find or create, with the step-by-step guide setup shows for it.
Generated from `scripts/setup/guides.mjs`; regenerate with `node scripts/setup/guides.mjs --markdown > docs/setup-guides.md`.
The same text is shown by setup (choose "Show me how to find this" or type `?`), by `miniclaw guide <topic>`,
and in Settings > MCP servers ("How do I find …?").

## Inventory

| Topic | When it is asked | Why it is needed |
| --- | --- | --- |
| `computer-use` | macOS setup, when the VM is added: "Include Computer Use in the VM?" | Setup copies this OpenAI-signed app into the VM so agents there can use Computer Use. Your sign-in is not copied. |
| `provider-sign-in` | Setup lists providers that are "found, not signed in"; sign-in happens in MiniClaw afterwards. | MiniClaw uses your own Codex or Claude Code sign-in. There is no separate MiniClaw account. |

## Find Codex Computer Use.app (macOS)

Topic `computer-use`. The Codex app installs it at ~/.codex/computer-use/Codex Computer Use.app. Setup looks there first.

1. Install the Codex (ChatGPT) desktop app on this Mac and sign in. In the app open Plugins > Computer Use, choose Install plugin if asked, turn on the Computer Use server and skill, then choose Try now.
2. When macOS asks, allow Screen Recording and Accessibility for Codex Computer Use in System Settings > Privacy & Security.
3. The app is then at ~/.codex/computer-use/Codex Computer Use.app, which is /Users/&lt;you>/.codex/computer-use/Codex Computer Use.app.
4. Finder hides folders whose names start with a dot. To look: in Finder choose Go > Go to Folder, type ~/.codex/computer-use and press Return.
5. Or in Terminal run this; it prints the app if it is there:

   ```
   ls -d ~/.codex/computer-use/*.app
   ```

6. Paste the full path at the setup prompt, starting with /. Setup checks OpenAI's code signature before it copies the app into the VM.

Sources: <https://learn.chatgpt.com/docs/computer-use>, <https://support.apple.com/guide/mac-help/mchlp1236/mac>

## Find the Codex Computer Use helper (Windows)

Topic `computer-use`. The file codex-computer-use[-arm64].exe inside the Codex app's package folder, under C:\Program Files\WindowsApps. Setup looks there first.

1. Install the Codex (ChatGPT) desktop app from OpenAI and sign in. In the app open Plugins > Computer Use, choose Install plugin if asked, and turn on the Computer Use server and skill.
2. The helper is codex-computer-use-arm64.exe on ARM64 PCs or codex-computer-use.exe on x64 PCs (Settings > System > About > System type says which), typically in C:\Program Files\WindowsApps\OpenAI.Codex_&lt;version>_arm64__2p2nqsd0c76g0\app\resources\cua_node\bin\node_modules\@oai\cua\bin\windows. &lt;version> changes with every app update.
3. Windows hides the WindowsApps folder, so ask Windows instead. In PowerShell (as yourself, not as administrator) run this; it prints the full path:

   ```
   (Get-ChildItem (Join-Path (Get-AppxPackage -Name OpenAI.Codex).InstallLocation 'app\resources\cua_node\bin\node_modules\@oai\cua\bin\windows') -Filter 'codex-computer-use*.exe').FullName
   ```

4. An error or no output means the Codex app is not installed for your account. Test-Path followed by the path in quotes prints True when the file is there.
5. Paste the path at the setup prompt; quotes from "Copy as path" are fine. Setup checks its OpenAI signature and saves your choice. After a Codex app update MiniClaw finds the helper in the new version when it starts and before each use, and checks the signature again; if it cannot, the Tools menu says why.

Sources: <https://learn.chatgpt.com/docs/computer-use>, <https://learn.microsoft.com/en-us/powershell/module/appx/get-appxpackage>

## Sign in to Codex or Claude (macOS)

Topic `provider-sign-in`. Start MiniClaw, open Settings > Providers and press Sign in.

1. Start MiniClaw (miniclaw) and open the address setup printed, http://127.0.0.1:&lt;port>.
2. Open Settings > Providers and press Sign in next to Codex (ChatGPT) or Claude Code.
3. Codex (ChatGPT): finish the sign-in in the browser page that opens.
4. Claude Code: Claude Code opens its sign-in page in your browser and MiniClaw notices the sign-in by itself. If nothing opened (the page may also be behind other windows), use "Open the Claude sign-in page" under Claude Code; that page ends with a code to paste into "Code from Claude", then press Finish sign-in.
5. Cancel under Claude Code stops a Claude sign-in; if Claude Code stops with an error, MiniClaw shows it there. MiniClaw reuses the credentials Codex or Claude Code store themselves.
6. Already signed in with the codex or claude command on this Mac? Nothing more is needed.

Sources: <https://developers.openai.com/codex/auth>, <https://docs.claude.com/en/docs/claude-code/setup>

## Sign in to Codex or Claude (Windows)

Topic `provider-sign-in`. Start MiniClaw, open Settings > Providers and press Sign in.

1. Start MiniClaw (.\miniclaw.cmd) and open the address setup printed, http://127.0.0.1:&lt;port>.
2. Open Settings > Providers and press Sign in next to Codex (ChatGPT) or Claude Code.
3. Codex (ChatGPT): finish the sign-in in the browser page that opens.
4. Claude Code: Claude Code opens its sign-in page in your browser and MiniClaw notices the sign-in by itself. If nothing opened (the page may also be behind other windows), use "Open the Claude sign-in page" under Claude Code; that page ends with a code to paste into "Code from Claude", then press Finish sign-in.
5. Cancel under Claude Code stops a Claude sign-in; if Claude Code stops with an error, MiniClaw shows it there. MiniClaw reuses the credentials Codex or Claude Code store themselves.
6. Already signed in with the codex or claude command on this PC? Nothing more is needed.

Sources: <https://developers.openai.com/codex/auth>, <https://docs.claude.com/en/docs/claude-code/setup>
