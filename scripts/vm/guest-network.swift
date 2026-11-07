// Resolves private socket paths and file ownership.
import Foundation
// Attaches Ethernet to the private network helper.
import Virtualization
// Creates connected Unix datagrams without host networking.
import Darwin

/// Returns a bounded Unix socket address without truncating its filesystem path.
private func networkAddress(_ path: String) throws -> sockaddr_un {
    var address = sockaddr_un()
    let bytes = Array(path.utf8)
    guard path.hasPrefix("/"), bytes.count < MemoryLayout.size(ofValue: address.sun_path) else {
        throw NSError(domain: "MiniClawNetwork", code: 1,
            userInfo: [NSLocalizedDescriptionKey: "Invalid network socket path"])
    }
    address.sun_family = sa_family_t(AF_UNIX)
    address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
    withUnsafeMutableBytes(of: &address.sun_path) { buffer in
        for (index, byte) in bytes.enumerated() { buffer[index] = byte }
    }
    return address
}

/// Connects one VM NIC to its private helper; no NAT, bridge or host adapter is added.
func attachNetwork(_ root: String, to configuration: VZVirtualMachineConfiguration) throws {
    let manager = FileManager.default
    let info = try manager.attributesOfItem(atPath: root)
    guard info[.type] as? FileAttributeType == .typeDirectory,
          (info[.posixPermissions] as? NSNumber)?.intValue == 0o700,
          (info[.ownerAccountID] as? NSNumber)?.uint32Value == getuid() else {
        throw NSError(domain: "MiniClawNetwork", code: 1,
            userInfo: [NSLocalizedDescriptionKey: "Network directory must be private and owned"])
    }
    let serverPath = root + "/network.sock"
    let guestPath = root + "/guest.sock"
    let server = try manager.attributesOfItem(atPath: serverPath)
    guard server[.type] as? FileAttributeType == .typeSocket,
          (server[.posixPermissions] as? NSNumber)?.intValue == 0o600,
          (server[.ownerAccountID] as? NSNumber)?.uint32Value == getuid(),
          !manager.fileExists(atPath: guestPath) else {
        throw NSError(domain: "MiniClawNetwork", code: 1,
            userInfo: [NSLocalizedDescriptionKey: "Invalid or already owned network socket"])
    }
    var local = try networkAddress(guestPath)
    var remote = try networkAddress(serverPath)
    let fd = socket(AF_UNIX, SOCK_DGRAM, 0)
    guard fd >= 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    let handle = FileHandle(fileDescriptor: fd, closeOnDealloc: true)
    var sendBuffer: Int32 = 1024 * 1024
    var receiveBuffer: Int32 = 4 * 1024 * 1024
    guard setsockopt(fd, SOL_SOCKET, SO_SNDBUF, &sendBuffer, 4) == 0,
          setsockopt(fd, SOL_SOCKET, SO_RCVBUF, &receiveBuffer, 4) == 0 else {
        throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
    }
    let bound = withUnsafePointer(to: &local) {
        $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            Darwin.bind(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
        }
    }
    guard bound == 0, chmod(guestPath, 0o600) == 0 else {
        throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
    }
    let connected = withUnsafePointer(to: &remote) {
        $0.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            Darwin.connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
        }
    }
    guard connected == 0 else { throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO) }
    let device = VZVirtioNetworkDeviceConfiguration()
    device.macAddress = VZMACAddress(string: "5a:94:ef:e4:0c:ee")!
    device.attachment = VZFileHandleNetworkDeviceAttachment(fileHandle: handle)
    configuration.networkDevices = [device]
}
