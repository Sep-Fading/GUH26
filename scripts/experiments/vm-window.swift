// Provides window controls and native alerts.
import AppKit
// Displays guest pixels and forwards input.
import Virtualization
// Exchanges fixed bytes with guest probes.
import Darwin

/// Absorbs host input so the optional VM window remains a passive display.
private final class PassiveDisplayShield: NSView {
    /// Keeps keyboard focus away from the VM view.
    override var acceptsFirstResponder: Bool { true }
    /// Absorbs mouse clicks without touching the guest.
    override func mouseDown(with event: NSEvent) {}
    /// Prevents scrolling the guest display.
    override func scrollWheel(with event: NSEvent) {}
    /// Absorbs keystrokes while the window is focused.
    override func keyDown(with event: NSEvent) {}
}

@MainActor
final class VMWindow: NSObject, NSWindowDelegate, NSApplicationDelegate, VZVirtualMachineDelegate {
    private let guest: Guest
    private let machine: VZVirtualMachine
    private let window: NSWindow
    private let view = VZVirtualMachineView()
    private let inputShield = PassiveDisplayShield()
    private var passiveDisplay = false
    // A background VM stays open to input until its worker connects, so its user can log in.
    private var workerAttached = false
    private var revealedForLogin = false
    // The user may lift the shield from the window's own bar, outside the guest display.
    private var userControl = false
    private let controlSwitch = NSButton(checkboxWithTitle: "Control this VM",
        target: nil, action: nil)
    /// The shield guards a working background VM unless the user asked for control.
    private var shielded: Bool { passiveDisplay && workerAttached && !userControl }
    private let state = NSTextField(labelWithString: "Starting")
    private var vmName = "Offline test VM"
    private var probe: MasterSocketProbe?
    private var bridges: [GuestBridge] = []
    private var signals: ShutdownSignals?
    private var revealSignal: (any DispatchSourceSignal)?
    private var windowPositioned = false
    private lazy var session = HeadlessSession(
        start: { [unowned self] in try await startMachine() },
        requestStop: { [unowned self] in
            try machine.requestStop()
            // macOS answers a stop request with a dialog; confirm it instead of waiting it out.
            DispatchQueue.main.asyncAfter(deadline: .now() + 2.5) { [weak self] in
                self?.confirmShutdownDialog()
            }
        },
        forceStop: { [unowned self] reply in
            machine.stop { error in
                Task { @MainActor in reply(error == nil) }
            }
        },
        emit: { [unowned self] in record($0) },
        finish: { [unowned self] in finish($0) }
    )

    /// Opens an installed guest without regenerating its disk or identity.
    init(_ path: String, media: String? = nil, routes: String? = nil, network: String? = nil) throws {
        guest = try Guest(path)
        guard try String(contentsOf: guest.file("Status"), encoding: .utf8) == "installed\n" else {
            throw failure("Guest installation is not complete")
        }
        let configuration = try guest.configuration()
        if let media { try attachSetup(media, to: configuration) }
        if let network { try attachNetwork(network, to: configuration) }
        let configured = try routes.map { try guestRoutes(URL(fileURLWithPath: $0)) } ?? []
        if !configured.isEmpty { configuration.socketDevices = [VZVirtioSocketDeviceConfiguration()] }
        try configuration.validate()
        machine = VZVirtualMachine(configuration: configuration)
        _ = NSApplication.shared
        window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 1024, height: 680),
            styleMask: [.titled, .closable, .miniaturizable, .resizable],
            backing: .buffered, defer: false
        )
        super.init()
        machine.delegate = self
        window.delegate = self
        window.isReleasedWhenClosed = false
        window.title = "MiniClaw - Offline test VM"
        window.minSize = NSSize(width: 640, height: 480)
        view.virtualMachine = machine
        view.capturesSystemKeys = false
        view.automaticallyReconfiguresDisplay = true
        configureContent()
        if media != nil { probe = MasterSocketProbe() }
        bridges = configured.map { GuestBridge($0) }
    }

    /// Keeps shutdown and fullscreen controls outside the guest display.
    private func configureContent() {
        let shutdown = NSButton(title: "Shut down", target: self, action: #selector(shutDown))
        let fullscreen = NSButton(
            image: NSImage(
                systemSymbolName: "arrow.up.left.and.arrow.down.right",
                accessibilityDescription: "Fullscreen"
            )!,
            target: self, action: #selector(fullScreen)
        )
        fullscreen.toolTip = "Fullscreen"
        controlSwitch.target = self
        controlSwitch.action = #selector(toggleControl)
        controlSwitch.toolTip = "Let your keyboard and mouse reach this VM. Agents keep working in it."
        controlSwitch.isHidden = true
        let bar = NSStackView(views: [state, controlSwitch, shutdown, fullscreen])
        bar.orientation = .horizontal
        bar.spacing = 12
        bar.edgeInsets = NSEdgeInsets(top: 8, left: 12, bottom: 8, right: 12)
        let content = NSView()
        for child in [bar, view, inputShield] {
            child.translatesAutoresizingMaskIntoConstraints = false
            content.addSubview(child)
        }
        NSLayoutConstraint.activate([
            bar.topAnchor.constraint(equalTo: content.topAnchor),
            bar.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            bar.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            view.topAnchor.constraint(equalTo: bar.bottomAnchor),
            view.leadingAnchor.constraint(equalTo: content.leadingAnchor),
            view.trailingAnchor.constraint(equalTo: content.trailingAnchor),
            view.bottomAnchor.constraint(equalTo: content.bottomAnchor),
            inputShield.topAnchor.constraint(equalTo: view.topAnchor),
            inputShield.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            inputShield.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            inputShield.bottomAnchor.constraint(equalTo: view.bottomAnchor),
        ])
        inputShield.isHidden = true
        window.contentView = content
    }

    /// Shows one movable window and boots its offline guest.
    func show(background: Bool = false) {
        let app = NSApplication.shared
        passiveDisplay = background
        // The shield protects a working agent VM, not a guest still waiting at its login.
        inputShield.isHidden = !shielded
        if background {
            for bridge in bridges { bridge.onPaired = { [weak self] in self?.workerConnected() } }
            // A guest that logs in by itself never needs a window; one that waits does.
            DispatchQueue.main.asyncAfter(deadline: .now() + 25) { [weak self] in
                guard let self, !self.workerAttached else { return }
                self.revealedForLogin = true
                self.state.stringValue = "\(self.vmName) · Log in to this VM to continue"
                self.reveal()
            }
        }
        vmName = bridges.isEmpty ? "Offline test VM" : background ? "Master VM" : "Project VM"
        window.title = "MiniClaw - \(vmName)"
        state.stringValue = "\(vmName) · Starting"
        app.setActivationPolicy(background ? .accessory : .regular)
        app.delegate = self
        let menu = NSMenu()
        let appItem = NSMenuItem()
        let appMenu = NSMenu()
        appMenu.addItem(
            withTitle: "Quit VM Lab",
            action: #selector(NSApplication.terminate(_:)), keyEquivalent: "q"
        )
        appItem.submenu = appMenu
        menu.addItem(appItem)
        app.mainMenu = menu
        signal(SIGUSR1, SIG_IGN)
        let revealSource = DispatchSource.makeSignalSource(signal: SIGUSR1, queue: .main)
        revealSource.setEventHandler { [weak self] in
            Task { @MainActor in self?.reveal() }
        }
        revealSource.activate()
        revealSignal = revealSource
        if !background { reveal() }
        let observers = ShutdownSignals { [weak self] in self?.session.shutdown() }
        signals = observers
        Task {
            await observers.ready()
            await session.start()
        }
    }

    /// Shields a background VM once its worker runs, and hides a window shown only for login.
    func workerConnected() {
        guard passiveDisplay, !workerAttached else { return }
        workerAttached = true
        controlSwitch.isHidden = false
        applyShield()
        guard revealedForLogin else { state.stringValue = "\(vmName) · Running"; return }
        revealedForLogin = false
        // Say why the window is about to go, so hiding does not look like a crash.
        state.stringValue = "\(vmName) · Logged in. This window hides in a moment; reopen it from MiniClaw."
        DispatchQueue.main.asyncAfter(deadline: .now() + 5) { [weak self] in
            guard let self, !self.userControl else { return }
            self.state.stringValue = "\(self.vmName) · Running"
            self.window.orderOut(nil)
            NSApplication.shared.setActivationPolicy(.accessory)
        }
    }

    /// Applies the shield and gives focus to whichever view should take input.
    private func applyShield() {
        inputShield.isHidden = !shielded
        if window.isVisible { window.makeFirstResponder(shielded ? inputShield : view) }
    }

    /// Lifts or restores the shield when the user flips the switch in the bar.
    @objc private func toggleControl() {
        userControl = controlSwitch.state == .on
        applyShield()
    }

    /// Presses Return in the guest, which is the default Shut Down button of the dialog
    /// macOS shows after a stop request. Needs no permission inside the guest.
    func confirmShutdownDialog() {
        guard machine.state == .running else { return }
        for kind in [NSEvent.EventType.keyDown, .keyUp] {
            guard let event = NSEvent.keyEvent(with: kind, location: .zero, modifierFlags: [],
                timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: window.windowNumber,
                context: nil, characters: "\r", charactersIgnoringModifiers: "\r",
                isARepeat: false, keyCode: 36) else { continue }
            if kind == .keyDown { view.keyDown(with: event) } else { view.keyUp(with: event) }
        }
    }

    /// Reveals the running VM. Input reaches the guest in setup runs, and in a background
    /// run only until its worker connects.
    private func reveal() {
        NSApplication.shared.setActivationPolicy(.regular)
        if !windowPositioned { window.center(); windowPositioned = true }
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(shielded ? inputShield : view)
        NSApplication.shared.activate(ignoringOtherApps: true)
    }

    /// Boots the guest and connects its private channels after native startup.
    private func startMachine() async throws {
        try await withCheckedThrowingContinuation { continuation in
            machine.start { result in continuation.resume(with: result) }
        }
        for bridge in bridges { bridge.start(machine) }
        if let device = machine.socketDevices.first as? VZVirtioSocketDevice {
            probe?.start(device)
        }
    }

    /// Shows shutdown progress and forwards bounded status to the launcher.
    private func record(_ status: String) {
        switch status {
        case "vm_running": state.stringValue = "\(vmName) · Running"
        case "shutdown_requested": state.stringValue = "\(vmName) · Shutting down"
        case "shutdown_failed": state.stringValue = "\(vmName) · Shutdown failed; power-off in 90s"
        case "force_stopping": state.stringValue = "\(vmName) · Powering off; unsaved data may be lost"
        case "force_stop_failed": state.stringValue = "\(vmName) · Power-off failed; retry Shut down"
        default: break
        }
        try? report(["status": status])
    }

    /// Releases channels after confirmed stop and preserves the forced exit code.
    private func finish(_ code: Int32) {
        revealSignal?.cancel()
        signals?.cancel()
        for bridge in bridges { bridge.stop() }
        Darwin.exit(code)
    }

    /// Expands the existing window while preserving its host controls.
    @objc private func fullScreen() {
        window.toggleFullScreen(nil)
    }

    /// Requests shutdown; repeated clicks leave the original deadline unchanged.
    @objc private func shutDown() {
        session.shutdown()
    }

    /// Hides the optional window without stopping the running guest.
    func windowShouldClose(_ sender: NSWindow) -> Bool {
        window.orderOut(nil)
        return false
    }

    /// Prevents app quit from silently cutting guest power.
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        if machine.state == .stopped || machine.state == .error { return .terminateNow }
        shutDown()
        return .terminateCancel
    }

    /// Confirms guest shutdown through the shared lifecycle, retaining its disk.
    nonisolated func guestDidStop(_ virtualMachine: VZVirtualMachine) {
        Task { @MainActor [weak self] in self?.session.stopped() }
    }

    /// Reports native failure without restarting or deleting the guest.
    nonisolated func virtualMachine(
        _ virtualMachine: VZVirtualMachine, didStopWithError error: Error
    ) {
        Task { @MainActor [weak self] in self?.session.stopped(failed: true) }
    }

    /// Reports a detached NIC and shuts down without switching to unrestricted networking.
    nonisolated func virtualMachine(
        _ virtualMachine: VZVirtualMachine, networkDevice: VZNetworkDevice,
        attachmentWasDisconnectedWithError error: Error
    ) {
        Task { @MainActor [weak self] in
            self?.record("network_failed")
            self?.session.shutdown()
        }
    }
}

@MainActor
final class MasterSocketProbe: NSObject, @preconcurrency VZVirtioSocketListenerDelegate {
    private let listener = VZVirtioSocketListener()
    private var connection: VZVirtioSocketConnection?
    private var reader: DispatchSourceRead?
    private var timeout: DispatchWorkItem?

    /// Listens on this owned VM only; this probe has no command or forwarding API.
    func start(_ device: VZVirtioSocketDevice) {
        listener.delegate = self
        device.setSocketListener(listener, forPort: 10240)
    }

    /// Accepts one bounded handshake and rejects concurrent guest connections.
    func listener(
        _ listener: VZVirtioSocketListener,
        shouldAcceptNewConnection connection: VZVirtioSocketConnection,
        from socketDevice: VZVirtioSocketDevice
    ) -> Bool {
        guard self.connection == nil else { return false }
        self.connection = connection
        let fd = connection.fileDescriptor
        _ = fcntl(fd, F_SETFL, O_NONBLOCK)
        var one: Int32 = 1
        _ = setsockopt(fd, SOL_SOCKET, SO_NOSIGPIPE, &one,
            socklen_t(MemoryLayout.size(ofValue: one)))
        let reader = DispatchSource.makeReadSource(fileDescriptor: fd, queue: .main)
        reader.setEventHandler { [weak self] in self?.received() }
        self.reader = reader
        let timeout = DispatchWorkItem { [weak self] in self?.finish(false) }
        self.timeout = timeout
        DispatchQueue.main.asyncAfter(deadline: .now() + 3, execute: timeout)
        reader.resume()
        DispatchQueue.main.async { [weak self] in
            guard self?.connection != nil else { return }
            var greeting: UInt8 = 77
            if Darwin.write(fd, &greeting, 1) != 1 { self?.finish(false) }
        }
        return true
    }

    /// Reports success only after the guest returns the fixed acknowledgement.
    private func received() {
        guard let connection else { return }
        var reply: UInt8 = 0
        let count = Darwin.read(connection.fileDescriptor, &reply, 1)
        if count < 0 && (errno == EAGAIN || errno == EINTR) { return }
        finish(count == 1 && reply == 67)
    }

    /// Closes the probe and prints no guest-supplied contents.
    private func finish(_ passed: Bool) {
        timeout?.cancel()
        timeout = nil
        reader?.cancel()
        reader = nil
        connection?.close()
        connection = nil
        try? report(["status": passed ? "guest_socket_verified" : "guest_socket_failed"])
    }
}
