// Reads fixed routes and owns connection state.
import Foundation
// Accepts sockets from this owned guest.
import Virtualization
// Opens private Unix sockets without TCP.
import Darwin

struct GuestRoute: Decodable {
    let port: UInt32
    let path: String
}

/// Loads only private, owner-controlled fixed routes for this VM supervisor.
func guestRoutes(_ file: URL) throws -> [GuestRoute] {
    let info = try FileManager.default.attributesOfItem(atPath: file.path)
    guard info[.type] as? FileAttributeType == .typeRegular,
          (info[.ownerAccountID] as? NSNumber)?.uint32Value == getuid(),
          let mode = info[.posixPermissions] as? NSNumber, mode.intValue & 0o077 == 0 else {
        throw POSIXError(.EACCES)
    }
    let routes = try JSONDecoder().decode([GuestRoute].self, from: Data(contentsOf: file))
    guard routes.count <= 67, Set(routes.map(\.port)).count == routes.count,
          routes.allSatisfy({ ([10242, 10243, 10244].contains($0.port)
              || (10300...10363).contains($0.port)) && $0.path.hasPrefix("/")
              && $0.path.utf8.count < 104 }) else { throw POSIXError(.EINVAL) }
    return routes
}

/// Connects a fixed Unix endpoint with a deadline, without interpreting guest bytes.
private func unixPeer(_ path: String) -> Int32 {
    let fd = socket(AF_UNIX, SOCK_STREAM, 0)
    guard fd >= 0 else { return -1 }
    _ = fcntl(fd, F_SETFD, FD_CLOEXEC)
    _ = fcntl(fd, F_SETFL, O_NONBLOCK)
    var one: Int32 = 1
    _ = setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one, socklen_t(MemoryLayout<Int32>.size))
    var address = sockaddr_un()
    address.sun_len = UInt8(MemoryLayout<sockaddr_un>.size)
    address.sun_family = sa_family_t(AF_UNIX)
    withUnsafeMutableBytes(of: &address.sun_path) { target in
        target.copyBytes(from: Array(path.utf8) + [0])
    }
    let result = withUnsafePointer(to: &address) { pointer in
        pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
            connect(fd, $0, socklen_t(MemoryLayout<sockaddr_un>.size))
        }
    }
    if result < 0 {
        guard errno == EINPROGRESS else { close(fd); return -1 }
        var waiting = pollfd(fd: fd, events: Int16(POLLOUT), revents: 0)
        guard poll(&waiting, 1, 3000) > 0 else { close(fd); return -1 }
        var error: Int32 = 0
        var size = socklen_t(MemoryLayout<Int32>.size)
        guard getsockopt(fd, SOL_SOCKET, SO_ERROR, &error, &size) == 0, error == 0 else {
            close(fd); return -1
        }
    }
    return fd
}

@MainActor
final class GuestBridge: NSObject, @preconcurrency VZVirtioSocketListenerDelegate {
    private let route: GuestRoute
    private let listener = VZVirtioSocketListener()
    private var connections: [UUID: VZVirtioSocketConnection] = [:]
    private var peers: [UUID: Int32] = [:]
    private var stopped = false
    /// Called on the worker route's first connection, which needs a logged-in guest user.
    var onPaired: (() -> Void)?

    /// Stores a host-selected route that guest messages cannot replace.
    init(_ route: GuestRoute) { self.route = route }

    /// Admits this VM's connections on one fixed virtual port.
    func start(_ machine: VZVirtualMachine) {
        guard let device = machine.socketDevices.first as? VZVirtioSocketDevice else { return }
        listener.delegate = self
        device.setSocketListener(listener, forPort: route.port)
    }

    /// Bounds admission before dialing the route's private host endpoint.
    func listener(_ listener: VZVirtioSocketListener,
                  shouldAcceptNewConnection connection: VZVirtioSocketConnection,
                  from socketDevice: VZVirtioSocketDevice) -> Bool {
        guard !stopped, connections.count < 32 else { return false }
        let id = UUID()
        connections[id] = connection
        let path = route.path
        DispatchQueue.global(qos: .utility).async {
            let peer = unixPeer(path)
            // Strong in the outer closure, weak here, as before (Swift 6.4 ImplicitStrongCapture warning).
            Task { @MainActor [weak self = self] in
                guard let self else { if peer >= 0 { close(peer) }; return }
                self.paired(id, peer: peer)
            }
        }
        return true
    }

    /// Starts bounded forwarding only while the owning guest connection still exists.
    private func paired(_ id: UUID, peer: Int32) {
        guard !stopped, peer >= 0, let connection = connections[id] else {
            if peer >= 0 { close(peer) }
            connections.removeValue(forKey: id)?.close()
            return
        }
        peers[id] = peer
        if route.port == 10242 {
            onPaired?()
            onPaired = nil
        }
        let guest = connection.fileDescriptor
        let idle: Int32 = route.port == 10242 ? -1 : 120_000
        DispatchQueue.global(qos: .utility).async {
            forward(guest, peer, idleMs: idle)
            Task { @MainActor [weak self = self] in self?.finished(id) }
        }
    }

    /// Closes descriptors with registry removal so reused descriptor numbers stay safe.
    private func finished(_ id: UUID) {
        if let peer = peers.removeValue(forKey: id) { close(peer) }
        connections.removeValue(forKey: id)?.close()
    }

    /// Stops admission and wakes active pumps after confirmed guest shutdown.
    func stop() {
        stopped = true
        for peer in peers.values { _ = shutdown(peer, SHUT_RDWR) }
        for connection in connections.values { _ = shutdown(connection.fileDescriptor, SHUT_RDWR) }
    }
}
