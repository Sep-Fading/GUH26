// Encodes observations from controlled lifecycle callbacks.
import Foundation
// Sends shutdown signals within this probe.
import Darwin

enum ProbeError: Error { case simulated }

@main
enum SessionProbe {
    /// Exercises one lifecycle scenario without creating a virtual machine.
    @MainActor
    static func main() async throws {
        let scenario = CommandLine.arguments[1]
        var events: [String] = []
        var exits: [Int32] = []
        var starts = 0
        var stops = 0
        var onShutdown: (() -> Void)?
        var powerReply: (@MainActor (Bool) -> Void)?
        var powerCalls = 0
        var shutdownAt: ContinuousClock.Instant?
        var session: HeadlessSession!
        session = HeadlessSession(
            start: {
                starts += 1
                if scenario == "stop-during-start" {
                    session.shutdown()
                    session.shutdown()
                }
                if scenario == "stopped-during-start" { session.stopped() }
                if scenario == "start-failure" { throw ProbeError.simulated }
            },
            requestStop: {
                stops += 1
                if scenario == "stop-retry", stops == 1 { throw ProbeError.simulated }
            },
            forceStop: { reply in
                powerCalls += 1
                if scenario == "default-deadline", let shutdownAt {
                    precondition(shutdownAt.duration(to: .now) >= .seconds(90))
                }
                powerReply = reply
            },
            shutdownDelay: scenario.hasPrefix("timer-") ? .milliseconds(40)
                : HeadlessSession.shutdownGrace,
            emit: {
                events.append($0)
                if $0 == "shutdown_requested" { onShutdown?() }
            },
            finish: { exits.append($0) }
        )
        if scenario == "stop-before-start" { session.shutdown() }
        await session.start()
        if scenario == "signal-term" || scenario == "signal-int" {
            let signals = ShutdownSignals { session.shutdown() }
            await signals.ready()
            await withCheckedContinuation { continuation in
                onShutdown = { continuation.resume() }
                // The native event filter observes process-directed signals only.
                precondition(kill(getpid(), scenario == "signal-term" ? SIGTERM : SIGINT) == 0)
            }
            signals.cancel()
        }
        switch scenario {
        case "duplicate-start":
            await session.start()
        case "shutdown":
            session.shutdown()
            session.shutdown()
        case "stopped":
            session.shutdown()
            session.stopped()
            session.stopped()
            session.shutdown()
            session.timedOut()
        case "timeout":
            session.shutdown()
            session.timedOut()
            session.timedOut()
            session.shutdown()
            precondition(powerCalls == 1)
        case "power-success", "power-failure", "power-race":
            session.shutdown()
            session.timedOut()
            precondition(exits.isEmpty && powerCalls == 1)
            if scenario == "power-race" { session.stopped() }
            powerReply?(scenario != "power-failure")
            if scenario != "power-failure" { session.stopped() }
        case "timer-fire", "timer-cancel":
            session.shutdown()
            precondition(powerCalls == 0)
            if scenario == "timer-cancel" { session.stopped() }
            try await Task.sleep(for: .milliseconds(100))
            precondition(powerCalls == (scenario == "timer-fire" ? 1 : 0))
        case "default-deadline":
            precondition(HeadlessSession.shutdownGrace == .seconds(90))
            shutdownAt = .now
            session.shutdown()
            try await Task.sleep(for: .seconds(45))
            precondition(powerCalls == 0)
            session.shutdown()
            try await Task.sleep(for: .seconds(46))
            precondition(powerCalls == 1)
            powerReply?(true)
        case "stop-retry":
            session.shutdown()
            session.shutdown()
            session.timedOut()
        case "power-retry":
            session.shutdown()
            session.timedOut()
            powerReply?(false)
            session.shutdown()
            session.timedOut()
            powerReply?(true)
            precondition(powerCalls == 2)
        case "guest-error":
            session.stopped(failed: true)
            session.stopped()
        case "early-timeout":
            session.timedOut()
        default:
            break
        }
        let result: [String: Any] = [
            "events": events, "exits": exits, "starts": starts, "stops": stops,
        ]
        let data = try JSONSerialization.data(withJSONObject: result, options: .sortedKeys)
        try FileHandle.standardOutput.write(contentsOf: data)
    }
}
