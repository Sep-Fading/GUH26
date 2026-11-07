package main

// Bounds descriptors and handles owned process shutdown.
import (
	"context"
	"fmt"
	"os"
	"os/signal"
	"syscall"
)

// Runs the private network helper until shutdown or a forwarding failure.
func main() {
	if len(os.Args) != 2 {
		fmt.Fprintln(os.Stderr, "Expected private network directory")
		os.Exit(1)
	}
	if err := syscall.Setrlimit(syscall.RLIMIT_NOFILE, &syscall.Rlimit{Cur: 512, Max: 512}); err != nil {
		fmt.Fprintln(os.Stderr, "Cannot bound network descriptors")
		os.Exit(1)
	}
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	network, err := startNetwork(ctx, os.Args[1])
	if err != nil {
		fmt.Fprintln(os.Stderr, "Network startup failed:", err)
		os.Exit(1)
	}
	defer network.Close()
	fmt.Println("{\"status\":\"network_ready\"}")
	select {
	case <-ctx.Done():
	case <-network.done:
		network.Close()
		fmt.Fprintln(os.Stderr, "Guest network transport failed")
		os.Exit(1)
	}
}
