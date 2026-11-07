// Coordinates native shutdown signals and timers.
import Foundation
// Boots guests without constructing host windows.
import Virtualization
// Prevents signals from silently terminating guests.
import Darwin

@MainActor
final class HeadlessSession {
    // A stop request acts as a power button. macOS answers with a dialog that shuts down
    // by itself after 60 seconds, so a shorter deadline would power off every time.
    static let shutdownGrace: Duration = .seconds(90)
    private enum State { case idle, starting, running, stopping, forcing, forceFailed, finished }
    private var state = State.idle
    private var stopPending = false
    private let startMachine: () async throws -> Void
    private let requestStop: () throws -> Void
    private let forceStop: (@escaping @MainActor (Bool) -> Void) -> Void
    private let shutdownDelay: Duration
    private var timeout: Task<Void, Never>?
    private var forced = false
    private let emit: (String) -> Void
    private let finish: (Int32) -> Void

    /// Accepts machine operations and reports lifecycle events to its owner.
    init(
        start: @escaping () async throws -> Void,
        requestStop: @escaping () throws -> Void,
        forceStop: @escaping (@escaping @MainActor (Bool) -> Void) -> Void,
        shutdownDelay: Duration = HeadlessSession.shutdownGrace,
        emit: @escaping (String) -> Void,
        finish: @escaping (Int32) -> Void
    ) {
        startMachine = start
        self.requestStop = requestStop
        self.forceStop = forceStop
        self.shutdownDelay = shutdownDelay
        self.emit = emit
        self.finish = finish
    }

    /// Starts once; VM startup alone never establishes backend readiness.
    func start() async {
        guard state == .idle else { return }
        state = .starting
        emit("starting")
        do {
            try await startMachine()
            guard state == .starting else { return }
            state = .running
            emit("vm_running")
            if stopPending { shutdown() }
        } catch {
            complete("start_failed", code: 1)
        }
    }

    /// Requests guest shutdown once, then allows 90 seconds before power-off.
    func shutdown() {
        if state == .idle {
            complete("stopped", code: 0)
            return
        }
        if state == .starting {
            stopPending = true
            return
        }
        guard state == .running || state == .forceFailed else { return }
        state = .stopping
        timeout?.cancel()
        timeout = Task { [weak self, shutdownDelay] in
            do { try await Task.sleep(for: shutdownDelay) }
            catch { return }
            self?.timedOut()
        }
        do {
            try requestStop()
            if state == .stopping { emit("shutdown_requested") }
        } catch {
            if state == .stopping { emit("shutdown_failed") }
        }
    }

    /// Powers off only a pending shutdown; ownership lasts until native confirmation.
    func timedOut() {
        guard state == .stopping else { return }
        state = .forcing
        forced = true
        emit("force_stopping")
        forceStop { [weak self] success in
            guard let self, state == .forcing else { return }
            if success {
                complete("forced_stopped", code: 2)
            } else {
                state = .forceFailed
                emit("force_stop_failed")
            }
        }
    }

    /// Completes once after the native delegate confirms a stop or failure.
    func stopped(failed: Bool = false) {
        if failed { complete("vm_failed", code: 1) }
        else { complete(forced ? "forced_stopped" : "stopped", code: forced ? 2 : 0) }
    }

    /// Sends one terminal event and exit result, ignoring late callbacks.
    private func complete(_ event: String, code: Int32) {
        guard state != .finished else { return }
        state = .finished
        timeout?.cancel()
        timeout = nil
        emit(event)
        finish(code)
    }
}

@MainActor
final class ShutdownSignals {
    private var sources: [any DispatchSourceSignal] = []
    private var registrations = 0
    private var cancelled = false
    private var waiters: [CheckedContinuation<Void, Never>] = []

    /// Routes termination requests onto the main actor without killing the process.
    init(handler: @escaping @MainActor () -> Void) {
        for value in [SIGTERM, SIGINT] {
            signal(value, SIG_IGN)
            let source = DispatchSource.makeSignalSource(signal: value, queue: .main)
            source.setEventHandler { Task { @MainActor in handler() } }
            source.setRegistrationHandler { [weak self] in
                Task { @MainActor in self?.registered() }
            }
            source.activate()
            sources.append(source)
        }
        // Losing the log reader must not cut power to the guest.
        signal(SIGPIPE, SIG_IGN)
    }

    /// Waits for both native observers before allowing guest startup.
    func ready() async {
        if cancelled || registrations == sources.count { return }
        await withCheckedContinuation { waiters.append($0) }
    }

    /// Releases startup waiters once every signal source is registered.
    private func registered() {
        registrations += 1
        if registrations == sources.count { releaseWaiters() }
    }

    /// Resumes outstanding startup waits once, including cancelled startup.
    private func releaseWaiters() {
        for waiter in waiters { waiter.resume() }
        waiters.removeAll()
    }

    /// Removes observers once the guest no longer needs process ownership.
    func cancel() {
        cancelled = true
        for source in sources { source.cancel() }
        sources.removeAll()
        releaseWaiters()
    }
}

@MainActor
final class HeadlessVM: NSObject, VZVirtualMachineDelegate {
    private let machine: VZVirtualMachine
    private let onStarted: (VZVirtualMachine) -> Void
    private var signals: ShutdownSignals?
    private var completion: CheckedContinuation<Int32, Never>?
    private lazy var session = HeadlessSession(
        start: { [unowned self] in try await startMachine() },
        requestStop: { [unowned self] in try machine.requestStop() },
        forceStop: { [unowned self] reply in
            machine.stop { error in
                Task { @MainActor in reply(error == nil) }
            }
        },
        emit: { [unowned self] in record($0) },
        finish: { [unowned self] in finish($0) }
    )

    /// Creates only the native machine, without AppKit views or activation.
    init(configuration: VZVirtualMachineConfiguration,
         onStarted: @escaping (VZVirtualMachine) -> Void = { _ in }) {
        machine = VZVirtualMachine(configuration: configuration)
        self.onStarted = onStarted
        super.init()
        machine.delegate = self
    }

    /// Awaits the callback API while keeping native calls on the main queue.
    private func startMachine() async throws {
        try await withCheckedThrowingContinuation { continuation in
            machine.start { result in continuation.resume(with: result) }
        }
        onStarted(machine)
    }

    /// Holds the runner until confirmed stop, including deadline-triggered power-off.
    func run() async -> Int32 {
        let observers = ShutdownSignals { [weak self] in self?.session.shutdown() }
        signals = observers
        return await withCheckedContinuation { continuation in
            completion = continuation
            Task {
                await observers.ready()
                await session.start()
            }
        }
    }

    /// Writes lifecycle status without exposing guest console contents.
    private func record(_ status: String) {
        if let data = try? JSONSerialization.data(withJSONObject: ["status": status]) {
            try? FileHandle.standardOutput.write(contentsOf: data + Data("\n".utf8))
        }
    }

    /// Cancels observers and returns only after startup failure or confirmed stop.
    private func finish(_ code: Int32) {
        signals?.cancel()
        signals = nil
        completion?.resume(returning: code)
        completion = nil
    }

    /// Forwards the native shutdown confirmation onto the machine's main queue.
    nonisolated func guestDidStop(_ virtualMachine: VZVirtualMachine) {
        Task { @MainActor [weak self] in self?.session.stopped() }
    }

    /// Reports native failure without exposing raw errors or restarting the guest.
    nonisolated func virtualMachine(
        _ virtualMachine: VZVirtualMachine, didStopWithError error: Error
    ) {
        Task { @MainActor [weak self] in self?.session.stopped(failed: true) }
    }
}
