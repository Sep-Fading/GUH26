// Encodes metadata and runs the callback.
import Foundation
// Fetches supported restore image requirements only.
import Virtualization

/// Prints restore metadata without downloading the image or creating a guest.
func report(_ image: VZMacOSRestoreImage) throws {
    guard let requirements = image.mostFeaturefulSupportedConfiguration else {
        throw NSError(domain: "MiniClawProbe", code: 1, userInfo: [
            NSLocalizedDescriptionKey: "No supported guest configuration",
        ])
    }
    let version = image.operatingSystemVersion
    let metadata: [String: Any] = [
        "supported": image.isSupported,
        "majorVersion": version.majorVersion,
        "version": "\(version.majorVersion).\(version.minorVersion).\(version.patchVersion)",
        "build": image.buildVersion,
        "url": image.url.absoluteString,
        "minimumCPUCount": requirements.minimumSupportedCPUCount,
        "minimumMemoryBytes": requirements.minimumSupportedMemorySize,
    ]
    let data = try JSONSerialization.data(withJSONObject: metadata, options: [.sortedKeys])
    FileHandle.standardOutput.write(data)
    FileHandle.standardOutput.write(Data("\n".utf8))
}

VZMacOSRestoreImage.fetchLatestSupported { result in
    do {
        try report(result.get())
        exit(0)
    } catch {
        FileHandle.standardError.write(Data("\(error)\n".utf8))
        exit(1)
    }
}
dispatchMain()
