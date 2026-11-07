// Stores guest identity and reports results.
import Foundation
// Configures offline hardware and installs macOS.
import Virtualization
// Protects files and locks guest ownership.
import Darwin
// Runs the guest window event loop.
import AppKit

let gib: UInt64 = 1024 * 1024 * 1024
let diskBytes = 64 * gib
let labMarker = Data("miniclaw-offline-vm-lab-v1\n".utf8)

struct MacResources: Codable {
    var cpu: Int = 4
    var memory: UInt64 = 8
    var disk: UInt64 = 64

    /// Rejects unsupported or unbounded allocations before touching guest files.
    func validate() throws {
        guard (2...64).contains(cpu), (4...512).contains(memory),
              (64...4096).contains(disk) else { throw failure("Invalid Master resources") }
    }

    /// Parses the wizard's explicit CPU, RAM and disk allocations.
    static func parse(_ values: ArraySlice<String>) throws -> MacResources {
        let values = Array(values)
        guard values.count == 3, let cpu = Int(values[0]),
              let memory = UInt64(values[1]), let disk = UInt64(values[2]) else {
            throw failure("Invalid Master resources")
        }
        let result = MacResources(cpu: cpu, memory: memory, disk: disk)
        try result.validate()
        return result
    }
}

/// Returns a readable lab error without terminating inside helpers.
func failure(_ message: String) -> NSError {
    NSError(domain: "MiniClawVMLab", code: 1, userInfo: [
        NSLocalizedDescriptionKey: message,
    ])
}

/// Writes one JSON record to stdout for the experiment harness.
func report(_ value: [String: Any]) throws {
    let data = try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
    FileHandle.standardOutput.write(data + Data("\n".utf8))
}

/// Builds the approved offline hardware without attaching persistent storage.
func baseConfiguration(_ resources: MacResources = MacResources()) -> VZVirtualMachineConfiguration {
    let configuration = VZVirtualMachineConfiguration()
    configuration.cpuCount = resources.cpu
    configuration.memorySize = resources.memory * gib
    configuration.bootLoader = VZMacOSBootLoader()
    configuration.networkDevices = []
    configuration.directorySharingDevices = []
    configuration.socketDevices = []
    configuration.serialPorts = []
    configuration.consoleDevices = []
    configuration.audioDevices = []
    configuration.usbControllers = []
    let graphics = VZMacGraphicsDeviceConfiguration()
    graphics.displays = [VZMacGraphicsDisplayConfiguration(
        widthInPixels: 1440, heightInPixels: 900, pixelsPerInch: 110
    )]
    configuration.graphicsDevices = [graphics]
    configuration.keyboards = [VZUSBKeyboardConfiguration()]
    configuration.pointingDevices = [VZUSBScreenCoordinatePointingDeviceConfiguration()]
    return configuration
}

/// Reports actual device counts, not a separate policy description.
func describe(_ configuration: VZVirtualMachineConfiguration, disk: UInt64 = diskBytes) -> [String: Any] {
    [
        "cpuCount": configuration.cpuCount,
        "memoryBytes": configuration.memorySize,
        "diskBytes": disk,
        "networks": configuration.networkDevices.count,
        "shares": configuration.directorySharingDevices.count,
        "sockets": configuration.socketDevices.count,
        "serialPorts": configuration.serialPorts.count,
        "consoles": configuration.consoleDevices.count,
        "audioDevices": configuration.audioDevices.count,
        "usbControllers": configuration.usbControllers.count,
        "displays": configuration.graphicsDevices.count,
        "keyboards": configuration.keyboards.count,
        "pointingDevices": configuration.pointingDevices.count,
        "storageReadOnly": configuration.storageDevices.compactMap {
            ($0 as? VZVirtioBlockDeviceConfiguration)?.attachment
                as? VZDiskImageStorageDeviceAttachment
        }.map { $0.isReadOnly },
    ]
}

/// Attaches one read-only setup disk and a probe socket, without networking or shares.
func attachSetup(_ path: String, to configuration: VZVirtualMachineConfiguration) throws {
    let media = URL(fileURLWithPath: path).standardizedFileURL
    try rejectLink(media)
    let info = try media.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey])
    guard info.isRegularFile == true, let size = info.fileSize,
          size > 0, size <= 512 * 1024 * 1024 else {
        throw failure("Setup media must be a regular image of at most 512 MiB")
    }
    let attachment = try VZDiskImageStorageDeviceAttachment(url: media, readOnly: true)
    configuration.storageDevices.append(VZVirtioBlockDeviceConfiguration(attachment: attachment))
    configuration.socketDevices = [VZVirtioSocketDeviceConfiguration()]
}

/// Rejects a symlink at the supplied path; system parent aliases remain usable.
func rejectLink(_ url: URL) throws {
    let values = try url.resourceValues(forKeys: [.isSymbolicLinkKey])
    guard values.isSymbolicLink != true else {
        throw failure("Symbolic links are not allowed: \(url.lastPathComponent)")
    }
}

/// Opens one existing guest and holds its process lock until release.
final class Guest {
    let root: URL
    private let lock: Int32

    /// Validates the directory and refuses simultaneous guest owners.
    init(_ path: String) throws {
        let url = URL(fileURLWithPath: path).standardizedFileURL
        var directory: ObjCBool = false
        guard FileManager.default.fileExists(atPath: url.path, isDirectory: &directory),
              directory.boolValue else {
            throw failure("Guest directory does not exist")
        }
        try rejectLink(url)
        root = url.resolvingSymlinksInPath()
        let marker = root.appendingPathComponent("LabOwner")
        guard FileManager.default.fileExists(atPath: marker.path) else {
            throw failure("Not a MiniClaw test guest")
        }
        try rejectLink(marker)
        guard (try? Data(contentsOf: marker)) == labMarker else {
            throw failure("Not a MiniClaw test guest")
        }
        lock = open(root.appendingPathComponent(".lock").path,
                    O_CREAT | O_RDWR | O_NOFOLLOW | O_CLOEXEC, 0o600)
        guard lock >= 0 else { throw failure("Cannot open guest lock") }
        guard flock(lock, LOCK_EX | LOCK_NB) == 0 else {
            close(lock)
            throw failure("Guest is already in use")
        }
    }

    /// Releases ownership without deleting any guest files.
    deinit {
        close(lock)
    }

    /// Resolves a required regular file, rejecting links and missing data.
    func file(_ name: String) throws -> URL {
        let url = root.appendingPathComponent(name)
        try rejectLink(url)
        let values = try url.resourceValues(forKeys: [.isRegularFileKey])
        guard values.isRegularFile == true else {
            throw failure("Guest file is not a regular file: \(name)")
        }
        return url
    }

    /// Reconstructs validated hardware using the saved identity and disk.
    func configuration() throws -> VZVirtualMachineConfiguration {
        let resources = try resources()
        let configuration = baseConfiguration(resources)
        let platform = VZMacPlatformConfiguration()
        guard let hardware = VZMacHardwareModel(dataRepresentation:
            try Data(contentsOf: file("HardwareModel"))), hardware.isSupported,
              let identifier = VZMacMachineIdentifier(dataRepresentation:
            try Data(contentsOf: file("MachineIdentifier"))) else {
            throw failure("Guest hardware or identity is invalid")
        }
        platform.hardwareModel = hardware
        platform.machineIdentifier = identifier
        platform.auxiliaryStorage = VZMacAuxiliaryStorage(url: try file("AuxiliaryStorage"))
        configuration.platform = platform
        let disk = try file("Disk.img")
        let attributes = try FileManager.default.attributesOfItem(atPath: disk.path)
        guard (attributes[.size] as? NSNumber)?.uint64Value == resources.disk * gib else {
            throw failure("Guest disk size differs from the approved allocation")
        }
        let attachment = try VZDiskImageStorageDeviceAttachment(url: disk, readOnly: false)
        configuration.storageDevices = [VZVirtioBlockDeviceConfiguration(attachment: attachment)]
        try configuration.validate()
        return configuration
    }

    /// Reads saved allocations; older retained labs keep their original defaults.
    func resources() throws -> MacResources {
        guard FileManager.default.fileExists(atPath: root.appendingPathComponent("Resources.json").path)
            else { return MacResources() }
        let value = try JSONDecoder().decode(MacResources.self, from: Data(contentsOf: file("Resources.json")))
        try value.validate()
        return value
    }
}

/// Creates new guest files from an image; never overwrites a directory.
func prepare(_ path: String, imageURL: URL, resources: MacResources = MacResources()) async throws {
    try resources.validate()
    let root = URL(fileURLWithPath: path).standardizedFileURL
    guard !FileManager.default.fileExists(atPath: root.path) else {
        throw failure("Guest directory already exists")
    }
    let image = try await VZMacOSRestoreImage.image(from: imageURL)
    guard let requirements = image.mostFeaturefulSupportedConfiguration,
          requirements.minimumSupportedCPUCount <= resources.cpu,
          requirements.minimumSupportedMemorySize <= resources.memory * gib else {
        throw failure("Restore image exceeds the approved allocation")
    }
    guard mkdir(root.path, 0o700) == 0 else {
        throw failure("Cannot create a new guest directory")
    }
    try labMarker.write(to: root.appendingPathComponent("LabOwner"))
    try JSONEncoder().encode(resources).write(to: root.appendingPathComponent("Resources.json"))
    let guest = try Guest(root.path)
    let hardware = requirements.hardwareModel
    try hardware.dataRepresentation.write(to: root.appendingPathComponent("HardwareModel"))
    try VZMacMachineIdentifier().dataRepresentation.write(
        to: root.appendingPathComponent("MachineIdentifier")
    )
    _ = try VZMacAuxiliaryStorage(
        creatingStorageAt: root.appendingPathComponent("AuxiliaryStorage"),
        hardwareModel: hardware, options: []
    )
    let disk = open(root.appendingPathComponent("Disk.img").path,
                    O_CREAT | O_EXCL | O_RDWR | O_NOFOLLOW, 0o600)
    guard disk >= 0 else { throw failure("Cannot create guest disk") }
    defer { close(disk) }
    guard ftruncate(disk, off_t(resources.disk * gib)) == 0 else {
        throw failure("Cannot allocate guest disk")
    }
    _ = try guest.configuration()
    try Data("prepared\n".utf8).write(to: root.appendingPathComponent("Status"), options: .atomic)
    try report(["status": "prepared", "build": image.buildVersion])
}

/// Installs once into a prepared guest and preserves files on failure.
@MainActor
func install(_ path: String, imageURL: URL) async throws {
    let guest = try Guest(path)
    let status = try guest.file("Status")
    guard try String(contentsOf: status, encoding: .utf8) == "prepared\n" else {
        throw failure("Installation requires a freshly prepared guest")
    }
    let machine = VZVirtualMachine(configuration: try guest.configuration())
    let installer = VZMacOSInstaller(virtualMachine: machine, restoringFromImageAt: imageURL)
    try Data("installing\n".utf8).write(to: status, options: .atomic)
    let progress = installer.progress.observe(\.fractionCompleted, options: [.new]) { value, _ in
        try? report(["status": "installing", "percent": Int(value.fractionCompleted * 100)])
    }
    defer { progress.invalidate() }
    do {
        try await installer.install()
        try Data("installed\n".utf8).write(to: status, options: .atomic)
        try report(["status": "installed"])
    } catch {
        try? Data("install-failed\n".utf8).write(to: status, options: .atomic)
        throw error
    }
    withExtendedLifetime(guest) {}
}

/// Dispatches explicit lab operations; inspection never starts a VM.
@MainActor
func execute(_ args: [String]) async throws {
    if args == ["policy"] {
        try report(describe(baseConfiguration()))
    } else if args.count == 5, args[0] == "policy", args[1] == "--resources" {
        let resources = try MacResources.parse(args[2...])
        try report(describe(baseConfiguration(resources), disk: resources.disk * gib))
    } else if args.count == 3, args[0] == "policy", args[1] == "--media" {
        let configuration = baseConfiguration()
        try attachSetup(args[2], to: configuration)
        try report(describe(configuration))
    } else if args.first == "inspect",
              args.count == 2 || (args.count == 3 && args[2] == "--socket") {
        let guest = try Guest(args[1])
        let configuration = try guest.configuration()
        var result = describe(configuration, disk: try guest.resources().disk * gib)
        if args.count == 3 {
            configuration.socketDevices = [VZVirtioSocketDeviceConfiguration()]
            do {
                try configuration.validate()
                result["socketSupported"] = true
            } catch {
                result["socketSupported"] = false
                result["socketError"] = error.localizedDescription
            }
        }
        result["status"] = try String(contentsOf: guest.file("Status"), encoding: .utf8)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        try report(result)
        withExtendedLifetime(guest) {}
    } else if args.count == 3, args[0] == "init" {
        try await prepare(args[1], imageURL: URL(fileURLWithPath: args[2]))
    } else if args.count == 6, args[0] == "init" {
        try await prepare(args[1], imageURL: URL(fileURLWithPath: args[2]),
            resources: MacResources.parse(args[3...]))
    } else if args.count == 3, args[0] == "install" {
        try await install(args[1], imageURL: URL(fileURLWithPath: args[2]))
    } else if args.count == 2, args[0] == "headless" {
        let guest = try Guest(args[1])
        guard try String(contentsOf: guest.file("Status"), encoding: .utf8) == "installed\n" else {
            throw failure("Guest installation is not complete")
        }
        let runner = HeadlessVM(configuration: try guest.configuration())
        let code = await runner.run()
        withExtendedLifetime(guest) {}
        exit(code)
    } else {
        throw failure(
            "Usage: vm-lab policy | inspect DIR [--socket] | init DIR IPSW | install DIR IPSW"
            + " | run DIR [--media IMAGE] | headless DIR"
        )
    }
}

@main
enum VMLab {
    /// Runs native windows on the main thread and CLI operations on its queue.
    @MainActor
    static func main() {
        let args = Array(CommandLine.arguments.dropFirst())
        if args.first == "run",
           args.count == 2 || (args.count == 3 && args[2] == "--background")
                || (args.count == 4 && args[2] == "--media")
                || (args.count == 6 && args[2] == "--media" && args[4] == "--routes")
                || (args.count == 7 && args[2] == "--media" && args[4] == "--routes"
                    && args[6] == "--background")
                || (args.count == 8 && args[2] == "--media" && args[4] == "--routes"
                    && args[6] == "--network")
                || (args.count == 9 && args[2] == "--media" && args[4] == "--routes"
                    && args[6] == "--network" && args[8] == "--background") {
            do {
                let window = try VMWindow(args[1], media: args.count >= 4 ? args[3] : nil,
                    routes: args.count >= 6 ? args[5] : nil,
                    network: args.count >= 8 ? args[7] : nil)
                window.show(background: args.last == "--background")
                withExtendedLifetime(window) { NSApplication.shared.run() }
            } catch {
                abortWith(error)
            }
        } else {
            Task { @MainActor in
                do {
                    try await execute(args)
                    exit(0)
                } catch {
                    abortWith(error)
                }
            }
            dispatchMain()
        }
    }

    /// Reports an operation failure without deleting retained lab data.
    static func abortWith(_ error: Error) -> Never {
        FileHandle.standardError.write(Data("\(error.localizedDescription)\n".utf8))
        exit(1)
    }
}
