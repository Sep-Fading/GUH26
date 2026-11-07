package main

// Serializes real DNS requests and ARP replies.
import (
	"github.com/google/gopacket"
	"github.com/google/gopacket/layers"
)

// Uses private sockets and bounded test deadlines.
import (
	"context"
	"net"
	"os"
	"path/filepath"
	"syscall"
	"testing"
	"time"
)

// Burst traffic needs the same socket capacity as the upstream VM transport.
func TestSocketCapacity(t *testing.T) {
	root, err := os.MkdirTemp("/private/tmp", "mc-capacity-")
	if err != nil {
		t.Fatal(err)
	}
	defer os.RemoveAll(root)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	n, err := startNetwork(ctx, root)
	if err != nil {
		t.Fatal(err)
	}
	defer n.Close()
	raw, err := n.socket.SyscallConn()
	if err != nil {
		t.Fatal(err)
	}
	var size int
	raw.Control(func(fd uintptr) { size, err = syscall.GetsockoptInt(int(fd), syscall.SOL_SOCKET, syscall.SO_RCVBUF) })
	if err != nil {
		t.Fatal(err)
	}
	if size < 4*1024*1024 {
		t.Fatalf("Receive buffer too small: %d", size)
	}
}

// A permitted DNS request must receive a reply through the complete Ethernet channel.
func TestDNSReply(t *testing.T) {
	root, err := os.MkdirTemp("/private/tmp", "mc-dns-")
	if err != nil {
		t.Fatal(err)
	}
	defer os.RemoveAll(root)
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	gateway, err := startNetwork(ctx, root)
	if err != nil {
		t.Fatal(err)
	}
	defer gateway.Close()
	client, err := net.ListenUnixgram("unixgram", &net.UnixAddr{Name: filepath.Join(root, "guest.sock"), Net: "unixgram"})
	if err != nil {
		t.Fatal(err)
	}
	defer client.Close()
	client.SetReadDeadline(time.Now().Add(5 * time.Second))
	target := &net.UnixAddr{Name: filepath.Join(root, "network.sock"), Net: "unixgram"}
	ether := &layers.Ethernet{SrcMAC: guestMAC, DstMAC: net.HardwareAddr{0x5a, 0x94, 0xef, 0xe4, 0x0c, 0x01}, EthernetType: layers.EthernetTypeIPv4}
	ip := &layers.IPv4{Version: 4, IHL: 5, TTL: 64, SrcIP: guestIP.To4(), DstIP: gatewayIP.To4(), Protocol: layers.IPProtocolUDP}
	udp := &layers.UDP{SrcPort: 45678, DstPort: 53}
	udp.SetNetworkLayerForChecksum(ip)
	name := "localhost"
	if os.Getenv("MINICLAW_NETWORK_LIVE") == "1" {
		name = "example.com"
	}
	dns := &layers.DNS{ID: 1234, RD: true, Questions: []layers.DNSQuestion{{Name: []byte(name), Type: layers.DNSTypeA, Class: layers.DNSClassIN}}}
	buffer := gopacket.NewSerializeBuffer()
	options := gopacket.SerializeOptions{FixLengths: true, ComputeChecksums: true}
	if err := gopacket.SerializeLayers(buffer, options, ether, ip, udp, dns); err != nil {
		t.Fatal(err)
	}
	if _, err := client.WriteToUnix(buffer.Bytes(), target); err != nil {
		t.Fatal(err)
	}
	packet := make([]byte, 1514)
	for {
		size, _, err := client.ReadFromUnix(packet)
		if err != nil {
			t.Fatal("No DNS reply:", err)
		}
		decoded := gopacket.NewPacket(packet[:size], layers.LayerTypeEthernet, gopacket.Default)
		if arp, ok := decoded.Layer(layers.LayerTypeARP).(*layers.ARP); ok {
			reply := &layers.ARP{AddrType: layers.LinkTypeEthernet, Protocol: layers.EthernetTypeIPv4, HwAddressSize: 6, ProtAddressSize: 4, Operation: 2,
				SourceHwAddress: guestMAC, SourceProtAddress: guestIP.To4(), DstHwAddress: arp.SourceHwAddress, DstProtAddress: arp.SourceProtAddress}
			ether.EthernetType = layers.EthernetTypeARP
			if err := gopacket.SerializeLayers(buffer, options, ether, reply); err != nil {
				t.Fatal(err)
			}
			if _, err := client.WriteToUnix(buffer.Bytes(), target); err != nil {
				t.Fatal(err)
			}
			continue
		}
		if reply, ok := decoded.Layer(layers.LayerTypeDNS).(*layers.DNS); ok && reply.ID == 1234 && reply.QR {
			if len(reply.Answers) == 0 {
				t.Fatal("DNS reply had no address")
			}
			return
		}
	}
}

// A guest that stops reading must not stall the network stack indefinitely.
func TestSlowReceiver(t *testing.T) {
	root, err := os.MkdirTemp("/private/tmp", "mc-slow-")
	if err != nil {
		t.Fatal(err)
	}
	defer os.RemoveAll(root)
	client, err := net.ListenUnixgram("unixgram", &net.UnixAddr{Name: filepath.Join(root, "guest.sock"), Net: "unixgram"})
	if err != nil {
		t.Fatal(err)
	}
	defer client.Close()
	socket, err := net.ListenUnixgram("unixgram", &net.UnixAddr{Name: filepath.Join(root, "network.sock"), Net: "unixgram"})
	if err != nil {
		t.Fatal(err)
	}
	defer socket.Close()
	n := &network{socket: socket}
	done := make(chan error, 1)
	go func() {
		for i := 0; i < 64; i++ {
			if err := n.sendFrame(make([]byte, 1500), client.LocalAddr().(*net.UnixAddr)); err != nil {
				done <- err
				return
			}
		}
		done <- nil
	}()
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("Non-reading guest stalled the transport")
	}
}

// A denied frame must never reach a real host listener through the full stack.
func TestPrivateCanary(t *testing.T) {
	root, err := os.MkdirTemp("/private/tmp", "mc-net-")
	if err != nil {
		t.Fatal(err)
	}
	defer os.RemoveAll(root)
	listener, err := net.Listen("tcp4", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer listener.Close()
	listener.(*net.TCPListener).SetDeadline(time.Now().Add(time.Second))
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	gateway, err := startNetwork(ctx, root)
	if err != nil {
		t.Fatal(err)
	}
	defer gateway.Close()
	client, err := net.ListenUnixgram("unixgram", &net.UnixAddr{Name: filepath.Join(root, "guest.sock"), Net: "unixgram"})
	if err != nil {
		t.Fatal(err)
	}
	defer client.Close()
	port := uint16(listener.Addr().(*net.TCPAddr).Port)
	target := &net.UnixAddr{Name: filepath.Join(root, "network.sock"), Net: "unixgram"}
	for i := 0; i < 3; i++ {
		if _, err := client.WriteToUnix(fixture(t, "127.0.0.1", port, false), target); err != nil {
			t.Fatal(err)
		}
	}
	connection, err := listener.Accept()
	if err == nil {
		connection.Close()
		t.Fatal("Guest reached the host canary")
	}
	if timeout, ok := err.(net.Error); !ok || !timeout.Timeout() {
		t.Fatal(err)
	}
}

// Existing sockets and non-private directories are not reused or unlinked.
func TestPrivateOwnership(t *testing.T) {
	root, err := os.MkdirTemp("/private/tmp", "mc-net-")
	if err != nil {
		t.Fatal(err)
	}
	defer os.RemoveAll(root)
	path := filepath.Join(root, "network.sock")
	if err := os.WriteFile(path, []byte("keep"), 0600); err != nil {
		t.Fatal(err)
	}
	if gateway, err := startNetwork(context.Background(), root); err == nil {
		gateway.Close()
		t.Fatal("Overwrote existing socket path")
	}
	value, _ := os.ReadFile(path)
	if string(value) != "keep" {
		t.Fatal("Existing file changed")
	}
	os.Remove(path)
	os.Chmod(root, 0755)
	if gateway, err := startNetwork(context.Background(), root); err == nil {
		gateway.Close()
		t.Fatal("Accepted public socket directory")
	}
}
