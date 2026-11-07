// Reads host resources and encodes JSON.
import Foundation
// Queries virtual machine support and limits.
import Virtualization

let host = ProcessInfo.processInfo
let report: [String: Any] = [
    "hostOS": host.operatingSystemVersionString,
    "hostCPUCount": host.processorCount,
    "hostMemoryBytes": host.physicalMemory,
    "virtualizationSupported": VZVirtualMachine.isSupported,
    "minimumCPUCount": VZVirtualMachineConfiguration.minimumAllowedCPUCount,
    "maximumCPUCount": VZVirtualMachineConfiguration.maximumAllowedCPUCount,
    "minimumMemoryBytes": VZVirtualMachineConfiguration.minimumAllowedMemorySize,
    "maximumMemoryBytes": VZVirtualMachineConfiguration.maximumAllowedMemorySize,
]
let data = try JSONSerialization.data(withJSONObject: report, options: [.sortedKeys])
FileHandle.standardOutput.write(data)
FileHandle.standardOutput.write(Data("\n".utf8))
