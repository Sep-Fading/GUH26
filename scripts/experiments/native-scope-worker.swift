// Locates the fixed native TextEdit process.
import AppKit
// Reads and changes bound accessibility elements.
import ApplicationServices
// Captures only the verified native window.
import ScreenCaptureKit
// Reads the worker's actual login session.
import Security
// Checks process ownership and imported files.
import Darwin

enum NativeWorkerFailure: Error { case unavailable, identity, document, unsupported }

struct NativeFileIdentity {
    let device: Int32
    let inode: UInt64
    let owner: UInt32
}

// Owns immutable notification registration across deinitialization.
private final class NativeSessionObserver: @unchecked Sendable {
    private let center: NotificationCenter
    private let token: NSObjectProtocol

    /// Registers one session-switch callback on the main queue.
    init(center: NotificationCenter, callback: @escaping @Sendable (Notification) -> Void) {
        self.center = center
        token = center.addObserver(forName: NSWorkspace.sessionDidResignActiveNotification,
                                   object: nil, queue: .main, using: callback)
    }

    /// Removes this object's session observer when its owner releases it.
    deinit { center.removeObserver(token) }
}

@available(macOS 14.0, *)
@MainActor
final class TextEditScopeAdapter: NativeScopeAdapter {
    private let binding: NativeSession
    private let imports: URL
    private let importOwner: UInt32
    private let identities: [String: NativeFileIdentity]
    private let approvedLocal: Bool
    private var invalidated = false
    private var observer: NativeSessionObserver?
    private let applicationURL = URL(fileURLWithPath: "/System/Applications/TextEdit.app")

    /// Stores trusted fixture bindings; this adapter has no runnable entry point.
    init(binding: NativeSession, imports: URL, importOwner: UInt32,
         identities: [String: NativeFileIdentity], approvedLocal: Bool = false) {
        self.binding = binding
        self.imports = imports
        self.importOwner = importOwner
        self.identities = identities
        self.approvedLocal = approvedLocal
        observer = NativeSessionObserver(center: NSWorkspace.shared.notificationCenter) {
            [weak self] _ in
            MainActor.assumeIsolated { self?.invalidated = true }
        }
    }

    /// Verifies the actual worker user, login session and original TextEdit process.
    func session() throws -> NativeSession {
        guard !invalidated, getuid() == binding.uid, geteuid() == binding.uid,
              binding.uid != 0 else { throw NativeWorkerFailure.identity }
        var login: SecuritySessionId = 0
        var flags: SessionAttributeBits = []
        guard SessionGetInfo(callerSecuritySession, &login, &flags) == errSecSuccess,
              let current = CGSessionCopyCurrentDictionary() as? [String: Any],
              let app = NSRunningApplication(processIdentifier: binding.process),
              !app.isTerminated, app.bundleIdentifier == "com.apple.TextEdit",
              app.bundleURL?.standardizedFileURL == applicationURL,
              let started = app.launchDate?.timeIntervalSince1970 else {
            throw NativeWorkerFailure.identity
        }
        var metadata = proc_bsdinfo()
        guard proc_pidinfo(binding.process, PROC_PIDTBSDINFO, 0, &metadata,
                           Int32(MemoryLayout<proc_bsdinfo>.size))
                == MemoryLayout<proc_bsdinfo>.size,
              metadata.pbi_uid == binding.uid else { throw NativeWorkerFailure.identity }
        return NativeSession(uid: getuid(), login: login, generation: binding.generation,
            process: binding.process, processStarted: started,
            active: current[kCGSessionOnConsoleKey as String] as? Bool == true)
    }

    /// Executes only the four fixed TextEdit operations after local identity checks.
    func perform(_ operation: NativeOperation, document: NativeDocument,
                 text: String?) async throws -> NativeObservation {
        try validate(document)
        guard AXIsProcessTrusted() else { throw NativeWorkerFailure.unavailable }
        let app = AXUIElementCreateApplication(binding.process)
        if operation == .open {
            let configuration = NSWorkspace.OpenConfiguration()
            configuration.activates = false
            configuration.promptsUserIfNeeded = false
            configuration.createsNewApplicationInstance = false
            configuration.allowsRunningApplicationSubstitution = false
            let opened = try await NSWorkspace.shared.open([document.url],
                withApplicationAt: applicationURL, configuration: configuration)
            guard opened.processIdentifier == binding.process else {
                throw NativeWorkerFailure.identity
            }
            try validate(document)
            _ = try documentWindow(app, document: document)
            return NativeObservation()
        }
        let window = try documentWindow(app, document: document)
        switch operation {
        case .observe:
            return try await capture(document, window: window)
        case .replaceText:
            guard document.writable, let text, text.utf8.count <= 32_768 else {
                throw NativeWorkerFailure.document
            }
            let editors = try descendants(window).filter {
                (try? attribute($0, kAXRoleAttribute) as? String) == kAXTextAreaRole
            }
            guard editors.count == 1 else { throw NativeWorkerFailure.unsupported }
            var settable = DarwinBoolean(false)
            guard AXUIElementIsAttributeSettable(editors[0], kAXValueAttribute as CFString,
                                                &settable) == .success, settable.boolValue else {
                throw NativeWorkerFailure.unsupported
            }
            try validate(document)
            guard CFEqual(try documentWindow(app, document: document), window) else {
                throw NativeWorkerFailure.document
            }
            guard AXUIElementSetAttributeValue(editors[0], kAXValueAttribute as CFString,
                                               text as CFString) == .success else {
                throw NativeWorkerFailure.unsupported
            }
        case .save:
            guard document.writable else { throw NativeWorkerFailure.document }
            let bar = try attribute(app, kAXMenuBarAttribute)
            guard CFGetTypeID(bar) == AXUIElementGetTypeID() else {
                throw NativeWorkerFailure.unsupported
            }
            let items = try descendants(unsafeDowncast(bar, to: AXUIElement.self)).filter {
                (try? attribute($0, kAXMenuItemCmdCharAttribute) as? String)?.lowercased() == "s"
                    && (try? attribute($0, kAXMenuItemCmdModifiersAttribute) as? NSNumber)?.intValue == 0
            }
            guard items.count == 1 else { throw NativeWorkerFailure.unsupported }
            try validate(document)
            let main = try attribute(app, kAXMainWindowAttribute)
            guard CFGetTypeID(main) == AXUIElementGetTypeID(), CFEqual(main, window),
                  CFEqual(try documentWindow(app, document: document), window) else {
                throw NativeWorkerFailure.document
            }
            guard AXUIElementPerformAction(items[0], kAXPressAction as CFString) == .success else {
                throw NativeWorkerFailure.unsupported
            }
        case .open:
            throw NativeWorkerFailure.unsupported
        }
        try validate(document)
        return NativeObservation()
    }

    /// Rejects changed sessions and files outside the fixed trusted import directory.
    private func validate(_ document: NativeDocument) throws {
        guard try session() == binding, binding.active,
              (approvedLocal || importOwner != binding.uid), document.url.isFileURL,
              document.url.pathExtension.lowercased() == "txt",
              document.url.deletingLastPathComponent() == imports,
              imports.resolvingSymlinksInPath() == imports,
              document.url.resolvingSymlinksInPath() == document.url,
              let expected = identities[document.id] else {
            throw NativeWorkerFailure.document
        }
        var directory = stat()
        var file = stat()
        guard lstat(imports.path, &directory) == 0,
              directory.st_mode & S_IFMT == S_IFDIR,
              directory.st_uid == importOwner, directory.st_mode & 0o022 == 0,
              lstat(document.url.path, &file) == 0,
              file.st_mode & S_IFMT == S_IFREG, file.st_nlink == 1,
              file.st_dev == expected.device, file.st_ino == expected.inode,
              file.st_uid == expected.owner, expected.owner == importOwner,
              file.st_size >= 0, file.st_size <= 1_048_576 else {
            throw NativeWorkerFailure.document
        }
    }

    /// Returns the only app window if its native document URL matches the binding.
    private func documentWindow(_ app: AXUIElement,
                                document: NativeDocument) throws -> AXUIElement {
        guard let windows = try attribute(app, kAXWindowsAttribute) as? [AXUIElement],
              windows.count == 1,
              let name = try attribute(windows[0], kAXDocumentAttribute) as? String,
              let url = URL(string: name), url.standardizedFileURL == document.url else {
            throw NativeWorkerFailure.document
        }
        return windows[0]
    }

    /// Returns one fixed accessibility attribute or refuses unsupported controls.
    private func attribute(_ element: AXUIElement, _ name: String) throws -> CFTypeRef {
        var value: CFTypeRef?
        guard AXUIElementCopyAttributeValue(element, name as CFString, &value) == .success,
              let value else { throw NativeWorkerFailure.unsupported }
        return value
    }

    /// Traverses a bounded native element tree without accepting caller selectors.
    private func descendants(_ root: AXUIElement) throws -> [AXUIElement] {
        var found = [root]
        var index = 0
        while index < found.count {
            guard found.count <= 256 else { throw NativeWorkerFailure.unsupported }
            if let children = try? attribute(found[index], kAXChildrenAttribute) as? [AXUIElement] {
                found.append(contentsOf: children)
            }
            index += 1
        }
        return found
    }

    /// Captures one bound TextEdit window with audio disabled and bounded dimensions.
    private func capture(_ document: NativeDocument,
                         window: AXUIElement) async throws -> NativeObservation {
        guard CGPreflightScreenCaptureAccess() else { throw NativeWorkerFailure.unavailable }
        let content = try await SCShareableContent.excludingDesktopWindows(true,
                                                                         onScreenWindowsOnly: true)
        try validate(document)
        let windows = content.windows.filter { $0.owningApplication?.processID == binding.process }
        guard windows.count == 1, windows[0].frame.width > 0, windows[0].frame.height > 0 else {
            throw NativeWorkerFailure.document
        }
        try match(window, captured: windows[0], document: document)
        let filter = SCContentFilter(desktopIndependentWindow: windows[0])
        let config = SCStreamConfiguration()
        let scale = min(1, 1280 / max(windows[0].frame.width, windows[0].frame.height))
        config.width = max(1, Int(windows[0].frame.width * scale))
        config.height = max(1, Int(windows[0].frame.height * scale))
        config.capturesAudio = false
        config.showsCursor = false
        let image = try await SCScreenshotManager.captureImage(contentFilter: filter,
                                                              configuration: config)
        try validate(document)
        try match(window, captured: windows[0], document: document)
        guard let data = NSBitmapImageRep(cgImage: image).representation(using: .png, properties: [:]),
              data.count <= 4_194_304 else { throw NativeWorkerFailure.unsupported }
        return NativeObservation(imagePNG: data)
    }

    /// Requires the sole capture window to match the bound native document window.
    private func match(_ window: AXUIElement, captured: SCWindow,
                       document: NativeDocument) throws {
        let app = AXUIElementCreateApplication(binding.process)
        guard CFEqual(try documentWindow(app, document: document), window),
              let title = try attribute(window, kAXTitleAttribute) as? String,
              captured.title == title else { throw NativeWorkerFailure.document }
        let position = try attribute(window, kAXPositionAttribute)
        let size = try attribute(window, kAXSizeAttribute)
        guard CFGetTypeID(position) == AXValueGetTypeID(), CFGetTypeID(size) == AXValueGetTypeID() else {
            throw NativeWorkerFailure.unsupported
        }
        var point = CGPoint.zero
        var dimensions = CGSize.zero
        guard AXValueGetValue(unsafeDowncast(position, to: AXValue.self), .cgPoint, &point),
              AXValueGetValue(unsafeDowncast(size, to: AXValue.self), .cgSize, &dimensions),
              captured.frame == CGRect(origin: point, size: dimensions) else {
            throw NativeWorkerFailure.document
        }
    }
}
