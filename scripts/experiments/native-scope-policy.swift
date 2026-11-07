// Defines typed requests and bounded observations.
import Foundation
// Authenticates fixture envelopes without OAuth credentials.
import CryptoKit

enum NativeOperation: String, Decodable {
    case observe, open, replaceText, save
}

struct NativeSession: Equatable {
    let uid: UInt32
    let login: UInt32
    let generation: UUID
    let process: Int32
    let processStarted: Double
    var active: Bool
}

struct NativeDocument {
    let id: String
    let url: URL
    let writable: Bool
}

struct NativeRequest: Decodable {
    let sequence: UInt64
    let generation: UUID
    let revision: UInt64
    let document: String
    let operation: NativeOperation
    let text: String?
}

struct NativeObservation {
    var text: String?
    var imagePNG: Data?
}

struct NativeReply {
    let status: String
    var observation: NativeObservation?
    var outcomeUnknown = false
}

@MainActor
enum NativeSaveVerifier {
    /// Reports whether this run can demonstrate changed saved bytes.
    static func canEdit(initial: Data, expected: Data) -> Bool { initial != expected }

    /// Checks bytes at most five times without repeating the native mutation.
    static func matches(expected: Data, read: () -> Data?,
                        pause: () async -> Void) async -> Bool {
        for attempt in 0..<5 {
            if read() == expected { return true }
            if attempt < 4 { await pause() }
        }
        return false
    }
}

@MainActor
protocol NativeScopeAdapter: AnyObject {
    /// Returns the current worker/session and fixed TextEdit process identity.
    func session() throws -> NativeSession
    /// Performs one fixed operation on a trusted document binding.
    func perform(_ operation: NativeOperation, document: NativeDocument,
                 text: String?) async throws -> NativeObservation
}

@MainActor
final class NativeScopePolicy {
    private let binding: NativeSession
    private let revision: UInt64
    private let documents: [String: NativeDocument]
    private let key: SymmetricKey
    private let adapter: any NativeScopeAdapter
    private var enabled = true
    private var busy = false
    private var sequence: UInt64 = 0

    /// Binds one experiment policy to injected trusted setup and adapter state.
    init(binding: NativeSession, revision: UInt64,
         documents: [NativeDocument], key: SymmetricKey,
         adapter: any NativeScopeAdapter) {
        self.binding = binding
        self.revision = revision
        self.documents = Dictionary(uniqueKeysWithValues: documents.map { ($0.id, $0) })
        self.key = key
        self.adapter = adapter
    }

    /// Marks this generation unavailable for further calls.
    func revoke() { enabled = false }

    /// Checks one authenticated request before reserving its sequence and adapter call.
    func dispatch(payload: Data, authentication: Data) async -> NativeReply {
        guard payload.count <= 65_536 else { return NativeReply(status: "tooLarge") }
        guard authentication.count == 32,
              HMAC<SHA256>.isValidAuthenticationCode(authentication,
                  authenticating: payload, using: key) else {
            return NativeReply(status: "unauthenticated")
        }
        guard enabled else { return NativeReply(status: "revoked") }
        guard !busy else { return NativeReply(status: "busy") }
        let request: NativeRequest
        do {
            let fields = try JSONSerialization.jsonObject(with: payload) as? [String: Any]
            let allowed: Set<String> = ["sequence", "generation", "revision",
                                        "document", "operation", "text"]
            guard let fields, Set(fields.keys).isSubset(of: allowed) else {
                return NativeReply(status: "invalidRequest")
            }
            request = try JSONDecoder().decode(NativeRequest.self, from: payload)
        } catch {
            return NativeReply(status: "invalidRequest")
        }
        guard request.generation == binding.generation, request.revision == revision else {
            return NativeReply(status: "stale")
        }
        guard let document = documents[request.document],
              (request.operation == .replaceText) == (request.text != nil),
              (request.text?.utf8.count ?? 0) <= 32_768 else {
            return NativeReply(status: "invalidRequest")
        }
        if !document.writable && [.replaceText, .save].contains(request.operation) {
            return NativeReply(status: "readOnly")
        }
        guard sequence < UInt64.max, request.sequence == sequence + 1 else {
            return NativeReply(status: "replay")
        }
        guard current() else { return NativeReply(status: "stale") }
        busy = true
        sequence = request.sequence
        let mutating = request.operation != .observe
        defer { busy = false }
        do {
            let result = try await adapter.perform(request.operation, document: document,
                                                   text: request.text)
            guard enabled else { return NativeReply(status: "revoked", outcomeUnknown: mutating) }
            guard current() else { return NativeReply(status: "stale", outcomeUnknown: mutating) }
            guard (result.text?.utf8.count ?? 0) <= 65_536,
                  (result.imagePNG?.count ?? 0) <= 4_194_304 else {
                enabled = false
                return NativeReply(status: "tooLarge", outcomeUnknown: mutating)
            }
            return NativeReply(status: "ok", observation: result)
        } catch {
            enabled = false
            return NativeReply(status: "adapterFailed", outcomeUnknown: mutating)
        }
    }

    /// Invalidates this policy when its bound active session or process changes.
    private func current() -> Bool {
        guard let state = try? adapter.session(), state == binding, state.active else {
            enabled = false
            return false
        }
        return true
    }
}
