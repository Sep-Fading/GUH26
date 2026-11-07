// Creates signed synthetic requests and observations.
import Foundation
// Generates fixture tags without account credentials.
import CryptoKit

enum NativeFixtureError: Error { case failed }

@MainActor
final class FakeNativeAdapter: NativeScopeAdapter {
    var identity: NativeSession
    var calls: [String] = []
    var onCall: (() -> Void)?
    var fails = false
    var oversized = false
    var pauseFirst = false
    var waiter: CheckedContinuation<Void, Never>?

    /// Sets the identity reported by the fake native session.
    init(_ identity: NativeSession) { self.identity = identity }

    /// Returns fixture state without querying any desktop APIs.
    func session() throws -> NativeSession { identity }

    /// Records dispatch and returns only a synthetic observation.
    func perform(_ operation: NativeOperation, document: NativeDocument,
                 text: String?) async throws -> NativeObservation {
        calls.append(operation.rawValue + ":" + document.id)
        onCall?()
        if pauseFirst && calls.count == 1 {
            await withCheckedContinuation { waiter = $0 }
        }
        if fails { throw NativeFixtureError.failed }
        return NativeObservation(text: oversized ? String(repeating: "x", count: 70_000)
                                 : "synthetic-only", imagePNG: nil)
    }
}

@main
enum NativeScopeProbe {
    /// Runs one synthetic policy scenario, never constructing the native adapter.
    @MainActor
    static func main() async throws {
        let scenario = CommandLine.arguments[1]
        if scenario == "unchanged-edit" {
            let bytes = Data("saved".utf8)
            print("{\"allowed\":\(NativeSaveVerifier.canEdit(initial: bytes, expected: bytes))}")
            return
        }
        if scenario.hasPrefix("save-check-") {
            var reads = 0
            let verified = await NativeSaveVerifier.matches(expected: Data("saved".utf8), read: {
                reads += 1
                return Data(scenario == "save-check-delayed" && reads == 3 ? "saved".utf8 : "old".utf8)
            }, pause: {})
            print("{\"verified\":\(verified),\"reads\":\(reads)}")
            return
        }
        let generation = UUID(uuidString: "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")!
        let binding = NativeSession(uid: 1001, login: 101, generation: generation,
                                    process: 201, processStarted: 1, active: true)
        let adapter = FakeNativeAdapter(binding)
        let key = SymmetricKey(data: Data(repeating: 0x11, count: 32))
        let document = NativeDocument(id: "import-1",
            url: URL(fileURLWithPath: "/synthetic-only/import.txt"),
            writable: scenario != "read-only")
        let policy = NativeScopePolicy(binding: binding, revision: 1,
                                       documents: [document], key: key, adapter: adapter)
        var body: [String: Any] = ["sequence": 1, "generation": generation.uuidString,
            "revision": 1, "document": "import-1", "operation": "observe"]
        switch scenario {
        case "open", "save": body["operation"] = scenario
        case "replaceText", "read-only":
            body["operation"] = "replaceText"; body["text"] = "replacement"
        case "missing-text": body["operation"] = "replaceText"
        case "unexpected-text": body["text"] = "not permitted"
        case "large-request": body["text"] = String(repeating: "x", count: 70_000)
        case "extra-field": body["path"] = "/denied.txt"
        case "unknown-document": body["document"] = "denied"
        case "generic-operation": body["operation"] = "click"
        case "stale-generation": body["generation"] = UUID().uuidString
        case "stale-revision": body["revision"] = 0
        case "sequence-gap": body["sequence"] = 2
        case "inactive": adapter.identity.active = false
        case "wrong-session":
            adapter.identity = NativeSession(uid: 1002, login: 102, generation: generation,
                process: 201, processStarted: 1, active: true)
        case "process-reused":
            adapter.identity = NativeSession(uid: 1001, login: 101, generation: generation,
                process: 201, processStarted: 2, active: true)
        case "switch-during-call": adapter.onCall = { adapter.identity.active = false }
        case "revoke-during-call": adapter.onCall = { policy.revoke() }
        case "revoked": policy.revoke()
        case "adapter-failure": adapter.fails = true
        case "revoke-during-edit":
            body["operation"] = "replaceText"; body["text"] = "replacement"
            adapter.onCall = { policy.revoke() }
        case "adapter-failure-edit":
            body["operation"] = "replaceText"; body["text"] = "replacement"
            adapter.fails = true
        case "large-response-edit":
            body["operation"] = "replaceText"; body["text"] = "replacement"
            adapter.oversized = true
        case "large-response": adapter.oversized = true
        case "overlap": adapter.pauseFirst = true
        default: break
        }
        let payload = try JSONSerialization.data(withJSONObject: body, options: [.sortedKeys])
        let signedKey = scenario == "wrong-key"
            ? SymmetricKey(data: Data(repeating: 0x22, count: 32)) : key
        let authentication = Data(HMAC<SHA256>.authenticationCode(for: payload, using: signedKey))
        var replies: [NativeReply]
        if scenario == "overlap" {
            let first = Task { await policy.dispatch(payload: payload,
                                                     authentication: authentication) }
            while adapter.waiter == nil { await Task.yield() }
            body["sequence"] = 2
            let secondPayload = try JSONSerialization.data(withJSONObject: body)
            let secondTag = Data(HMAC<SHA256>.authenticationCode(for: secondPayload, using: key))
            let second = await policy.dispatch(payload: secondPayload, authentication: secondTag)
            adapter.waiter?.resume()
            adapter.waiter = nil
            replies = [await first.value, second]
        } else {
            replies = [await policy.dispatch(payload: payload, authentication: authentication)]
        }
        if scenario == "replay" {
            replies.append(await policy.dispatch(payload: payload, authentication: authentication))
        }
        var output: [String: Any] = ["statuses": replies.map(\.status),
            "calls": adapter.calls,
            "observations": replies.compactMap { $0.observation?.text }]
        if scenario.hasSuffix("-edit") { output["uncertain"] = replies[0].outcomeUnknown }
        let data = try JSONSerialization.data(withJSONObject: output, options: [.sortedKeys])
        print(String(decoding: data, as: UTF8.self))
    }
}
