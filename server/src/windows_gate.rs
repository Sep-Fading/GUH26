//! Decides whether this Windows installation can enforce the sandbox, and says why when it cannot.
//!
//! `seatbelt::available()` answers from here on Windows. The answer is true only when every
//! precondition of a sandboxed run holds on this installation; otherwise the first missing one is
//! named in words a user can act on. A closed gate behaves exactly as before native acceptance:
//! no sandbox is created, nothing is stamped on a folder, and nothing runs unrestricted instead.
//! This file holds the decision and its wording, which need no Windows API; the native part
//! collects the facts.

/// The lowest Windows build the sandbox is enabled on: Windows 11, version 24H2.
///
/// Microsoft documents no minimum for `PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY`
/// (the LPAC attribute), and the Winsock catalog key list is this build's. Every native isolation
/// result so far was measured on build 26100, so nothing older is claimed to be enforceable.
pub const MIN_BUILD: u32 = 26100;

/// The one environment switch: any non-empty value closes the gate. It can never open it.
pub const SWITCH_OFF: &str = "MINICLAW_DISABLE_SANDBOX";

/// What the installed network service answered to a read-only readiness check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    /// Registered, running as LocalSystem, and it proved the installation key on a fresh connection.
    Ready,
    /// `MINICLAW_NETWORK_CONFIG` is unset or names no file: setup never installed the service.
    NotConfigured,
    /// The configuration names a service that Windows no longer has.
    NotInstalled,
    /// Windows has the service and it is not running.
    Stopped,
    /// Something answered, or was registered, that is not this installation's service.
    Unverified,
}

/// Everything the decision looks at, collected once per check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facts {
    /// The kernel's build number, or `None` when Windows would not say.
    pub build: Option<u32>,
    /// Whether process creation accepts the LPAC attribute.
    pub lpac: bool,
    /// Whether the pinned shell beside the backend has the released bytes.
    pub shell: bool,
    /// Whether the pinned ConPTY pair beside the backend has the released bytes and loads.
    pub terminal: bool,
    pub service: Service,
    /// Whether `MINICLAW_DISABLE_SANDBOX` is set.
    pub switched_off: bool,
    /// What the last recovery of earlier runs' records left unresolved, if anything.
    pub recovery: Option<String>,
}

/// The first precondition that does not hold.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Missing {
    SwitchedOff,
    /// Records of earlier runs are unresolved (the detail names the first one and why). Only
    /// sandboxed work waits for them; the backend itself keeps serving.
    Recovery(String),
    Windows { build: Option<u32> },
    Lpac,
    Shell,
    Terminal,
    ServiceNotConfigured,
    ServiceNotInstalled,
    ServiceStopped,
    ServiceUnverified,
}

impl Missing {
    /// Says what is missing and what the user can do, in one or two plain sentences.
    pub fn reason(&self) -> String {
        match self {
            Self::SwitchedOff => format!("The sandbox was switched off for this run with {SWITCH_OFF}."),
            Self::Recovery(detail) => format!(
                "Sandboxed commands, the terminal, Git views and sandboxed MCP servers are paused: MiniClaw could not finish \
                 cleaning up after an earlier sandboxed run, and its records and permissions were kept. {}{} Chat still \
                 works. Fix the item named here (or restart Windows if asked), then choose Try again or restart MiniClaw.",
                detail.trim_end(), if detail.trim_end().ends_with('.') { "" } else { "." }),
            Self::Windows { build: Some(build) } => format!(
                "MiniClaw's sandbox needs Windows 11, version 24H2 (build {MIN_BUILD}) or later. This computer runs build {build}."),
            Self::Windows { build: None } => format!(
                "MiniClaw's sandbox needs Windows 11, version 24H2 (build {MIN_BUILD}) or later, and the Windows version could not be read."),
            Self::Lpac => "This Windows version cannot start the restricted process type MiniClaw's sandbox uses.".into(),
            Self::Shell => "The sandbox's shell is missing or was changed. Run `miniclaw update`, then start MiniClaw again.".into(),
            Self::Terminal => "The sandbox's terminal runtime is missing or was changed. Run `miniclaw update`, then start MiniClaw again.".into(),
            Self::ServiceNotConfigured => "The MiniClaw network service is not installed. Run `miniclaw resume` to finish setup, or `miniclaw setup` to install again.".into(),
            Self::ServiceNotInstalled => "The MiniClaw network service was removed from Windows. Run `miniclaw setup` to install it again.".into(),
            Self::ServiceStopped => "The MiniClaw network service is not running. Start \"MiniClaw network isolation\" in Windows Services, or restart Windows.".into(),
            Self::ServiceUnverified => "The MiniClaw network service could not be verified as this installation's. Run `miniclaw update` to repair it.".into(),
        }
    }
}

/// Opens the gate only when every precondition holds; otherwise names the first that does not.
pub fn decide(facts: &Facts) -> Result<(), Missing> {
    if facts.switched_off { return Err(Missing::SwitchedOff); }
    if let Some(detail) = &facts.recovery { return Err(Missing::Recovery(detail.clone())); }
    if !facts.build.is_some_and(|build| build >= MIN_BUILD) { return Err(Missing::Windows { build: facts.build }); }
    if !facts.lpac { return Err(Missing::Lpac); }
    if !facts.shell { return Err(Missing::Shell); }
    if !facts.terminal { return Err(Missing::Terminal); }
    match facts.service {
        Service::Ready => Ok(()),
        Service::NotConfigured => Err(Missing::ServiceNotConfigured),
        Service::NotInstalled => Err(Missing::ServiceNotInstalled),
        Service::Stopped => Err(Missing::ServiceStopped),
        Service::Unverified => Err(Missing::ServiceUnverified),
    }
}

/// Why the product's own Git views are unavailable although the sandbox may be.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitMissing {
    Sandbox(Missing),
    /// Setup installed no private Git.
    NotInstalled,
    /// The private Git record or its folder failed validation.
    Invalid,
}

impl GitMissing {
    /// Says why Changes and discard are unavailable.
    pub fn reason(&self) -> String {
        match self {
            Self::Sandbox(missing) => missing.reason(),
            Self::NotInstalled => "MiniClaw's private Git is not installed. Run `miniclaw update` to install it.".into(),
            Self::Invalid => "MiniClaw's private Git installation failed its check. Run `miniclaw update` to repair it.".into(),
        }
    }
}

/// Git needs the sandbox first, then its own verified installation.
pub fn decide_git(sandbox: Result<(), Missing>, git: Result<bool, ()>) -> Result<(), GitMissing> {
    sandbox.map_err(GitMissing::Sandbox)?;
    match git {
        Ok(true) => Ok(()),
        Ok(false) => Err(GitMissing::NotInstalled),
        Err(()) => Err(GitMissing::Invalid),
    }
}

/// What the page and the tests read: the gate, its two dependent capabilities, and each reason.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Report {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Set when the reason is unresolved records of an earlier run; the page then offers to try
    /// MiniClaw's recovery again (`POST /api/sandbox/repair`).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub recovery: bool,
    pub git: Part,
    pub network: Part,
    /// What the network service last said about its Windows access entries (Windows only): healthy,
    /// repaired after Windows removed them, or failed with the reason; unknown, with the gate's reason,
    /// while the service is stopped ([`access_report`]). Absent when nothing was read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub access: Option<crate::windows_winsock_acl::AccessStatus>,
}

/// What the page is told about the network service's Windows access entries. The status the service
/// published describes them only while it runs: a stopped service checks and repairs nothing, so its last
/// "healthy" (or any other) status is history. Then the access is "unknown", with the gate's own reason.
/// Without a ready or stopped service of this installation there is nothing to report.
pub fn access_report(sandbox: Result<(), Missing>, published: Option<crate::windows_winsock_acl::AccessStatus>)
    -> Option<crate::windows_winsock_acl::AccessStatus> {
    match sandbox {
        Ok(()) => published,
        Err(missing @ Missing::ServiceStopped) => Some(crate::windows_winsock_acl::AccessStatus {
            state: "unknown", time: None, reason: Some(missing.reason()) }),
        Err(_) => None,
    }
}

/// One capability that needs the sandbox and something of its own.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct Part {
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Part {
    /// Builds the part from a decision and its wording.
    pub fn from(result: Result<(), String>) -> Self {
        Self { available: result.is_ok(), reason: result.err() }
    }
}

#[cfg(windows)]
mod native {
    use super::{Facts, GitMissing, Missing, SWITCH_OFF, Service, decide, decide_git};
    // Bounds how long one answer is reused.
    use std::{sync::{Mutex, OnceLock}, time::{Duration, Instant}};
    // Reads the kernel version and probes process attributes.
    use windows_sys::Win32::System::{SystemInformation::OSVERSIONINFOW, Threading::{DeleteProcThreadAttributeList,
        InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY, UpdateProcThreadAttribute}};

    /// How long one answer is reused. A hot path pays one lock and a clock read in between; a
    /// stopped or started service is noticed within this time. A stale "available" opens nothing:
    /// every launch authenticates the service again before its sandbox exists.
    const REUSE: Duration = Duration::from_secs(2);

    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn RtlGetVersion(info: *mut OSVERSIONINFOW) -> i32;
    }

    /// Reads the kernel's own build number, which no compatibility manifest changes.
    fn build() -> Option<u32> {
        let mut info: OSVERSIONINFOW = unsafe { std::mem::zeroed() };
        info.dwOSVersionInfoSize = size_of::<OSVERSIONINFOW>() as u32;
        (unsafe { RtlGetVersion(&mut info) } == 0 && info.dwMajorVersion >= 10).then_some(info.dwBuildNumber)
    }

    /// Asks process creation whether it knows the LPAC attribute, creating nothing.
    fn lpac() -> bool {
        let mut bytes = 0;
        unsafe { InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut bytes); }
        if bytes == 0 || bytes > 65536 { return false; }
        let mut storage = vec![0usize; bytes.div_ceil(size_of::<usize>())];
        let list = storage.as_mut_ptr().cast();
        if unsafe { InitializeProcThreadAttributeList(list, 1, 0, &mut bytes) } == 0 { return false; }
        let policy: u32 = 1;
        let known = unsafe { UpdateProcThreadAttribute(list, 0, PROC_THREAD_ATTRIBUTE_ALL_APPLICATION_PACKAGES_POLICY as usize,
            (&policy as *const u32).cast(), size_of::<u32>(), std::ptr::null_mut(), std::ptr::null()) } != 0;
        unsafe { DeleteProcThreadAttributeList(list); }
        known
    }

    /// Collects every fact afresh, except those that cannot change while this process runs.
    pub fn facts() -> Facts {
        static SYSTEM: OnceLock<(Option<u32>, bool)> = OnceLock::new();
        let (build, lpac) = *SYSTEM.get_or_init(|| (build(), lpac()));
        let switched_off = std::env::var_os(SWITCH_OFF).is_some_and(|value| !value.is_empty());
        // The runtimes are pinned for the life of the backend, so their modules keep the answer.
        let shell = crate::windows_shell::runtime().is_ok();
        let terminal = crate::windows_conpty::runtime().is_ok();
        // The service is asked last and only when everything else holds: it is the one slow fact.
        let recovery = crate::windows_sandbox::recovery_problem();
        let ready = Facts { build, lpac, shell, terminal, service: Service::Ready, switched_off, recovery };
        let service = if decide(&ready).is_ok() {
            let path = std::env::var_os("MINICLAW_NETWORK_CONFIG").map(std::path::PathBuf::from);
            crate::windows_network_broker::readiness(path.as_deref())
        } else { Service::Unverified };
        Facts { service, ..ready }
    }

    /// The last answer and when it was given.
    static LAST: Mutex<Option<(Instant, Result<(), Missing>)>> = Mutex::new(None);

    /// Forgets the last answer, so the next question is answered afresh (after a repair).
    pub fn forget() {
        *LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    /// Answers the gate, reusing the last answer for a short time.
    pub fn state() -> Result<(), Missing> {
        // One caller checks while the others wait for its answer instead of repeating it.
        let mut last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((time, answer)) = last.as_ref() && time.elapsed() < REUSE { return answer.clone(); }
        let answer = decide(&facts());
        // Said once per change, not at every check: the reason the sandbox became unavailable.
        if last.as_ref().map(|(_, earlier)| earlier) != Some(&answer) {
            match &answer {
                Err(missing) => tracing::warn!(reason = %missing.reason(), "The sandbox is unavailable"),
                Ok(()) => tracing::info!("The sandbox is available"),
            }
        }
        *last = Some((Instant::now(), answer.clone()));
        answer
    }

    /// Answers whether the product's own Git can run: the sandbox, then the private installation.
    pub fn git() -> Result<(), GitMissing> {
        static LAST: Mutex<Option<(Instant, Result<bool, ()>)>> = Mutex::new(None);
        let sandbox = state();
        if sandbox.is_err() { return decide_git(sandbox, Ok(false)); }
        let mut last = LAST.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let installed = match last.as_ref() {
            Some((time, installed)) if time.elapsed() < REUSE => *installed,
            _ => {
                let installed = crate::windows_git::load().map(|runtime| runtime.is_some()).map_err(|error| {
                    tracing::warn!(%error, "The private Git installation failed its check");
                });
                *last = Some((Instant::now(), installed));
                installed
            }
        };
        decide_git(sandbox, installed)
    }

    /// Answers whether a sandboxed command can be given approved hosts. The service decides again
    /// at every lease; this only repeats what it last published, so the page can say it early.
    pub fn network() -> Result<(), String> {
        state().map_err(|missing| missing.reason())?;
        let path = std::env::var_os("MINICLAW_NETWORK_CONFIG").map(std::path::PathBuf::from);
        match path.as_deref().and_then(crate::windows_network_broker::winsock_health) {
            Some(Err(reason)) => Err(reason),
            _ => Ok(()),
        }
    }

    /// Builds the whole report for the page.
    pub fn report() -> super::Report {
        let sandbox = state();
        super::Report { available: sandbox.is_ok(), recovery: matches!(sandbox, Err(Missing::Recovery(_))),
            reason: sandbox.err().map(|missing| missing.reason()),
            git: super::Part::from(git().map_err(|missing| missing.reason())),
            network: super::Part::from(network()), access: access() }
    }

    /// Reads the network service's published access status while the service itself is ready; a
    /// stopped service's status is reported as unknown (`super::access_report`).
    pub fn access() -> Option<crate::windows_winsock_acl::AccessStatus> {
        let sandbox = state();
        let published = if sandbox.is_ok() {
            std::env::var_os("MINICLAW_NETWORK_CONFIG").map(std::path::PathBuf::from)
                .and_then(|path| crate::windows_network_broker::winsock_status(&path))
        } else { None };
        super::access_report(sandbox, published)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The guest's kernel reports a build, and process creation knows the LPAC attribute.
        #[test]
        fn this_windows_reports_its_build_and_lpac_support() {
            let build = build().expect("Windows reports its build");
            assert!(build >= 10_240, "{build}");
            assert!(lpac(), "build {build} accepts the LPAC attribute");
        }

        /// The gate equals the decision over the facts, and its answer is reused.
        #[test]
        fn the_gate_is_the_decision_over_this_installation() {
            let first = state();
            assert_eq!(first, decide(&facts()));
            assert_eq!(crate::seatbelt::available(), first.is_ok());
            assert_eq!(crate::seatbelt::unavailable_reason(), first.err().map(|missing| missing.reason()));
            let started = Instant::now();
            for _ in 0..10_000 { let _ = state(); }
            // Reused answers cost a lock and a clock read; allow one refresh inside the loop.
            assert!(started.elapsed() < Duration::from_millis(500), "{:?}", started.elapsed());
        }

        /// Writes one private client configuration for a service name, as setup does.
        fn configuration(folder: &std::path::Path, name: &str, body: Option<&str>) -> std::path::PathBuf {
            use std::io::Write;
            let path = folder.join(name);
            let owner = crate::windows_network_broker::current_user_sid().unwrap();
            let service = format!("MiniClawNetwork-{}", uuid::Uuid::new_v4().simple());
            let text = body.map(str::to_owned).unwrap_or_else(|| serde_json::json!({ "owner_sid": owner, "pipe_name": service,
                "service_name": service, "service_exe": format!(r"C:\Program Files\{service}\miniclaw-network-service.exe"),
                "secret_hex": "ab".repeat(32) }).to_string());
            let mut file = crate::platform_fs::private_file(&path, true).unwrap();
            file.write_all(text.as_bytes()).unwrap();
            path
        }

        /// A missing configuration, a service Windows does not have and an untrusted file are told apart.
        #[test]
        fn readiness_names_what_is_missing() {
            use crate::windows_network_broker::readiness;
            assert_eq!(readiness(None), Service::NotConfigured);
            let folder = std::env::temp_dir().join(format!("mc-gate-config-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&folder).unwrap();
            assert_eq!(readiness(Some(&folder.join("absent.json"))), Service::NotConfigured);
            assert_eq!(readiness(Some(&configuration(&folder, "unknown.json", None))), Service::NotInstalled);
            assert_eq!(readiness(Some(&configuration(&folder, "garbage.json", Some("{}")))), Service::Unverified);
            assert_eq!(readiness(Some(std::path::Path::new("relative.json"))), Service::Unverified);
            std::fs::remove_dir_all(folder).unwrap();
        }

        /// Checks the gate against the service state the runner arranged, and times the check.
        #[test]
        #[ignore = "the runner arranges the service and names the state in MINICLAW_GATE_EXPECT"]
        fn the_gate_follows_the_service() {
            use crate::windows_network_broker::readiness;
            let expect = std::env::var("MINICLAW_GATE_EXPECT").expect("MINICLAW_GATE_EXPECT");
            let path = std::env::var_os("MINICLAW_NETWORK_CONFIG").map(std::path::PathBuf::from);
            let wait = |wanted: Result<(), Missing>| {
                let started = Instant::now();
                while state() != wanted {
                    assert!(started.elapsed() < Duration::from_secs(90), "the gate never became {wanted:?}; it is {:?}", state());
                    std::thread::sleep(Duration::from_millis(50));
                }
            };
            match expect.as_str() {
                "ready" => {
                    assert_eq!(readiness(path.as_deref()), Service::Ready);
                    assert_eq!(state(), Ok(()));
                    assert!(crate::seatbelt::available() && crate::seatbelt::unavailable_reason().is_none());
                    assert_eq!(network(), Ok(()), "the service published healthy Winsock access");
                    // One whole check with nothing reused, then the reused answer.
                    let started = Instant::now();
                    for _ in 0..20 { assert_eq!(decide(&facts()), Ok(())); }
                    let fresh = started.elapsed() / 20;
                    let started = Instant::now();
                    for _ in 0..100_000 { let _ = std::hint::black_box(state()); }
                    let reused = started.elapsed();
                    println!("GATE COST fresh check {fresh:?} each (20 runs); 100000 reused answers in {reused:?} including at most one refresh");
                    assert!(fresh < Duration::from_millis(250), "{fresh:?}");
                }
                "stopped" => {
                    assert_eq!(readiness(path.as_deref()), Service::Stopped);
                    assert_eq!(state(), Err(Missing::ServiceStopped));
                    assert!(!crate::seatbelt::available());
                    assert_eq!(crate::seatbelt::unavailable_reason(), Some(Missing::ServiceStopped.reason()));
                    assert_eq!(git(), Err(GitMissing::Sandbox(Missing::ServiceStopped)));
                    assert_eq!(network(), Err(Missing::ServiceStopped.reason()));
                    // The service's last published status is not reported as current while it is stopped.
                    let reported = access().expect("a stopped service is reported");
                    assert_eq!((reported.state, reported.reason), ("unknown", Some(Missing::ServiceStopped.reason())));
                }
                "foreign" => {
                    // The configuration holds another key: the running service cannot prove it.
                    assert_eq!(readiness(path.as_deref()), Service::Unverified);
                    assert_eq!(state(), Err(Missing::ServiceUnverified));
                }
                "unconfigured" => {
                    assert!(path.is_none());
                    assert_eq!(state(), Err(Missing::ServiceNotConfigured));
                }
                "transition" => {
                    // The runner stops the service while this waits, then starts it again.
                    assert_eq!(state(), Ok(()));
                    println!("GATE TRANSITION waiting for the stop");
                    wait(Err(Missing::ServiceStopped));
                    println!("GATE TRANSITION saw the stop; waiting for the start");
                    wait(Ok(()));
                    println!("GATE TRANSITION saw the start");
                }
                other => panic!("unknown expectation {other}"),
            }
        }
    }
}
#[cfg(windows)]
pub use native::{access, facts, forget, git, network, report, state};

#[cfg(test)]
mod tests {
    use super::*;

    /// An installation on which every precondition holds.
    fn ready() -> Facts {
        Facts { build: Some(MIN_BUILD), lpac: true, shell: true, terminal: true, service: Service::Ready, switched_off: false, recovery: None }
    }

    #[test]
    fn opens_only_when_everything_holds() {
        assert_eq!(decide(&ready()), Ok(()));
        assert_eq!(decide(&Facts { build: Some(27_900), ..ready() }), Ok(()));
    }

    #[test]
    fn old_or_unknown_windows_closes_it() {
        assert_eq!(decide(&Facts { build: Some(MIN_BUILD - 1), ..ready() }), Err(Missing::Windows { build: Some(MIN_BUILD - 1) }));
        assert_eq!(decide(&Facts { build: Some(22_631), ..ready() }), Err(Missing::Windows { build: Some(22_631) }));
        assert_eq!(decide(&Facts { build: None, ..ready() }), Err(Missing::Windows { build: None }));
        assert!(Missing::Windows { build: Some(19_045) }.reason().contains("build 19045"));
        assert!(Missing::Windows { build: None }.reason().contains("could not be read"));
    }

    #[test]
    fn missing_lpac_support_closes_it() {
        assert_eq!(decide(&Facts { lpac: false, ..ready() }), Err(Missing::Lpac));
    }

    #[test]
    fn an_unverified_shell_closes_it() {
        assert_eq!(decide(&Facts { shell: false, ..ready() }), Err(Missing::Shell));
        assert!(Missing::Shell.reason().contains("miniclaw update"));
    }

    #[test]
    fn an_unverified_terminal_runtime_closes_it() {
        assert_eq!(decide(&Facts { terminal: false, ..ready() }), Err(Missing::Terminal));
    }

    #[test]
    fn every_service_state_but_ready_closes_it() {
        for (service, missing, words) in [
            (Service::NotConfigured, Missing::ServiceNotConfigured, "miniclaw resume"),
            (Service::NotInstalled, Missing::ServiceNotInstalled, "miniclaw setup"),
            (Service::Stopped, Missing::ServiceStopped, "is not running"),
            (Service::Unverified, Missing::ServiceUnverified, "could not be verified"),
        ] {
            assert_eq!(decide(&Facts { service, ..ready() }), Err(missing.clone()));
            assert!(missing.reason().contains(words), "{}", missing.reason());
            assert!(missing.reason().contains("MiniClaw network service"));
        }
    }

    #[test]
    fn the_switch_closes_a_ready_installation_and_opens_nothing() {
        assert_eq!(decide(&Facts { switched_off: true, ..ready() }), Err(Missing::SwitchedOff));
        assert!(Missing::SwitchedOff.reason().contains(SWITCH_OFF));
        // Without the switch a missing precondition still closes the gate: no fact opens it alone.
        for broken in [Facts { build: None, ..ready() }, Facts { lpac: false, ..ready() }, Facts { shell: false, ..ready() },
            Facts { terminal: false, ..ready() }, Facts { service: Service::Stopped, ..ready() }] {
            assert!(decide(&broken).is_err());
            assert!(decide(&Facts { switched_off: true, ..broken }).is_err());
        }
    }

    /// Unresolved records of earlier runs close the gate for sandboxed work only, with the record's
    /// own reason; they are named before every other missing precondition but the switch.
    #[test]
    fn unresolved_recovery_closes_it_with_its_reason() {
        let detail = "Windows sandbox recovery retained 1 unresolved records; native job/object cleanup must complete before serving. \
            First: MiniClaw.Run.0123: C:\\Users\\S\\AppData\\Local\\Roblox\\x: Access is denied.";
        let stuck = Facts { recovery: Some(detail.into()), ..ready() };
        assert_eq!(decide(&stuck), Err(Missing::Recovery(detail.into())));
        let reason = Missing::Recovery(detail.into()).reason();
        assert!(reason.contains(detail) && reason.contains("Chat still works") && reason.contains("Try again"), "{reason}");
        let unfinished = Missing::Recovery("First: x: it cannot be checked".into()).reason();
        assert!(unfinished.contains("it cannot be checked. Chat still works."), "{unfinished}");
        assert!(!reason.contains('\n'), "{reason}");
        assert_eq!(decide(&Facts { service: Service::Stopped, shell: false, ..stuck.clone() }), Err(Missing::Recovery(detail.into())));
        assert_eq!(decide(&Facts { switched_off: true, ..stuck }), Err(Missing::SwitchedOff));
        assert_eq!(decide_git(Err(Missing::Recovery(detail.into())), Ok(true)), Err(GitMissing::Sandbox(Missing::Recovery(detail.into()))));
    }

    #[test]
    fn the_first_missing_precondition_is_the_one_named() {
        let nothing = Facts { build: Some(10_240), lpac: false, shell: false, terminal: false, service: Service::NotConfigured, switched_off: false, recovery: None };
        assert_eq!(decide(&nothing), Err(Missing::Windows { build: Some(10_240) }));
        assert_eq!(decide(&Facts { build: Some(MIN_BUILD), ..nothing.clone() }), Err(Missing::Lpac));
        assert_eq!(decide(&Facts { build: Some(MIN_BUILD), lpac: true, ..nothing.clone() }), Err(Missing::Shell));
        assert_eq!(decide(&Facts { build: Some(MIN_BUILD), lpac: true, shell: true, ..nothing.clone() }), Err(Missing::Terminal));
        assert_eq!(decide(&Facts { build: Some(MIN_BUILD), lpac: true, shell: true, terminal: true, ..nothing }), Err(Missing::ServiceNotConfigured));
    }

    #[test]
    fn reasons_are_short_plain_sentences() {
        for missing in [Missing::SwitchedOff, Missing::Windows { build: Some(1) }, Missing::Windows { build: None }, Missing::Lpac,
            Missing::Shell, Missing::Terminal, Missing::ServiceNotConfigured, Missing::ServiceNotInstalled,
            Missing::ServiceStopped, Missing::ServiceUnverified] {
            let reason = missing.reason();
            assert!(reason.ends_with('.') && reason.len() < 220 && !reason.contains('\n'), "{reason}");
            assert!(!reason.contains("LPAC") && !reason.contains("AppContainer") && !reason.contains("SID"), "{reason}");
        }
    }

    /// What the service published holds only while it runs. A stopped service's last "healthy" (or
    /// "repaired", or even "failed") is history: the page is told the access is unknown, with the gate's reason.
    #[test]
    fn a_stopped_service_never_reports_its_last_access_as_current() {
        use crate::windows_winsock_acl::AccessStatus;
        let healthy = AccessStatus { state: "healthy", time: Some(5), reason: None };
        let failed = AccessStatus { state: "failed", time: Some(6), reason: Some("its entries were removed".into()) };
        assert_eq!(access_report(Ok(()), Some(healthy.clone())), Some(healthy.clone()));
        assert_eq!(access_report(Ok(()), Some(failed.clone())), Some(failed.clone()));
        assert_eq!(access_report(Ok(()), None), None);
        let unknown = Some(AccessStatus { state: "unknown", time: None, reason: Some(Missing::ServiceStopped.reason()) });
        for published in [Some(healthy.clone()), Some(AccessStatus { state: "repaired", time: Some(7), reason: None }), Some(failed), None] {
            assert_eq!(access_report(Err(Missing::ServiceStopped), published), unknown);
        }
        // Without a service of this installation there is nothing to report at all.
        for missing in [Missing::ServiceNotConfigured, Missing::ServiceNotInstalled, Missing::ServiceUnverified, Missing::SwitchedOff, Missing::Shell] {
            assert_eq!(access_report(Err(missing), Some(healthy.clone())), None);
        }
    }

    #[test]
    fn git_needs_the_sandbox_and_its_own_installation() {
        assert_eq!(decide_git(Ok(()), Ok(true)), Ok(()));
        assert_eq!(decide_git(Ok(()), Ok(false)), Err(GitMissing::NotInstalled));
        assert_eq!(decide_git(Ok(()), Err(())), Err(GitMissing::Invalid));
        // A closed sandbox is the reason given, whatever Git's own state.
        assert_eq!(decide_git(Err(Missing::ServiceStopped), Ok(true)), Err(GitMissing::Sandbox(Missing::ServiceStopped)));
        assert_eq!(GitMissing::Sandbox(Missing::ServiceStopped).reason(), Missing::ServiceStopped.reason());
        assert!(GitMissing::NotInstalled.reason().contains("private Git is not installed"));
    }
}
