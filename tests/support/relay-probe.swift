// Exercises bounded streams without starting guests.
import Foundation
// Creates local socket pairs and half-closes.
import Darwin

@main
struct RelayProbe {
    /// Checks prefix framing, streamed replies, half-close and descriptor ownership.
    static func main() {
        stalledHangup(token: Data([97]))
        stalledHangup(token: nil)
        var left: [Int32] = [-1, -1]
        var right: [Int32] = [-1, -1]
        precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &left) == 0)
        precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &right) == 0)
        let client = left[0], relayClient = left[1], relayGuest = right[0], server = right[1]
        defer { for fd in left + right { close(fd) } }
        let finished = DispatchSemaphore(value: 0)
        DispatchQueue.global().async {
            forward(relayClient, relayGuest, token: Data(repeating: 97, count: 64))
            finished.signal()
        }
        precondition(write(client, "request", 7) == 7)
        shutdown(client, SHUT_WR)
        var request = Data()
        var buffer = [UInt8](repeating: 0, count: 1024)
        while true {
            let size = read(server, &buffer, buffer.count)
            precondition(size >= 0)
            if size == 0 { break }
            request.append(contentsOf: buffer.prefix(size))
        }
        precondition(request == Data((String(repeating: "a", count: 64) + "\nrequest").utf8))
        precondition(write(server, "data: streamed\n\n", 16) == 16)
        shutdown(server, SHUT_WR)
        var response = Data()
        while response.count < 16 {
            let size = read(client, &buffer, buffer.count)
            precondition(size > 0)
            response.append(contentsOf: buffer.prefix(size))
        }
        precondition(response == Data("data: streamed\n\n".utf8))
        precondition(finished.wait(timeout: .now() + 2) == .success)
        precondition(fcntl(relayClient, F_GETFD) >= 0, "The owner must close its descriptor")
        print("Relay stream and ownership checks passed")
    }

    /// Reproduces a closed virtual endpoint reporting HUP while its read would block.
    static func stalledHangup(token: Data?) {
        var pair: [Int32] = [-1, -1]
        precondition(socketpair(AF_UNIX, SOCK_STREAM, 0, &pair) == 0)
        defer { for fd in pair { close(fd) } }
        let client = pair[0], guest = pair[1]
        let finished = DispatchSemaphore(value: 0)
        DispatchQueue.global().async {
            forward(client, guest, token: token, poller: { descriptors, _ in
                descriptors[0].revents = 0
                descriptors[1].revents = Int16(POLLHUP)
                return 1
            })
            finished.signal()
        }
        precondition(finished.wait(timeout: .now() + 1) == .success,
                     "A hung-up endpoint must not spin forever")
    }
}
