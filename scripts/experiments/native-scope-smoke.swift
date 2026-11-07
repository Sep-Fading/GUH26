// Parses trusted fixtures and records canaries.
import Foundation
// Checks the fixed app without launching.
import AppKit
// Checks existing permissions without prompting users.
import ApplicationServices
// Identifies the caller's actual login session.
import Security
// Signs manual harness calls without OAuth.
import CryptoKit
// Validates fixture ownership before native construction.
import Darwin

private struct SmokeFixture: Decodable {
    let version: Int
    let fixtureId: UUID
    let scopeUserUID: UInt32
    let ownerUID: UInt32
    let importsDirectory: String
}

private struct SmokeRefusal: Error {
    let message: String
}

@main
enum NativeScopeSmoke {
    /// Runs validation or explicitly opted-in native canaries; never requests permissions.
    @MainActor
    static func main() async {
        do {
            var args = Array(CommandLine.arguments.dropFirst())
            let enabled = args.first == "--enable-native-smoke"
            if enabled { args.removeFirst() }
            guard args.count == 2, ["validate", "observe", "edit", "readonly", "switch"].contains(args[0]) else {
                throw SmokeRefusal(message: "Usage: [--enable-native-smoke] validate|observe|edit|readonly|switch MANIFEST")
            }
            guard args[0] == "validate" || enabled else {
                throw SmokeRefusal(message: "Native smoke is disabled; explicit setup approval and --enable-native-smoke are required")
            }
            let fixture = try readFixture(URL(fileURLWithPath: args[1]))
            let identities = try validateFiles(fixture)
            if args[0] == "validate" {
                print("Fixture checks passed. No native API or permission preflight was invoked.")
                return
            }
            guard #available(macOS 14.0, *) else {
                throw SmokeRefusal(message: "Native smoke requires macOS 14 or newer")
            }
            try await native(args[0], fixture: fixture, identities: identities)
        } catch let refusal as SmokeRefusal {
            FileHandle.standardError.write(Data((refusal.message + "\n").utf8))
            exit(2)
        } catch {
            FileHandle.standardError.write(Data("Native smoke refused or failed; no automatic retry\n".utf8))
            exit(2)
        }
    }

    /// Reads a bounded, regular fixture manifest and rejects the trusted owner's session.
    private static func readFixture(_ url: URL) throws -> SmokeFixture {
        let fd = open(url.path, O_RDONLY | O_NOFOLLOW | O_CLOEXEC)
        guard fd >= 0 else { throw SmokeRefusal(message: "Invalid fixture manifest") }
        let handle = FileHandle(fileDescriptor: fd, closeOnDealloc: true)
        var metadata = stat()
        guard fstat(fd, &metadata) == 0, metadata.st_mode & S_IFMT == S_IFREG,
              metadata.st_nlink == 1, metadata.st_size <= 4096,
              metadata.st_mode & 0o022 == 0,
              let bytes = try handle.read(upToCount: 4097), bytes.count <= 4096,
              let object = try JSONSerialization.jsonObject(with: bytes) as? [String: Any],
              Set(object.keys) == Set(["version", "fixtureId", "scopeUserUID", "ownerUID", "importsDirectory"]) else {
            throw SmokeRefusal(message: "Invalid fixture manifest")
        }
        let fixture = try JSONDecoder().decode(SmokeFixture.self, from: bytes)
        guard fixture.version == 1, fixture.ownerUID == metadata.st_uid else {
            throw SmokeRefusal(message: "Invalid fixture manifest")
        }
        guard getuid() == fixture.scopeUserUID, geteuid() == fixture.scopeUserUID,
              fixture.scopeUserUID != fixture.ownerUID, fixture.scopeUserUID != 0 else {
            throw SmokeRefusal(message: "Run as the disposable user, never the fixture owner")
        }
        let root = URL(fileURLWithPath: fixture.importsDirectory).standardizedFileURL
        var directory = stat()
        guard root.path == fixture.importsDirectory, root.resolvingSymlinksInPath() == root,
              url.standardizedFileURL.deletingLastPathComponent() == root,
              lstat(root.path, &directory) == 0, directory.st_mode & S_IFMT == S_IFDIR,
              directory.st_uid == fixture.ownerUID, directory.st_mode & 0o022 == 0 else {
            throw SmokeRefusal(message: "Invalid fixture directory")
        }
        return fixture
    }

    /// Checks only synthetic plain-text files and their OS read/write permissions.
    private static func validateFiles(_ fixture: SmokeFixture) throws -> [String: NativeFileIdentity] {
        var identities: [String: NativeFileIdentity] = [:]
        for name in ["allowed", "readonly", "denied"] {
            let url = URL(fileURLWithPath: fixture.importsDirectory).appendingPathComponent(name + ".txt")
            var metadata = stat()
            guard lstat(url.path, &metadata) == 0, metadata.st_mode & S_IFMT == S_IFREG,
                  metadata.st_nlink == 1, metadata.st_uid == fixture.ownerUID,
                  metadata.st_size > 0, metadata.st_size <= 1024 else {
                throw SmokeRefusal(message: "Invalid synthetic canary files")
            }
            identities[name] = NativeFileIdentity(device: metadata.st_dev,
                inode: metadata.st_ino, owner: metadata.st_uid)
            if name == "denied" {
                guard access(url.path, R_OK) != 0, access(url.path, W_OK) != 0 else {
                    throw SmokeRefusal(message: "Denied canary is accessible; native calls refused")
                }
            } else {
                let initial = "MINICLAW_\(name.uppercased()) \(fixture.fixtureId.uuidString)\n"
                let edited = "MINICLAW_EDITED \(fixture.fixtureId.uuidString)\n"
                let data = try Data(contentsOf: url)
                guard data == Data(initial.utf8) || (name == "allowed" && data == Data(edited.utf8)),
                      (access(url.path, W_OK) == 0) == (name == "allowed") else {
                    throw SmokeRefusal(message: "Canary bytes or permissions differ from the fixture contract")
                }
            }
        }
        return identities
    }

    /// Performs a fixed manual native check after setup and permission gates succeed.
    @available(macOS 14.0, *)
    @MainActor
    private static func native(_ mode: String, fixture: SmokeFixture,
                               identities: [String: NativeFileIdentity]) async throws {
        let allowed = URL(fileURLWithPath: fixture.importsDirectory).appendingPathComponent("allowed.txt")
        let initial = try Data(contentsOf: allowed)
        let expected = Data("MINICLAW_EDITED \(fixture.fixtureId.uuidString)\n".utf8)
        guard mode != "edit" || NativeSaveVerifier.canEdit(initial: initial, expected: expected) else {
            throw SmokeRefusal(message: "Edit needs a fresh canary; target bytes are already present")
        }
        let initialHash = SHA256.hash(data: initial).map { String(format: "%02x", $0) }.joined()
        guard AXIsProcessTrusted(), CGPreflightScreenCaptureAccess() else {
            throw SmokeRefusal(message: "Required helper permissions are absent; no permission prompt was requested")
        }
        let apps = NSRunningApplication.runningApplications(withBundleIdentifier: "com.apple.TextEdit")
        guard apps.count == 1, let app = apps.first, let started = app.launchDate else {
            throw SmokeRefusal(message: "Open only the disposable TextEdit fixture before running this check")
        }
        var login: SecuritySessionId = 0
        var attributes: SessionAttributeBits = []
        guard SessionGetInfo(callerSecuritySession, &login, &attributes) == errSecSuccess else {
            throw SmokeRefusal(message: "No valid GUI login session")
        }
        let binding = NativeSession(uid: getuid(), login: login, generation: UUID(),
            process: app.processIdentifier, processStarted: started.timeIntervalSince1970, active: true)
        let root = URL(fileURLWithPath: fixture.importsDirectory)
        let documents = [NativeDocument(id: "allowed", url: root.appendingPathComponent("allowed.txt"), writable: true),
            NativeDocument(id: "readonly", url: root.appendingPathComponent("readonly.txt"), writable: false)]
        let adapter = TextEditScopeAdapter(binding: binding, imports: root,
            importOwner: fixture.ownerUID, identities: identities)
        let key = SymmetricKey(size: .bits256)
        let policy = NativeScopePolicy(binding: binding, revision: 1,
            documents: documents, key: key, adapter: adapter)
        let output = URL(fileURLWithPath: NSTemporaryDirectory())
            .appendingPathComponent("miniclaw-native-smoke-" + UUID().uuidString)
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: false,
                                               attributes: [.posixPermissions: 0o700])
        let operations: [NativeOperation] = mode == "edit" ? [.open, .replaceText, .save, .observe]
            : mode == "readonly" ? [.open, .replaceText] : [.observe]
        var sequence: UInt64 = 0
        let count = mode == "switch" ? 5 : 1
        for _ in 0..<count {
            for operation in operations {
                sequence += 1
                var body: [String: Any] = ["sequence": sequence,
                    "generation": binding.generation.uuidString, "revision": 1,
                    "document": mode == "readonly" ? "readonly" : "allowed",
                    "operation": operation.rawValue]
                if operation == .replaceText {
                    body["text"] = "MINICLAW_EDITED \(fixture.fixtureId.uuidString)\n"
                }
                let payload = try JSONSerialization.data(withJSONObject: body, options: [.sortedKeys])
                let tag = Data(HMAC<SHA256>.authenticationCode(for: payload, using: key))
                let reply = await policy.dispatch(payload: payload, authentication: tag)
                var report: [String: Any] = ["operation": operation.rawValue,
                    "status": reply.status, "outcomeUnknown": reply.outcomeUnknown,
                    "initialSHA256": initialHash]
                if let png = reply.observation?.imagePNG {
                    let path = output.appendingPathComponent("observation-\(sequence).png")
                    try png.write(to: path, options: [.withoutOverwriting])
                    report["image"] = path.path
                }
                var saveConfirmed = true
                if operation == .save {
                    saveConfirmed = await NativeSaveVerifier.matches(
                        expected: Data("MINICLAW_EDITED \(fixture.fixtureId.uuidString)\n".utf8),
                        read: {
                            guard let handle = try? FileHandle(forReadingFrom: documents[0].url) else {
                                return nil
                            }
                            defer { try? handle.close() }
                            return try? handle.read(upToCount: 1025)
                        }, pause: { try? await Task.sleep(for: .milliseconds(100)) })
                    report["savedBytesMatch"] = saveConfirmed
                    if !saveConfirmed {
                        report["status"] = "saveUnconfirmed"
                        report["outcomeUnknown"] = true
                    }
                }
                let encoded = try JSONSerialization.data(withJSONObject: report, options: [.sortedKeys])
                print(String(decoding: encoded, as: UTF8.self))
                fflush(stdout)
                if mode == "readonly", operation == .replaceText, reply.status == "readOnly" {
                    _ = try validateFiles(fixture)
                } else if reply.status != "ok" || !saveConfirmed {
                    throw SmokeRefusal(message: "Native check did not pass; inspect its result before retrying")
                }
            }
            if mode == "switch" { try await Task.sleep(for: .seconds(2)) }
        }
    }
}
