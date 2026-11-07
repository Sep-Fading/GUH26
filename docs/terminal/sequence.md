# Terminal: sequence

Back to [Terminal](README.md).

```mermaid
sequenceDiagram
    participant P as Page (Terminal.svelte)
    participant A as api/terminal.rs
    participant T as terminal.rs
    participant Z as zsh under Seatbelt

    P->>A: POST terminal/open {cols, rows}
    A->>T: ensure(thread, spec: cwd, grants, hosts)
    T->>Z: openpty, sandbox-exec -p profile -D TTY=/dev/ttysN -- /bin/zsh -i
    A-->>P: {seq, status: live}
    P->>A: GET terminal/stream?after=seq (EventSource)
    Z-->>T: prompt, with the invisible prompt marker
    T-->>A: ring bumped
    A-->>P: output (base64, id = offset)
    P->>A: POST terminal/input {"cat notes.txt\r"}
    A->>Z: write to the terminal
    Z-->>P: echo and output, through the ring and the stream
    P->>A: POST terminal/input {"exit\r"}
    Z-->>T: EOF, exit code recorded
    A-->>P: exit {code}
    P->>A: POST terminal/open (next time)
    T->>Z: a fresh shell, empty ring
```

## An agent command

```mermaid
sequenceDiagram
    participant M as Model
    participant C as chat.rs scoped_tool_result
    participant U as User (popup)
    participant T as terminal.rs run
    participant Z as zsh
    participant P as Page

    M->>C: run_shell {text, cwd, hosts}
    C->>U: approval popup (as before)
    U-->>C: approve
    C->>T: run(call, text, cwd, hosts)
    T->>T: wait for the prompt marker
    T->>Z: write run/<call>.zsh, type Ctrl-U __mc <call>
    Z-->>P: dimmed command line, then output (via the ring)
    Z-->>T: start marker ... output ... end marker;code
    T-->>C: Ran {output <= 60 KiB, code, refused}
    C-->>M: Exit code, stdout (one-shot shape)
    C-->>P: Activity entry finished
    Note over T,C: past 120 s: partial result now, Activity finished later
```
