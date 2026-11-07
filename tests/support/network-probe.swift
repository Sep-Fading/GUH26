// Creates disposable socket paths and checks metadata.
import Foundation
// Inspects the configured native network device.
import Virtualization
// Owns Unix datagram sockets without privileges.
import Darwin

@main
enum NetworkProbe {
    /// Verifies private network attachment without booting a VM.
    static func main() throws {
        let root = URL(fileURLWithPath: "/private/tmp/mc-network-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false,
            attributes: [.posixPermissions: 0o700])
        defer { try? FileManager.default.removeItem(at: root) }
        let config = VZVirtualMachineConfiguration()
        do {
            try attachNetwork(root.path, to: config)
            fatalError("Accepted a missing network helper")
        } catch { }
        let server = socket(AF_UNIX, SOCK_DGRAM, 0)
        guard server >= 0 else { fatalError("Socket failed") }
        defer { close(server) }
        var address = sockaddr_un()
        address.sun_family = sa_family_t(AF_UNIX)
        let path = root.appendingPathComponent("network.sock").path
        withUnsafeMutableBytes(of: &address.sun_path) { buffer in
            for (index, byte) in path.utf8.enumerated() { buffer[index] = byte }
        }
        let bound = withUnsafePointer(to: &address) {
            $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                bind(server, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
            }
        }
        guard bound == 0 else { fatalError("Bind failed") }
        _ = chmod(path, 0o600)
        try attachNetwork(root.path, to: config)
        guard config.networkDevices.count == 1,
              let device = config.networkDevices.first as? VZVirtioNetworkDeviceConfiguration,
              device.attachment is VZFileHandleNetworkDeviceAttachment,
              device.macAddress.string.lowercased() == "5a:94:ef:e4:0c:ee" else {
            fatalError("Wrong network attachment")
        }
        print("private network checks passed")
    }
}
