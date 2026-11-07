package main

// Provides DHCP, DNS and established TCP/IP handling.
import (
	"github.com/containers/gvisor-tap-vsock/pkg/types"
	"github.com/containers/gvisor-tap-vsock/pkg/virtualnetwork"
)

// Owns private sockets and bounded packet framing.
import (
	"context"
	"encoding/binary"
	"errors"
	"io"
	"net"
	"os"
	"path/filepath"
	"sync"
	"syscall"
	"time"
)

type network struct {
	socket *net.UnixConn
	pipe   net.Conn
	peer   net.Conn
	once   sync.Once
	done   chan error
}

// Opens one private Ethernet channel without mounting management or forwarding APIs.
func startNetwork(ctx context.Context, root string) (*network, error) {
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	info, err := os.Lstat(root)
	if err != nil {
		return nil, err
	}
	owner, ok := info.Sys().(*syscall.Stat_t)
	if !ok || !info.IsDir() || info.Mode().Perm()&0077 != 0 || int(owner.Uid) != os.Getuid() {
		return nil, errors.New("Network directory must be private and owned")
	}
	path := filepath.Join(root, "network.sock")
	if _, err := os.Lstat(path); !os.IsNotExist(err) {
		return nil, errors.New("Network socket already exists")
	}
	local, err := hostAddresses()
	if err != nil {
		return nil, err
	}
	stack, err := virtualnetwork.New(&types.Configuration{
		MTU: 1500, Subnet: "192.168.127.0/24", GatewayIP: gatewayIP.String(),
		GatewayMacAddress: "5a:94:ef:e4:0c:01", Protocol: types.QemuProtocol,
		DHCPStaticLeases: map[string]string{guestIP.String(): guestMAC.String()},
	})
	if err != nil {
		return nil, err
	}
	socket, err := net.ListenUnixgram("unixgram", &net.UnixAddr{Name: path, Net: "unixgram"})
	if err != nil {
		return nil, err
	}
	// Match upstream VM transport buffers before the guest can send boot traffic.
	if err := socket.SetReadBuffer(4 * 1024 * 1024); err != nil {
		socket.Close()
		return nil, err
	}
	if err := socket.SetWriteBuffer(1024 * 1024); err != nil {
		socket.Close()
		return nil, err
	}
	if err := os.Chmod(path, 0600); err != nil {
		socket.Close()
		return nil, err
	}
	pipe, peer := net.Pipe()
	n := &network{socket: socket, pipe: pipe, peer: peer, done: make(chan error, 3)}
	client := &net.UnixAddr{Name: filepath.Join(root, "guest.sock"), Net: "unixgram"}
	go func() { n.done <- stack.AcceptQemu(ctx, peer) }()
	go func() { n.done <- n.receive(client, newPolicy(local)) }()
	go func() { n.done <- n.transmit(client) }()
	return n, nil
}

// Returns current host interface addresses for destination exclusion.
func hostAddresses() ([]net.IP, error) {
	addresses, err := net.InterfaceAddrs()
	if err != nil {
		return nil, err
	}
	result := make([]net.IP, 0, len(addresses))
	for _, address := range addresses {
		ip, _, err := net.ParseCIDR(address.String())
		if err != nil {
			return nil, err
		}
		result = append(result, ip)
	}
	return result, nil
}

// Admits bounded frames from the selected guest socket and refreshes host addresses.
func (n *network) receive(client *net.UnixAddr, policy policy) error {
	packet := make([]byte, 1515)
	var header [4]byte
	refreshed := time.Now()
	for {
		size, from, err := n.socket.ReadFromUnix(packet)
		if err != nil {
			return err
		}
		if from == nil || from.Name != client.Name {
			continue
		}
		if time.Since(refreshed) >= time.Second {
			local, err := hostAddresses()
			if err != nil {
				return err
			}
			policy.local = local
			refreshed = time.Now()
		}
		frame := packet[:size]
		allowed := policy.allows(frame)
		traceFrame("guest", frame, allowed)
		if !allowed {
			continue
		}
		binary.BigEndian.PutUint32(header[:], uint32(size))
		if _, err := n.pipe.Write(header[:]); err != nil {
			return err
		}
		if _, err := n.pipe.Write(frame); err != nil {
			return err
		}
	}
}

// Returns network-stack frames only to the selected private guest socket.
func (n *network) transmit(client *net.UnixAddr) error {
	var header [4]byte
	packet := make([]byte, 1514)
	for {
		if _, err := io.ReadFull(n.pipe, header[:]); err != nil {
			return err
		}
		size := binary.BigEndian.Uint32(header[:])
		if size < 14 || size > uint32(len(packet)) {
			return errors.New("Invalid outbound Ethernet frame")
		}
		if _, err := io.ReadFull(n.pipe, packet[:size]); err != nil {
			return err
		}
		traceFrame("stack", packet[:size], true)
		if err := n.sendFrame(packet[:size], client); err != nil {
			return err
		}
	}
}

// Drops frames under temporary buffer pressure instead of stalling the network stack.
func (n *network) sendFrame(packet []byte, client *net.UnixAddr) error {
	if err := n.socket.SetWriteDeadline(time.Now().Add(20 * time.Millisecond)); err != nil {
		return err
	}
	_, err := n.socket.WriteToUnix(packet, client)
	if errors.Is(err, syscall.ENOBUFS) || errors.Is(err, syscall.EAGAIN) || errors.Is(err, os.ErrDeadlineExceeded) {
		return nil
	}
	return err
}

// Closes the owned transport; the helper process owns the network stack's lifetime.
func (n *network) Close() {
	n.once.Do(func() { n.socket.Close(); n.pipe.Close(); n.peer.Close() })
}
