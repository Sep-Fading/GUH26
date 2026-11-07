// Retains socket data and dispatches IO.
import Foundation
// Connects only to the owned guest.
import Virtualization
// Binds loopback and forwards bounded streams.
import Darwin

/// Pumps at most 64 KiB per direction, retaining half-close semantics for HTTP/SSE.
func forward(_ client: Int32, _ guest: Int32, token: Data? = nil, idleMs: Int32 = 30_000,
             poller: (UnsafeMutablePointer<pollfd>, Int32) -> Int32 = { poll($0, 2, $1) }) {
    let descriptors = [client, guest]
    var buffers = [Data(), token.map { $0 + Data("\n".utf8) } ?? Data()]
    var reading = [true, true]
    for fd in descriptors { _ = fcntl(fd, F_SETFL, O_NONBLOCK) }
    while reading.contains(true) || buffers.contains(where: { !$0.isEmpty }) {
        var polls = descriptors.enumerated().map { index, fd in
            let events = (reading[index] && buffers[1 - index].isEmpty ? Int16(POLLIN) : 0)
                | (!buffers[index].isEmpty ? Int16(POLLOUT) : 0)
            return pollfd(fd: events == 0 ? -1 : fd, events: events, revents: 0)
        }
        let ready = poller(&polls, idleMs)
        if ready < 0 && errno == EINTR { continue }
        if ready <= 0 { return }
        for index in 0..<2 {
            let fd = descriptors[index]
            if polls[index].revents & Int16(POLLERR | POLLNVAL) != 0 { return }
            let hungUp = polls[index].revents & Int16(POLLHUP) != 0
            // HUP cannot become writable; queued bytes would otherwise spin forever.
            if hungUp && !buffers[index].isEmpty { return }
            if polls[index].revents & Int16(POLLOUT) != 0 {
                let sent = buffers[index].withUnsafeBytes { Darwin.write(fd, $0.baseAddress, $0.count) }
                if sent < 0 && (errno == EAGAIN || errno == EINTR) { continue }
                if sent <= 0 { return }
                buffers[index].removeFirst(sent)
                if buffers[index].isEmpty && !reading[1 - index] { _ = shutdown(fd, SHUT_WR) }
            }
            if reading[index] && buffers[1 - index].isEmpty
                && polls[index].revents & Int16(POLLIN | POLLHUP) != 0 {
                var chunk = [UInt8](repeating: 0, count: 65536)
                let count = read(fd, &chunk, chunk.count)
                if count < 0 && errno == EAGAIN && hungUp { return }
                if count < 0 && (errno == EAGAIN || errno == EINTR) { continue }
                if count < 0 { return }
                if count == 0 {
                    reading[index] = false
                    _ = shutdown(descriptors[1 - index], SHUT_WR)
                } else { buffers[1 - index].append(contentsOf: chunk.prefix(count)) }
            }
        }
    }
}

@MainActor
final class HostRelay {
    private var listener: Int32
    private var source: (any DispatchSourceRead)?
    private let token: Data
    private var clients = Set<Int32>()
    private var connections: [Int32: VZVirtioSocketConnection] = [:]

    /// Claims the selected loopback port and loads a private per-VM channel secret.
    init(port: UInt16, tokenFile: URL) throws {
        let info = try FileManager.default.attributesOfItem(atPath: tokenFile.path)
        guard info[.type] as? FileAttributeType == .typeRegular,
              let mode = info[.posixPermissions] as? NSNumber, mode.intValue & 0o077 == 0 else {
            throw NSError(domain: "MiniClawRelay", code: 1,
                          userInfo: [NSLocalizedDescriptionKey: "Channel token must be a private regular file"])
        }
        token = try Data(contentsOf: tokenFile)
        guard token.count == 64, token.allSatisfy({ (48...57).contains($0) || (97...102).contains($0) }) else {
            throw NSError(domain: "MiniClawRelay", code: 2)
        }
        listener = socket(AF_INET, SOCK_STREAM, 0)
        guard listener >= 0 else { throw POSIXError(.ENFILE) }
        var address = sockaddr_in()
        address.sin_len = UInt8(MemoryLayout<sockaddr_in>.size)
        address.sin_family = sa_family_t(AF_INET)
        address.sin_port = port.bigEndian
        address.sin_addr.s_addr = inet_addr("127.0.0.1")
        var reuse: Int32 = 1
        _ = setsockopt(listener, SOL_SOCKET, SO_REUSEADDR, &reuse, socklen_t(MemoryLayout<Int32>.size))
        let bound = withUnsafePointer(to: &address) { pointer in
            pointer.withMemoryRebound(to: sockaddr.self, capacity: 1) {
                Darwin.bind(listener, $0, socklen_t(MemoryLayout<sockaddr_in>.size))
            }
        }
        guard bound == 0, listen(listener, 32) == 0 else {
            let error = POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
            close(listener)
            throw error
        }
        _ = fcntl(listener, F_SETFL, O_NONBLOCK)
        _ = fcntl(listener, F_SETFD, FD_CLOEXEC)
    }

    /// Accepts bounded connections only after the selected guest has started.
    func start(_ machine: VZVirtualMachine) {
        guard let socket = machine.socketDevices.first as? VZVirtioSocketDevice else { return }
        let source = DispatchSource.makeReadSource(fileDescriptor: listener, queue: .main)
        source.setEventHandler { [weak self] in
            Task { @MainActor in self?.accept(socket) }
        }
        self.source = source
        source.resume()
    }

    /// Rejects excess clients and pairs each accepted stream with the fixed guest port.
    private func accept(_ socket: VZVirtioSocketDevice) {
        guard listener >= 0 else { return }
        while true {
            let client = Darwin.accept(listener, nil, nil)
            if client < 0 { return }
            if clients.count >= 32 { close(client); continue }
            clients.insert(client)
            _ = fcntl(client, F_SETFD, FD_CLOEXEC)
            socket.connect(toPort: 8787) { [weak self] result in
                guard let self else { close(client); return }
                guard self.listener >= 0, case .success(let connection) = result else {
                    close(client); self.clients.remove(client); return
                }
                self.connections[client] = connection
                let guest = connection.fileDescriptor
                let token = self.token
                DispatchQueue.global(qos: .utility).async {
                    forward(client, guest, token: token)
                    // The outer closure holds the relay strongly while forwarding, as before; the explicit
                    // assignment states that (Swift 6.4 ImplicitStrongCapture warning).
                    Task { @MainActor [weak self = self] in
                        // Release descriptors with their registry entry to prevent reuse races.
                        close(client)
                        self?.connections.removeValue(forKey: client)
                        self?.clients.remove(client)
                    }
                }
            }
        }
    }

    /// Closes admission and active streams after confirmed guest shutdown.
    func stop() {
        source?.cancel()
        source = nil
        if listener >= 0 { close(listener); listener = -1 }
        for client in clients { _ = shutdown(client, SHUT_RDWR) }
        for connection in connections.values { _ = shutdown(connection.fileDescriptor, SHUT_RDWR) }
    }
}
