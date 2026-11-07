package main

// Builds Ethernet fixtures through the protocol library.
import (
	"github.com/google/gopacket"
	"github.com/google/gopacket/layers"
)

// Supplies addresses and independent test expectations.
import (
	"net"
	"testing"
)

// Serializes a real Ethernet/IP packet for policy tests.
func fixture(t *testing.T, destination string, port uint16, udp bool) []byte {
	t.Helper()
	ether := &layers.Ethernet{SrcMAC: net.HardwareAddr{0x5a, 0x94, 0xef, 0xe4, 0x0c, 0xee},
		DstMAC: net.HardwareAddr{0x5a, 0x94, 0xef, 0xe4, 0x0c, 0x01}, EthernetType: layers.EthernetTypeIPv4}
	ip := &layers.IPv4{Version: 4, IHL: 5, TTL: 64, SrcIP: net.ParseIP("192.168.127.2"),
		DstIP: net.ParseIP(destination), Protocol: layers.IPProtocolTCP}
	var transport gopacket.SerializableLayer
	if udp {
		ip.Protocol = layers.IPProtocolUDP
		u := &layers.UDP{SrcPort: 45678, DstPort: layers.UDPPort(port)}
		if port == 67 {
			u.SrcPort = 68
		}
		if err := u.SetNetworkLayerForChecksum(ip); err != nil {
			t.Fatal(err)
		}
		transport = u
	} else {
		tcp := &layers.TCP{SrcPort: 45678, DstPort: layers.TCPPort(port), SYN: true, Window: 65535}
		if err := tcp.SetNetworkLayerForChecksum(ip); err != nil {
			t.Fatal(err)
		}
		transport = tcp
	}
	buffer := gopacket.NewSerializeBuffer()
	if err := gopacket.SerializeLayers(buffer, gopacket.SerializeOptions{FixLengths: true, ComputeChecksums: true},
		ether, ip, transport, gopacket.Payload([]byte("fixture"))); err != nil {
		t.Fatal(err)
	}
	return buffer.Bytes()
}

// Blocks host, LAN, metadata and gateway management before the network stack.
func TestDestinations(t *testing.T) {
	policy := newPolicy([]net.IP{net.ParseIP("8.8.4.4")})
	for _, ip := range []string{"127.0.0.1", "10.0.0.1", "192.168.1.1", "172.16.1.1",
		"169.254.169.254", "100.64.1.1", "0.0.0.0", "198.18.0.1", "224.0.0.1",
		"192.168.127.1", "192.168.127.2", "8.8.4.4"} {
		for _, udp := range []bool{false, true} {
			if policy.allows(fixture(t, ip, 8787, udp)) {
				t.Errorf("Allowed private destination %s", ip)
			}
		}
	}
	for _, port := range []uint16{80, 443, 5223} {
		for _, udp := range []bool{false, true} {
			if !policy.allows(fixture(t, "1.1.1.1", port, udp)) {
				t.Errorf("Denied public port %d", port)
			}
		}
	}
	for _, udp := range []bool{false, true} {
		if !policy.allows(fixture(t, "192.168.127.1", 53, udp)) {
			t.Error("DNS must work")
		}
	}
	if !policy.allows(fixture(t, "255.255.255.255", 67, true)) {
		t.Error("DHCP must work")
	}
}

// Malformed, tagged, fragmented and spoofed frames cannot bypass the destination check.
func TestMalformed(t *testing.T) {
	policy := newPolicy(nil)
	original := fixture(t, "1.1.1.1", 443, false)
	for i := 0; i < 54; i++ {
		if policy.allows(original[:i]) {
			t.Fatalf("Allowed truncated frame at %d", i)
		}
	}
	for _, change := range []struct {
		offset int
		value  byte
	}{
		{12, 0x81}, {12, 0x86}, {14, 0x46}, {20, 0x20}, {21, 0x01}, {26, 127}, {6, 0x00},
	} {
		frame := append([]byte(nil), original...)
		frame[change.offset] = change.value
		if policy.allows(frame) {
			t.Errorf("Allowed malformed frame at %d", change.offset)
		}
	}
}

// Return traffic can resolve the guest using a public peer's source IP.
func TestPeerARP(t *testing.T) {
	gateway := net.HardwareAddr{0x5a, 0x94, 0xef, 0xe4, 0x0c, 0x01}
	ether := &layers.Ethernet{SrcMAC: guestMAC, DstMAC: gateway, EthernetType: layers.EthernetTypeARP}
	arp := &layers.ARP{AddrType: layers.LinkTypeEthernet, Protocol: layers.EthernetTypeIPv4,
		HwAddressSize: 6, ProtAddressSize: 4, Operation: 2, SourceHwAddress: guestMAC,
		SourceProtAddress: guestIP.To4(), DstHwAddress: gateway, DstProtAddress: net.ParseIP("1.1.1.1").To4()}
	buffer := gopacket.NewSerializeBuffer()
	if err := gopacket.SerializeLayers(buffer, gopacket.SerializeOptions{FixLengths: true}, ether, arp); err != nil {
		t.Fatal(err)
	}
	if !newPolicy(nil).allows(buffer.Bytes()) {
		t.Fatal("Denied guest ARP reply to public peer")
	}
	arp.SourceProtAddress = net.ParseIP("127.0.0.1").To4()
	gopacket.SerializeLayers(buffer, gopacket.SerializeOptions{FixLengths: true}, ether, arp)
	if newPolicy(nil).allows(buffer.Bytes()) {
		t.Fatal("Allowed spoofed ARP ownership")
	}
}
