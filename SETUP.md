# Run it from source

Follow the section for your computer. When you're done, MiniClaw is open at http://127.0.0.1:8787
and the swarm is under **Swarm** in the sidebar. The first start compiles everything and takes a
few minutes; after that it starts in seconds.

You need at least one of the two AI tools, **Codex** or **Claude Code**, signed in with your own
account. Both is best.

---

## Linux (Ubuntu or Debian)

1. Install the basics:

       sudo apt update && sudo apt install -y git curl build-essential bubblewrap

2. Install Rust, then load it into this terminal:

       curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
       source "$HOME/.cargo/env"

3. Install Node.js 24:

       curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.3/install.sh | bash
       source ~/.nvm/nvm.sh
       nvm install 24

4. Install and sign in to the AI tools (one is enough):

       npm install -g @openai/codex && codex login
       curl -fsSL https://claude.ai/install.sh | bash && claude

   For Claude, sign in when it asks, then type `/exit`.

5. Check that the sandbox works:

       bwrap --unshare-all --ro-bind / / true && echo "sandbox ok"

   If it doesn't print `sandbox ok` (Ubuntu 24.04 can block it), run
   `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0` and try again.

6. Get the code and start it:

       git clone https://github.com/Sep-Fading/GUN26.git
       cd GUN26
       npm ci
       npm start

7. Open http://127.0.0.1:8787 in your browser.

---

## Windows (WSL)

1. Open **PowerShell as administrator** and run:

       wsl --install -d Ubuntu

   Restart the PC if it asks. Open **Ubuntu** from the Start menu and choose a username and
   password.

2. In the Ubuntu window, do **steps 1 to 6 of the Linux section above**. Clone into your Ubuntu
   home folder (the default when you open Ubuntu), not into `/mnt/c/...`.

3. Open http://127.0.0.1:8787 in your normal Windows browser.

If `codex login` or `claude` doesn't open a browser, copy the link it prints into your Windows
browser.

---

## macOS (Apple silicon)

1. Install Apple's command-line tools (click Install in the window that opens):

       xcode-select --install

2. Install Rust, then load it into this terminal:

       curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
       source "$HOME/.cargo/env"

3. Install Node.js 24:

       curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.40.3/install.sh | bash
       source ~/.nvm/nvm.sh
       nvm install 24

4. Install and sign in to the AI tools (one is enough):

       npm install -g @openai/codex && codex login
       curl -fsSL https://claude.ai/install.sh | bash && claude

   For Claude, sign in when it asks, then type `/exit`. If you use the Codex desktop app, its
   sign-in works too.

5. Get the code and start it:

       git clone https://github.com/Sep-Fading/GUN26.git
       cd GUN26
       npm ci
       npm start

6. Open http://127.0.0.1:8787 in your browser.

---

## If something goes wrong

- **Port already in use:** start on another port with `MINICLAW_PORT=8790 npm start`, then open
  http://127.0.0.1:8790.
- **"Codex/Claude unavailable" at the top of the page:** that tool isn't installed or isn't signed
  in. Do step 4 again, stop MiniClaw with Ctrl+C, and run `npm start` again.
- **Stopping it:** press Ctrl+C in the terminal running `npm start`.
- **Native Windows without WSL** starts too, but agents can only run commands with **Unsandboxed
  shell** switched on. See [docs/swarm](docs/swarm/README.md).
