package main

// Limits opt-in diagnostics to packet headers.
import (
	"fmt"
	"os"
	"sync/atomic"
)

var traced atomic.Uint32

// Prints at most 96 link/IP headers; transport payloads are never included.
func traceFrame(direction string, frame []byte, allowed bool) {
	if os.Getenv("MINICLAW_NETWORK_DIAGNOSTICS") != "1" || traced.Add(1) > 96 {
		return
	}
	size := min(len(frame), 34)
	fmt.Fprintf(os.Stderr, "network_header direction=%s allowed=%t bytes=%x\n", direction, allowed, frame[:size])
}
