// Decodes inventories and encodes budget results.
import Foundation
// Returns failure status for invalid input.
import Darwin

struct Resources: Codable {
    var cpuCount: UInt64
    var memoryBytes: UInt64
    var diskBytes: UInt64

    /// Rejects missing capacity or unusable VM allocations.
    func validate() throws {
        guard cpuCount > 0, memoryBytes > 0, diskBytes > 0 else {
            throw BudgetError.invalidInput
        }
    }
}

struct Limits: Codable {
    let cpuPercent: UInt64
    let memoryPercent: UInt64
    let diskPercent: UInt64

    static let defaults = Limits(cpuPercent: 50, memoryPercent: 50, diskPercent: 25)

    /// Rejects percentages outside the configurable 1-100 range.
    func validate() throws {
        guard [cpuPercent, memoryPercent, diskPercent].allSatisfy({ (1...100).contains($0) }) else {
            throw BudgetError.invalidInput
        }
    }
}

enum GuestState: String, Codable {
    case starting, running, paused, stopping, stopped
}

struct Reservation: Decodable {
    let id: String
    let state: GuestState
    let cpuCount: UInt64
    let memoryBytes: UInt64
    let diskBytes: UInt64
}

struct Inventory: Decodable {
    let host: Resources
    let limits: Limits?
    let vms: [Reservation]
    let extraDiskBytes: UInt64
}

struct BudgetResult: Encodable {
    let allowed: Bool
    let limits: Limits
    let capacity: Resources
    let used: Resources
    let exceeded: [String]
}

enum BudgetError: Error {
    case invalidInput
}

/// Floors a percentage without overflowing large byte counts.
func portion(_ total: UInt64, _ percent: UInt64) -> UInt64 {
    (total / 100) * percent + ((total % 100) * percent) / 100
}

/// Adds allocations, rejecting overflow instead of approving a wrapped total.
func sum(_ total: UInt64, _ value: UInt64) throws -> UInt64 {
    let (result, overflow) = total.addingReportingOverflow(value)
    guard !overflow else { throw BudgetError.invalidInput }
    return result
}

/// Checks a complete proposed inventory; it neither reserves resources nor starts VMs.
func evaluate(_ inventory: Inventory) throws -> BudgetResult {
    try inventory.host.validate()
    let limits = inventory.limits ?? .defaults
    try limits.validate()
    let capacity = Resources(
        cpuCount: portion(inventory.host.cpuCount, limits.cpuPercent),
        memoryBytes: portion(inventory.host.memoryBytes, limits.memoryPercent),
        diskBytes: portion(inventory.host.diskBytes, limits.diskPercent)
    )
    var used = Resources(cpuCount: 0, memoryBytes: 0, diskBytes: inventory.extraDiskBytes)
    var identifiers = Set<String>()
    for vm in inventory.vms {
        guard !vm.id.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
              identifiers.insert(vm.id).inserted else { throw BudgetError.invalidInput }
        try Resources(cpuCount: vm.cpuCount, memoryBytes: vm.memoryBytes,
                      diskBytes: vm.diskBytes).validate()
        used.diskBytes = try sum(used.diskBytes, vm.diskBytes)
        // Paused and stopping guests still own their reservations.
        if vm.state != .stopped {
            used.cpuCount = try sum(used.cpuCount, vm.cpuCount)
            used.memoryBytes = try sum(used.memoryBytes, vm.memoryBytes)
        }
    }
    var exceeded: [String] = []
    if used.cpuCount > capacity.cpuCount { exceeded.append("cpu") }
    if used.memoryBytes > capacity.memoryBytes { exceeded.append("memory") }
    if used.diskBytes > capacity.diskBytes { exceeded.append("disk") }
    return BudgetResult(allowed: exceeded.isEmpty, limits: limits,
                        capacity: capacity, used: used, exceeded: exceeded)
}

@main
enum ResourceProbe {
    /// Reads bounded synthetic JSON and reports policy results without host changes.
    static func main() {
        do {
            let input = FileHandle.standardInput.readData(ofLength: 1_048_577)
            guard input.count <= 1_048_576 else { throw BudgetError.invalidInput }
            let inventory = try JSONDecoder().decode(Inventory.self, from: input)
            let result = try evaluate(inventory)
            let encoder = JSONEncoder()
            encoder.outputFormatting = [.sortedKeys]
            FileHandle.standardOutput.write(try encoder.encode(result) + Data("\n".utf8))
        } catch {
            FileHandle.standardError.write(Data("Invalid resource inventory\n".utf8))
            exit(1)
        }
    }
}
