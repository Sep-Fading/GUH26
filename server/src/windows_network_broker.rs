//! Authenticates narrowly scoped Windows network leases without granting privileged execution.

// Encodes a closed protocol and signs each fresh connection challenge.
use serde::{Deserialize, Serialize};
use std::io;

// Signs requests without disclosing the shared key to an unverified pipe endpoint.
use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub owner_sid: String,
    pub pipe_name: String,
    pub service_name: String,
    pub service_exe: String,
    pub secret_hex: String,
}
impl Config {
    /// Rejects arbitrary object namespaces and malformed installation keys.
    fn validate(&self) -> io::Result<()> {
        if !user_sid(&self.owner_sid)
            || !identifier(&self.pipe_name, "MiniClawNetwork-") || !identifier(&self.service_name, "MiniClawNetwork-")
            || self.secret_hex.len() != 64 || self.secret_hex.bytes().any(|b| !b.is_ascii_hexdigit())
            || self.service_exe.is_empty() || self.service_exe.contains('\0') {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        Ok(())
    }
    /// Decodes the installation secret only after its fixed shape has been validated.
    fn key(&self) -> io::Result<Vec<u8>> {
        self.validate()?;
        (0..64).step_by(2).map(|i| u8::from_str_radix(&self.secret_hex[i..i + 2], 16)
            .map_err(|_| io::ErrorKind::InvalidInput.into())).collect()
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub profile: String,
    pub job_name: String,
    pub job_handle: u64,
    pub proxy_port: u16,
    pub connect_ports: Vec<u16>,
    pub listen_ports: Vec<u16>,
}
#[cfg_attr(not(windows), allow(dead_code))]
impl Request {
    /// Restricts the request to generated per-run objects and one loopback port.
    fn validate(&self) -> io::Result<()> {
        if !identifier(&self.profile, "MiniClaw.Run.") || !identifier(&self.job_name, "Global\\MiniClaw.Job.")
            || self.job_handle == 0 || self.job_handle > isize::MAX as u64 || self.proxy_port == 0
            || !ports(&self.connect_ports) || !ports(&self.listen_ports) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        Ok(())
    }
    /// Binds one exact request to a fresh server challenge.
    fn sign(self, config: &Config, challenge: &str) -> io::Result<Signed> {
        self.validate()?;
        let proof = mac(config, challenge, &self)?.finalize().into_bytes().to_vec();
        Ok(Signed { request: self, proof })
    }
}

/// Authenticated identity of one installation's OS lifetime; never supplied by an agent.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BootEpoch { pub installation: String, pub epoch: String }
impl BootEpoch {
    /// Only a verified different boot of the same installation proves old processes cannot remain.
    pub fn supersedes(&self, saved: &Self) -> bool {
        self.valid() && saved.valid() && self.installation == saved.installation && self.epoch != saved.epoch
    }
    fn valid(&self) -> bool {
        identifier(&self.installation, "MiniClawNetwork-") && identifier(&self.epoch, "")
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(not(windows), allow(dead_code))]
struct Hello { challenge: String, boot: BootEpoch, server_proof: Vec<u8> }
#[cfg_attr(not(windows), allow(dead_code))]
impl Hello {
    /// Proves that the running service loaded the same installation key as the backend.
    fn new(config: &Config, challenge: String, boot: BootEpoch) -> io::Result<Self> {
        let server_proof = hello_mac(config, &challenge, &boot)?.finalize().into_bytes().to_vec();
        Ok(Self { challenge, boot, server_proof })
    }
    /// Rejects mismatched installations before disclosing any process or job identifiers.
    fn verify(&self, config: &Config) -> io::Result<()> {
        hello_mac(config, &self.challenge, &self.boot)?.verify_slice(&self.server_proof)
            .map_err(|_| io::ErrorKind::PermissionDenied.into())
    }
}

/// Keeps service identity proofs separate from authorization requests.
#[cfg_attr(not(windows), allow(dead_code))]
fn hello_mac(config: &Config, challenge: &str, boot: &BootEpoch) -> io::Result<Hmac<Sha256>> {
    if !boot.valid() || boot.installation != config.service_name { return Err(io::ErrorKind::InvalidData.into()); }
    if challenge.is_empty() || challenge.len() > 128 { return Err(io::ErrorKind::InvalidInput.into()); }
    let mut mac = Hmac::<Sha256>::new_from_slice(&config.key()?).map_err(io::Error::other)?;
    mac.update(b"MiniClaw Windows network service v2\0");
    mac.update(challenge.as_bytes());
    mac.update(&[0]);
    mac.update(&serde_json::to_vec(boot)?);
    Ok(mac)
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Signed { request: Request, proof: Vec<u8> }
#[cfg_attr(not(windows), allow(dead_code))]
impl Signed {
    /// Verifies the fresh request in constant time before privileged object inspection.
    fn verify(&self, config: &Config, challenge: &str) -> io::Result<()> {
        self.request.validate()?;
        mac(config, challenge, &self.request)?.verify_slice(&self.proof)
            .map_err(|_| io::ErrorKind::PermissionDenied.into())
    }
}

/// Names one finished run whose processes the service is asked to confirm dead.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Attest {
    pub profile: String,
    pub job_name: String,
}
impl Attest {
    /// Accepts only the generated per-run profile and global job names.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn validate(&self) -> io::Result<()> {
        if !identifier(&self.profile, "MiniClaw.Run.") || !identifier(&self.job_name, "Global\\MiniClaw.Job.") {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        Ok(())
    }
    /// Binds one exact question to a fresh server challenge.
    #[cfg_attr(not(windows), allow(dead_code))]
    fn sign(self, config: &Config, challenge: &str) -> io::Result<SignedAttest> {
        let proof = attest_mac(config, challenge, &self)?.finalize().into_bytes().to_vec();
        Ok(SignedAttest { attest: self, proof })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SignedAttest { attest: Attest, proof: Vec<u8> }
impl SignedAttest {
    /// Verifies the question in constant time before any process is inspected.
    #[cfg_attr(not(windows), allow(dead_code))]
    fn verify(&self, config: &Config, challenge: &str) -> io::Result<()> {
        attest_mac(config, challenge, &self.attest)?.verify_slice(&self.proof)
            .map_err(|_| io::ErrorKind::PermissionDenied.into())
    }
}

/// Asks the service to check its Windows access entries now and to put back what Windows
/// removed. It names nothing: the service repairs only the entries of its own installation.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct Repair {
    /// Always [`REPAIR_ACCESS`]; a fixed word keeps the frame shape distinct and versioned.
    pub repair: String,
}

/// The one thing a repair request may ask for.
pub(crate) const REPAIR_ACCESS: &str = "windows-access-v1";

impl Repair {
    /// The only valid request.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub(crate) fn access() -> Self { Self { repair: REPAIR_ACCESS.to_owned() } }

    /// Binds the request to a fresh server challenge.
    #[cfg_attr(not(windows), allow(dead_code))]
    fn sign(self, config: &Config, challenge: &str) -> io::Result<SignedRepair> {
        let proof = repair_mac(config, challenge, &self)?.finalize().into_bytes().to_vec();
        Ok(SignedRepair { request: self, proof })
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SignedRepair { request: Repair, proof: Vec<u8> }
impl SignedRepair {
    /// Verifies the request in constant time before the service touches any access list.
    #[cfg_attr(not(windows), allow(dead_code))]
    fn verify(&self, config: &Config, challenge: &str) -> io::Result<()> {
        repair_mac(config, challenge, &self.request)?.verify_slice(&self.proof)
            .map_err(|_| io::ErrorKind::PermissionDenied.into())
    }
}

/// Keeps repair requests in a MAC domain separate from leases, attestations and service proofs.
fn repair_mac(config: &Config, challenge: &str, repair: &Repair) -> io::Result<Hmac<Sha256>> {
    if repair.repair != REPAIR_ACCESS { return Err(io::ErrorKind::InvalidInput.into()); }
    if challenge.is_empty() || challenge.len() > 128 { return Err(io::ErrorKind::InvalidInput.into()); }
    let mut mac = Hmac::<Sha256>::new_from_slice(&config.key()?).map_err(io::Error::other)?;
    mac.update(b"MiniClaw Windows access repair v1\0");
    mac.update(challenge.as_bytes());
    mac.update(&[0]);
    mac.update(&serde_json::to_vec(repair)?);
    Ok(mac)
}

/// The one frame a client sends after Hello: a lease request, a job-death question, or a request
/// to repair the service's Windows access entries.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Deserialize)]
#[serde(untagged)]
enum Frame { Lease(Signed), Attest(SignedAttest), Repair(SignedRepair) }

/// The service's whole answer to a job-death question: one boolean.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Attested { ok: bool, dead: bool }

/// Keeps job-death questions in a MAC domain separate from leases and service proofs.
fn attest_mac(config: &Config, challenge: &str, attest: &Attest) -> io::Result<Hmac<Sha256>> {
    attest.validate()?;
    if challenge.is_empty() || challenge.len() > 128 { return Err(io::ErrorKind::InvalidInput.into()); }
    let mut mac = Hmac::<Sha256>::new_from_slice(&config.key()?).map_err(io::Error::other)?;
    mac.update(b"MiniClaw Windows job attestation v1\0");
    mac.update(challenge.as_bytes());
    mac.update(&[0]);
    mac.update(&serde_json::to_vec(attest)?);
    Ok(mac)
}

/// Shown when a sandboxed program declares a port to listen on.
pub const LISTEN_UNSUPPORTED: &str = "Listening ports for sandboxed programs are not supported on Windows yet. Remove the listening ports, or run the program without the sandbox.";

/// Reads the service's answer to a lease request. A refusal carries the reason the run gets no
/// network, which is shown to the user; anything else is a plain denial.
#[cfg_attr(not(windows), allow(dead_code))]
fn lease_answer(bytes: &[u8]) -> io::Result<()> {
    let answer: serde_json::Value = serde_json::from_slice(bytes)?;
    if answer["ok"] == true {
        return Ok(());
    }
    match answer["error"].as_str() {
        Some(reason) if !reason.is_empty() => {
            let reason: String = reason.chars().filter(|c| !c.is_control()).take(600).collect();
            Err(io::Error::new(io::ErrorKind::PermissionDenied, reason))
        }
        _ => Err(io::ErrorKind::PermissionDenied.into()),
    }
}

/// Service facts recorded by the SCM; only administrators can change a default-secured registration.
#[cfg_attr(not(windows), allow(dead_code))]
struct Registration { kind: u32, state: u32, pid: u32, account: String, command: String }

// Documented winsvc.h values, kept here so the decision is testable on every host.
const SERVICE_WIN32_OWN_PROCESS: u32 = 0x10;
const SERVICE_RUNNING: u32 = 4;

/// Binds a connected pipe instance to the installed LocalSystem service without a process handle.
///
/// The SCM started `service.pid` from `service.command`, so the image is known without opening a
/// SYSTEM process. A squatter is rejected three independent ways: its live PID differs from the
/// running service's, a non-administrator cannot create a pipe owned by SYSTEM/Administrators, and
/// the service reports RUNNING only after creating the first pipe instance, so no older instance
/// (including one whose creator PID was later reused) can coexist with a RUNNING service.
#[cfg_attr(not(windows), allow(dead_code))]
fn authenticate_server(config: &Config, pipe_pid: u32, pipe_owner: &str, service: &Registration) -> io::Result<()> {
    config.validate()?;
    let denied = || io::Error::from(io::ErrorKind::PermissionDenied);
    if !["S-1-5-18", "S-1-5-32-544"].contains(&pipe_owner) { return Err(denied()); }
    if service.kind != SERVICE_WIN32_OWN_PROCESS || !service.account.eq_ignore_ascii_case("LocalSystem") { return Err(denied()); }
    if service.state != SERVICE_RUNNING || pipe_pid == 0 || service.pid != pipe_pid { return Err(denied()); }
    // The installer registers exactly: "<service_exe>" --network-service "<protected config>".
    let prefix = format!("\"{}\" --network-service \"", config.service_exe).to_lowercase();
    let command = service.command.to_lowercase();
    let argument = command.strip_prefix(&prefix).and_then(|rest| rest.strip_suffix('"')).ok_or_else(denied)?;
    if argument.is_empty() || argument.contains('"') || config.service_exe.contains('"') { return Err(denied()); }
    Ok(())
}

/// Accepts canonical local/domain or Azure AD user identities, never service principals.
fn user_sid(value: &str) -> bool {
    let Some(tail) = value.strip_prefix("S-1-5-21-").or_else(|| value.strip_prefix("S-1-12-1-")) else { return false; };
    let parts: Vec<_> = tail.split('-').collect();
    parts.len() == 4 && parts.iter().all(|part| part.parse::<u32>().ok().is_some_and(|number| number.to_string() == *part))
}

/// Requires a small canonical TCP-port list so signatures and native filters agree exactly.
#[cfg_attr(not(windows), allow(dead_code))]
fn ports(values: &[u16]) -> bool {
    values.len() <= 8 && values.iter().all(|port| *port != 0) && values.windows(2).all(|pair| pair[0] < pair[1])
}

/// Accepts only a fixed namespace followed by a full lowercase UUID hex value.
fn identifier(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| suffix.len() == 32
        && suffix.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
}

/// Separates protocol version, fresh nonce and the exact closed request in the MAC input.
#[cfg_attr(not(windows), allow(dead_code))]
fn mac(config: &Config, challenge: &str, request: &Request) -> io::Result<Hmac<Sha256>> {
    if challenge.is_empty() || challenge.len() > 128 { return Err(io::ErrorKind::InvalidInput.into()); }
    let mut mac = Hmac::<Sha256>::new_from_slice(&config.key()?).map_err(io::Error::other)?;
    mac.update(b"MiniClaw Windows network lease v1\0");
    mac.update(challenge.as_bytes());
    mac.update(&[0]);
    mac.update(&serde_json::to_vec(request)?);
    Ok(mac)
}

#[cfg(windows)]
#[path = "windows_network_broker_native.rs"]
mod native;
#[cfg(windows)]
pub use native::{ClientLease, available, connect, connect_ports, readiness, service, verified_boot_epoch, winsock_capability, winsock_health};
#[cfg(windows)]
pub(crate) use native::{attest_job_dead, current_user_sid, recovery_job};
#[cfg(windows)]
pub use native::{repair_access, winsock_status};
#[cfg(all(windows, test))]
pub(crate) use native::attest_within;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_extra_privileged_operations_and_replayed_proofs() {
        let config = Config { owner_sid: "S-1-5-21-1-2-3-1001".into(), pipe_name: format!("MiniClawNetwork-{}", "a".repeat(32)),
            service_name: format!("MiniClawNetwork-{}", "a".repeat(32)), secret_hex: "12".repeat(32), service_exe: "C:\\Program Files\\MiniClaw\\network.exe".into() };
        assert!(config.validate().is_ok());
        let request = Request { profile: format!("MiniClaw.Run.{}", "b".repeat(32)), job_name: format!("Global\\MiniClaw.Job.{}", "c".repeat(32)), job_handle: 123, proxy_port: 8123, connect_ports: Vec::new(), listen_ports: Vec::new() };
        let signed = request.sign(&config, "challenge-one").unwrap();
        assert!(signed.verify(&config, "challenge-one").is_ok());
        assert!(signed.verify(&config, "challenge-two").is_err());
        let mut altered = signed;
        altered.request.proxy_port = 9999;
        assert!(altered.verify(&config, "challenge-one").is_err());
        assert!(serde_json::from_str::<Request>(r#"{"profile":"x","job_name":"x","job_handle":1,"proxy_port":1,"command":"cmd.exe"}"#).is_err());
    }

    #[test]
    fn reboot_proof_requires_same_installation_and_changed_authenticated_epoch() {
        let saved = BootEpoch { installation: format!("MiniClawNetwork-{}", "a".repeat(32)), epoch: "b".repeat(32) };
        assert!(!saved.supersedes(&saved));
        let mut current = saved.clone();
        current.epoch = "c".repeat(32);
        assert!(current.supersedes(&saved));
        current.installation = format!("MiniClawNetwork-{}", "d".repeat(32));
        assert!(!current.supersedes(&saved));
        current.installation = saved.installation.clone();
        current.epoch = "malformed".into();
        assert!(!current.supersedes(&saved));
    }

    #[test]
    fn authenticates_server_challenge_with_separate_key_domain() {
        let config = Config { owner_sid: "S-1-5-21-1-2-3-1001".into(), pipe_name: format!("MiniClawNetwork-{}", "a".repeat(32)),
            service_name: format!("MiniClawNetwork-{}", "a".repeat(32)), secret_hex: "12".repeat(32), service_exe: "service.exe".into() };
        let boot = BootEpoch { installation: config.service_name.clone(), epoch: "a".repeat(32) };
        let mut hello = Hello::new(&config, "fresh-challenge".into(), boot.clone()).unwrap();
        hello.boot.epoch = "b".repeat(32);
        assert!(hello.verify(&config).is_err());
        hello.boot = boot.clone();
        assert!(hello.verify(&config).is_ok());
        let mut wrong = config.clone();
        wrong.secret_hex = "34".repeat(32);
        assert!(hello.verify(&wrong).is_err());
        hello.challenge.push('x');
        assert!(hello.verify(&config).is_err());
        assert!(Hello::new(&config, String::new(), boot).is_err());
    }

    /// Installed-service facts used by the handle-free server authentication regressions.
    fn installed() -> (Config, Registration) {
        let suffix = "a".repeat(32);
        let exe = format!("C:\\Program Files\\MiniClawNetwork-{suffix}\\miniclaw-network-service.exe");
        let config = Config { owner_sid: "S-1-5-21-1-2-3-1001".into(), pipe_name: format!("MiniClawNetwork-{suffix}"),
            service_name: format!("MiniClawNetwork-{suffix}"), secret_hex: "12".repeat(32), service_exe: exe.clone() };
        let command = format!("\"{exe}\" --network-service \"C:\\ProgramData\\MiniClawNetwork-{suffix}\\service.json\"");
        (config, Registration { kind: 0x10, state: 4, pid: 4321, account: "LocalSystem".into(), command })
    }

    #[test]
    fn standard_user_authenticates_installed_service_without_process_handle() {
        let (config, service) = installed();
        // SCM facts plus kernel pipe metadata are sufficient; no SYSTEM process handle is involved.
        assert!(authenticate_server(&config, 4321, "S-1-5-18", &service).is_ok());
        assert!(authenticate_server(&config, 4321, "S-1-5-32-544", &service).is_ok());
        let mut cased = installed().1;
        cased.command = cased.command.replace("C:\\Program Files", "c:\\PROGRAM FILES");
        cased.account = "localsystem".into();
        assert!(authenticate_server(&config, 4321, "S-1-5-18", &cased).is_ok());
    }

    #[test]
    fn rejects_pipe_squatters_and_altered_service_registrations() {
        let (config, service) = installed();
        // A same-user squatter has another live PID and cannot own the pipe as SYSTEM/Administrators.
        assert!(authenticate_server(&config, 9999, "S-1-5-18", &service).is_err());
        assert!(authenticate_server(&config, 4321, &config.owner_sid, &service).is_err());
        assert!(authenticate_server(&config, 4321, "S-1-1-0", &service).is_err());
        assert!(authenticate_server(&config, 0, "S-1-5-18", &Registration { pid: 0, ..installed().1 }).is_err());
        // Stopped, starting and stopping services have not proven first-instance pipe ownership.
        for state in [1, 2, 3, 5, 6, 7] {
            assert!(authenticate_server(&config, 4321, "S-1-5-18", &Registration { state, ..installed().1 }).is_err(), "{state}");
        }
        // A shared or interactive host process is not exclusively this service's image.
        for kind in [0x20, 0x110, 0x30, 0x1, 0x50] {
            assert!(authenticate_server(&config, 4321, "S-1-5-18", &Registration { kind, ..installed().1 }).is_err(), "{kind}");
        }
        for account in ["NT AUTHORITY\\LocalService", ".\\owner", ""] {
            assert!(authenticate_server(&config, 4321, "S-1-5-18", &Registration { account: account.into(), ..installed().1 }).is_err(), "{account}");
        }
        let exe = &config.service_exe;
        for command in [format!("{exe} --network-service \"C:\\service.json\""),
            format!("\"C:\\Users\\owner\\evil.exe\" --network-service \"C:\\service.json\""),
            format!("\"{exe}\" --other \"C:\\service.json\""),
            format!("\"{exe}\" --network-service \"C:\\service.json\" \"extra\""),
            format!("\"{exe}\" --network-service \"\""),
            format!("\"{exe}\" --network-service C:\\service.json"),
            format!("\"{exe}\""), String::new()] {
            assert!(authenticate_server(&config, 4321, "S-1-5-18", &Registration { command: command.clone(), ..installed().1 }).is_err(), "{command}");
        }
    }

    #[test]
    fn accepts_canonical_user_sids_including_azuread_only() {
        let mut config = Config { owner_sid: "S-1-12-1-1-2-3-4".into(), pipe_name: format!("MiniClawNetwork-{}", "a".repeat(32)),
            service_name: format!("MiniClawNetwork-{}", "a".repeat(32)), secret_hex: "12".repeat(32), service_exe: "C:\\Program Files\\MiniClaw\\network.exe".into() };
        assert!(config.validate().is_ok());
        for sid in ["S-1-5-18", "S-1-5-21-1-2-3", "S-1-12-1-1-2-3-4294967296", "S-1-12-1-01-2-3-4", "S-1-12-1-1-2-3-4-5"] {
            config.owner_sid = sid.into();
            assert!(config.validate().is_err(), "{sid}");
        }
    }

    #[test]
    fn network_ports_are_explicit_bounded_and_signed() {
        let mut request = Request { profile: format!("MiniClaw.Run.{}", "b".repeat(32)), job_name: format!("Global\\MiniClaw.Job.{}", "c".repeat(32)), job_handle: 1, proxy_port: 8123,
            connect_ports: vec![3000], listen_ports: vec![9876] };
        assert!(request.validate().is_ok());
        request.listen_ports = vec![9876, 9876];
        assert!(request.validate().is_err());
        request.listen_ports = vec![0];
        assert!(request.validate().is_err());
        request.listen_ports = (1..=9).collect();
        assert!(request.validate().is_err());
    }

    /// Shared fixture for the job-death question tests.
    fn attest_fixture() -> (Config, Attest) {
        let config = Config { owner_sid: "S-1-5-21-1-2-3-1001".into(), pipe_name: format!("MiniClawNetwork-{}", "a".repeat(32)),
            service_name: format!("MiniClawNetwork-{}", "a".repeat(32)), secret_hex: "12".repeat(32), service_exe: "service.exe".into() };
        let attest = Attest { profile: format!("MiniClaw.Run.{}", "b".repeat(32)), job_name: format!("Global\\MiniClaw.Job.{}", "c".repeat(32)) };
        (config, attest)
    }

    #[test]
    fn attestation_is_bound_to_challenge_names_and_key() {
        let (config, attest) = attest_fixture();
        let signed = attest.clone().sign(&config, "challenge-one").unwrap();
        assert!(signed.verify(&config, "challenge-one").is_ok());
        assert!(signed.verify(&config, "challenge-two").is_err(), "A proof must not replay on another connection");
        let mut other_key = config.clone();
        other_key.secret_hex = "34".repeat(32);
        assert!(signed.verify(&other_key, "challenge-one").is_err());
        for altered in [Attest { profile: format!("MiniClaw.Run.{}", "d".repeat(32)), ..attest.clone() },
            Attest { job_name: format!("Global\\MiniClaw.Job.{}", "d".repeat(32)), ..attest.clone() }] {
            let forged = SignedAttest { attest: altered, proof: signed.proof.clone() };
            assert!(forged.verify(&config, "challenge-one").is_err());
        }
        // The documented MAC input: domain, challenge, zero byte, then the JSON of the names.
        let mut mac = Hmac::<Sha256>::new_from_slice(&[0x12; 32]).unwrap();
        mac.update(b"MiniClaw Windows job attestation v1\0challenge-one\0");
        mac.update(format!(r#"{{"profile":"MiniClaw.Run.{}","job_name":"Global\\MiniClaw.Job.{}"}}"#, "b".repeat(32), "c".repeat(32)).as_bytes());
        assert_eq!(signed.proof, mac.finalize().into_bytes().to_vec());
    }

    #[test]
    fn attestation_and_lease_proofs_are_not_interchangeable() {
        let (config, attest) = attest_fixture();
        let request = Request { profile: attest.profile.clone(), job_name: attest.job_name.clone(), job_handle: 1, proxy_port: 8123,
            connect_ports: Vec::new(), listen_ports: Vec::new() };
        let lease = request.sign(&config, "challenge").unwrap();
        let question = attest.clone().sign(&config, "challenge").unwrap();
        assert_ne!(lease.proof, question.proof);
        assert!(SignedAttest { attest: attest.clone(), proof: lease.proof.clone() }.verify(&config, "challenge").is_err());
        assert!(Signed { request: lease.request, proof: question.proof }.verify(&config, "challenge").is_err());
        let boot = BootEpoch { installation: config.service_name.clone(), epoch: "a".repeat(32) };
        let hello = Hello::new(&config, "challenge".into(), boot).unwrap();
        assert!(SignedAttest { attest, proof: hello.server_proof }.verify(&config, "challenge").is_err());
    }

    #[test]
    fn frame_is_exactly_one_lease_or_one_attestation() {
        let (config, attest) = attest_fixture();
        let question = serde_json::to_string(&attest.clone().sign(&config, "challenge").unwrap()).unwrap();
        assert!(question.starts_with(r#"{"attest":{"profile":"MiniClaw.Run."#), "{question}");
        assert!(matches!(serde_json::from_str::<Frame>(&question).unwrap(), Frame::Attest(signed) if signed.attest == attest));
        let request = Request { profile: attest.profile.clone(), job_name: attest.job_name.clone(), job_handle: 1, proxy_port: 8123,
            connect_ports: Vec::new(), listen_ports: Vec::new() };
        let lease = serde_json::to_string(&request.sign(&config, "challenge").unwrap()).unwrap();
        assert!(matches!(serde_json::from_str::<Frame>(&lease).unwrap(), Frame::Lease(_)));
        let names = format!(r#"{{"profile":"{}","job_name":"Global\\MiniClaw.Job.{}"}}"#, attest.profile, "c".repeat(32));
        for frame in [format!(r#"{{"attest":{names},"proof":[1],"request":{{}}}}"#),
            format!(r#"{{"attest":{names},"proof":[1],"command":"cmd.exe"}}"#),
            format!(r#"{{"attest":{{"profile":"{}","job_name":"x","pid":4}},"proof":[1]}}"#, attest.profile),
            format!(r#"{{"attest":{names}}}"#), format!(r#"{{"attest":{names},"proof":"AQ=="}}"#), "{}".to_owned(), "[]".to_owned()] {
            assert!(serde_json::from_str::<Frame>(&frame).is_err(), "{frame}");
        }
    }

    #[test]
    fn repair_requests_are_bound_and_kept_apart_from_other_proofs() {
        let (config, attest) = attest_fixture();
        let signed = Repair::access().sign(&config, "challenge").unwrap();
        assert!(signed.verify(&config, "challenge").is_ok());
        assert!(signed.verify(&config, "another").is_err(), "A proof must not replay on another connection");
        let mut other_key = config.clone();
        other_key.secret_hex = "34".repeat(32);
        assert!(signed.verify(&other_key, "challenge").is_err());
        assert!(Repair { repair: "registry".into() }.sign(&config, "challenge").is_err());
        assert!(SignedRepair { request: Repair { repair: "registry".into() }, proof: signed.proof.clone() }.verify(&config, "challenge").is_err());
        let question = attest.clone().sign(&config, "challenge").unwrap();
        assert!(SignedRepair { request: Repair::access(), proof: question.proof.clone() }.verify(&config, "challenge").is_err());
        assert!(SignedAttest { attest, proof: signed.proof.clone() }.verify(&config, "challenge").is_err());
        let text = serde_json::to_string(&signed).unwrap();
        assert!(text.starts_with(r#"{"request":{"repair":"windows-access-v1"}"#), "{text}");
        assert!(matches!(serde_json::from_str::<Frame>(&text).unwrap(), Frame::Repair(_)));
        for frame in [r#"{"request":{"repair":"windows-access-v1","key":"HKLM"},"proof":[1]}"#, r#"{"request":{},"proof":[1]}"#,
            r#"{"request":{"repair":"windows-access-v1"}}"#] {
            assert!(serde_json::from_str::<Frame>(frame).is_err() || !matches!(serde_json::from_str::<Frame>(frame).unwrap(), Frame::Repair(_)), "{frame}");
        }
    }

    #[test]
    fn attestation_accepts_only_generated_names() {
        let (config, good) = attest_fixture();
        assert!(good.validate().is_ok());
        let hex = "b".repeat(32);
        for profile in ["MiniClaw.Run.".to_owned(), format!("MiniClaw.Run.{}", "B".repeat(32)), format!("MiniClaw.Run.{hex}0"),
            format!("MiniClaw.Run.{}", "b".repeat(31)), format!("miniclaw.run.{hex}"), format!("Other.Run.{hex}"),
            format!("MiniClaw.Run.{}\0", "b".repeat(31)), format!(" MiniClaw.Run.{hex}"), format!("MiniClaw.Run.{}g", "b".repeat(31)),
            format!("MiniClaw.Run.{}\\", "b".repeat(31))] {
            let attest = Attest { profile: profile.clone(), ..good.clone() };
            assert!(attest.validate().is_err(), "{profile:?}");
            assert!(attest.sign(&config, "challenge").is_err(), "{profile:?}");
        }
        for job in [format!("MiniClaw.Job.{hex}"), format!("Local\\MiniClaw.Job.{hex}"), format!("Global\\MiniClaw.Job.{}", "C".repeat(32)),
            format!("Global\\MiniClaw.Job.{hex}\\x"), format!("Global\\Other.{hex}"), format!("Session\\1\\MiniClaw.Job.{hex}"),
            format!("Global\\MiniClaw.Job.{}", "c".repeat(33)), format!("Global/MiniClaw.Job.{hex}"), String::new()] {
            let attest = Attest { job_name: job.clone(), ..good.clone() };
            assert!(attest.validate().is_err(), "{job:?}");
            // A stored proof for a malformed name is refused before any MAC comparison.
            assert!(SignedAttest { attest, proof: vec![0; 32] }.verify(&config, "challenge").is_err(), "{job:?}");
        }
    }

    #[test]
    fn attestation_answer_is_one_strict_boolean() {
        assert_eq!(serde_json::to_string(&Attested { ok: true, dead: false }).unwrap(), r#"{"ok":true,"dead":false}"#);
        assert!(serde_json::from_str::<Attested>(r#"{"ok":true,"dead":true}"#).unwrap().dead);
        for answer in [r#"{"ok":true}"#, r#"{"dead":true}"#, r#"{"ok":true,"dead":1}"#, r#"{"ok":true,"dead":true,"pid":4}"#, r#"{"ok":true,"dead":"true"}"#] {
            assert!(serde_json::from_str::<Attested>(answer).is_err(), "{answer}");
        }
    }

    #[test]
    fn lease_refusal_reaches_the_caller_with_its_reason() {
        assert!(lease_answer(br#"{"ok":true}"#).is_ok());
        let refused = lease_answer(br#"{"ok":false,"error":"Network access for sandboxed commands is unavailable: catalog\r\nkey"}"#).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::PermissionDenied);
        assert_eq!(refused.to_string(), "Network access for sandboxed commands is unavailable: catalogkey");
        let long = format!(r#"{{"ok":false,"error":"{}"}}"#, "x".repeat(5000));
        assert_eq!(lease_answer(long.as_bytes()).unwrap_err().to_string().len(), 600);
        for answer in [&br#"{"ok":false}"#[..], br#"{"ok":"true"}"#, br#"{"ok":1}"#, br#"{"error":"x"}"#, br#"{"ok":false,"error":""}"#, br#"{"ok":false,"error":7}"#, b"{}"] {
            let error = lease_answer(answer).unwrap_err();
            assert_eq!(error.kind(), io::ErrorKind::PermissionDenied, "{}", String::from_utf8_lossy(answer));
        }
        assert!(lease_answer(b"not json").is_err());
    }

    #[test]
    fn rejects_ambiguous_profile_pipe_and_job_names() {
        for profile in ["Other.Profile", "MiniClaw.Run...", "MiniClaw.Run.0123\\other"] {
            let request = Request { profile: profile.into(), job_name: format!("Global\\MiniClaw.Job.{}", "c".repeat(32)), job_handle: 1, proxy_port: 1, connect_ports: Vec::new(), listen_ports: Vec::new() };
            assert!(request.validate().is_err());
        }
    }
}
