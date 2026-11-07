package main

// Decodes network headers without parsing application payloads.
import (
	"github.com/google/gopacket"
	"github.com/google/gopacket/layers"
)

// Compares identities and classifies destination addresses.
import (
	"bytes"
	"net"
)

var guestMAC = net.HardwareAddr{0x5a, 0x94, 0xef, 0xe4, 0x0c, 0xee}
var gatewayIP = net.ParseIP("192.168.127.1")
var guestIP = net.ParseIP("192.168.127.2")

type policy struct {
	denied []*net.IPNet
	local  []net.IP
}

// Builds a public-only destination policy with explicit local service exceptions.
func newPolicy(local []net.IP) policy {
	p := policy{local: local}
	for _, cidr := range []string{"0.0.0.0/8", "10.0.0.0/8", "100.64.0.0/10",
		"127.0.0.0/8", "169.254.0.0/16", "172.16.0.0/12", "192.168.0.0/16",
		"192.0.0.0/24", "192.0.2.0/24", "192.88.99.0/24", "198.18.0.0/15",
		"198.51.100.0/24", "203.0.113.0/24", "224.0.0.0/4", "240.0.0.0/4"} {
		_, subnet, err := net.ParseCIDR(cidr)
		if err != nil {
			panic(err)
		}
		p.denied = append(p.denied, subnet)
	}
	return p
}

// Accepts only routable IPv4 addresses that are not assigned to the host.
func (p policy) public(ip net.IP) bool {
	if ip.To4() == nil {
		return false
	}
	for _, subnet := range p.denied {
		if subnet.Contains(ip) {
			return false
		}
	}
	for _, host := range p.local {
		if host.Equal(ip) {
			return false
		}
	}
	return true
}

// Filters guest frames before any DNS service or outbound socket sees them.
func (p policy) allows(frame []byte) bool {
	if len(frame) < 14 || len(frame) > 1514 {
		return false
	}
	packet := gopacket.NewPacket(frame, layers.LayerTypeEthernet, gopacket.Default)
	ether, ok := packet.Layer(layers.LayerTypeEthernet).(*layers.Ethernet)
	if !ok || !bytes.Equal(ether.SrcMAC, guestMAC) {
		return false
	}
	if ether.EthernetType == layers.EthernetTypeARP {
		arp, ok := packet.Layer(layers.LayerTypeARP).(*layers.ARP)
		if !ok {
			return false
		}
		// gVisor may resolve the guest from a public peer's return address.
		peerReply := arp.Operation == 2 && net.IP(arp.SourceProtAddress).Equal(guestIP) &&
			bytes.Equal(arp.DstHwAddress, net.HardwareAddr{0x5a, 0x94, 0xef, 0xe4, 0x0c, 0x01}) &&
			p.public(net.IP(arp.DstProtAddress))
		return ok && arp.AddrType == layers.LinkTypeEthernet && arp.Protocol == layers.EthernetTypeIPv4 &&
			arp.HwAddressSize == 6 && arp.ProtAddressSize == 4 &&
			bytes.Equal(arp.SourceHwAddress, guestMAC) &&
			(net.IP(arp.SourceProtAddress).Equal(guestIP) || net.IP(arp.SourceProtAddress).Equal(net.IPv4zero)) &&
			(peerReply || net.IP(arp.DstProtAddress).Equal(gatewayIP) || net.IP(arp.DstProtAddress).Equal(guestIP))
	}
	if ether.EthernetType != layers.EthernetTypeIPv4 {
		return false
	}
	ip, ok := packet.Layer(layers.LayerTypeIPv4).(*layers.IPv4)
	if !ok || ip.Version != 4 || ip.IHL != 5 || ip.FragOffset != 0 ||
		ip.Flags&layers.IPv4MoreFragments != 0 || int(ip.Length) != 20+len(ip.Payload) {
		return false
	}
	source := ip.SrcIP.Equal(guestIP)
	switch ip.Protocol {
	case layers.IPProtocolUDP:
		udp, ok := packet.Layer(layers.LayerTypeUDP).(*layers.UDP)
		if !ok || udp.DstPort == 0 {
			return false
		}
		if (source || ip.SrcIP.Equal(net.IPv4zero)) && udp.SrcPort == 68 && udp.DstPort == 67 &&
			(ip.DstIP.Equal(net.IPv4bcast) || ip.DstIP.Equal(gatewayIP)) {
			return true
		}
		return source && ((ip.DstIP.Equal(gatewayIP) && udp.DstPort == 53) || p.public(ip.DstIP))
	case layers.IPProtocolTCP:
		tcp, ok := packet.Layer(layers.LayerTypeTCP).(*layers.TCP)
		return ok && source && tcp.DstPort != 0 &&
			((ip.DstIP.Equal(gatewayIP) && tcp.DstPort == 53) || p.public(ip.DstIP))
	}
	return false
}
