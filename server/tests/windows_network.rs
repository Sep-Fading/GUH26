#![cfg(windows)]
// Verifies elevated persistent filters against disposable profiles.
use miniclaw_server::{
    platform_fs, seatbelt::Grant, windows_network::Lease, windows_sandbox::Sandbox,
};
// Builds isolated files without user credential access.
use std::fs;

#[test]
#[ignore = "requires elevated Windows networking service test account"]
/// Dropping a lease preserves enforcement until an explicit post-job revocation.
fn persistent_lease_cleanup() {
    let path = std::env::temp_dir().join(format!("mc-network-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&path, false).unwrap();
    let grants = vec![Grant {
        path: path.to_string_lossy().into_owned(),
        write: true,
        folder: true,
        blocked: false,
    }];
    let sandbox = Sandbox::new(&grants, &path).unwrap();
    let mut lease = Lease::install(&sandbox.package_sid().unwrap(), 45678).unwrap();
    assert!(lease.installed().unwrap());
    let record = lease.record();
    drop(lease);
    lease = Lease::recover(record).unwrap();
    assert!(lease.installed().unwrap());
    lease.revoke().unwrap();
    assert!(!lease.installed().unwrap());
    drop(sandbox);
    // Cleanup runs on its own thread and still holds the folder until it finishes.
    miniclaw_server::windows_sandbox::drain_cleanup(std::time::Duration::from_secs(35)).unwrap();
    fs::remove_dir_all(path).unwrap();
}

/// What one networked child run produced, with the lease already released.
struct Outcome { output: String, errors: String, status: u32 }

/// Runs one child test of this binary the way production runs a networked command: a fresh
/// sandbox with the two network capabilities, a lease from the installed service, then resume.
async fn networked(child: &str, work: &std::path::Path, extra: &[(&str, String)], proxy_port: u16, listen: &[u16]) -> std::io::Result<Outcome> {
    let program = std::env::current_exe().unwrap();
    let grants = vec![
        Grant { path: work.to_string_lossy().into_owned(), write: true, folder: true, blocked: false },
        Grant { path: program.to_string_lossy().into_owned(), write: false, folder: false, blocked: false },
    ];
    let mut sandbox = Sandbox::new(&grants, work)?;
    sandbox.enable_network()?;
    let security = sandbox.security();
    assert_eq!(security.capabilities.CapabilityCount, 2, "A networked run carries internetClient and the Winsock capability only");
    let mut env: Vec<_> = sandbox.environment().into_iter()
        .map(|(key, value)| (key.into_string().unwrap(), value.into_string().unwrap())).collect();
    env.extend(extra.iter().map(|(key, value)| ((*key).to_owned(), value.clone())));
    let args = ["--exact", child, "--nocapture"].map(str::to_owned);
    let pending = miniclaw_server::windows_process::pending_stdio_restricted(&program, &args, work, &env, &security)?;
    sandbox.bind_process(&pending.server.process)?;
    let lease = miniclaw_server::windows_network_broker::connect_ports(&pending.server.process, &sandbox.profile_name(), proxy_port, &[], listen).await;
    let result = match lease {
        Ok(lease) => {
            let tree = pending.server.process.tree_guard();
            let captured = tokio::time::timeout(std::time::Duration::from_secs(60),
                miniclaw_server::windows_process::capture_stdio(pending.resume()?)).await;
            tree.terminate_and_wait(std::time::Duration::from_secs(10))?;
            drop(lease);
            captured.map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut)).and_then(|result| result)
        }
        Err(error) => { drop(pending); Err(error) }
    };
    drop(sandbox);
    miniclaw_server::windows_sandbox::drain_cleanup(std::time::Duration::from_secs(35))?;
    let (output, errors, status) = result?;
    Ok(Outcome { output: String::from_utf8_lossy(&output).into_owned(), errors: String::from_utf8_lossy(&errors).into_owned(), status })
}

/// A private working folder for one test.
fn workspace(label: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("mc-network-{label}-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&path, false).unwrap();
    path
}

#[tokio::test]
#[ignore = "requires the installed Windows network service"]
/// The LPAC reaches its exact proxy socket while a second live loopback listener is denied.
async fn exact_proxy_only() {
    let path = workspace("sockets");
    let allowed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let denied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = allowed.local_addr().unwrap().port();
    let blocked_port = denied.local_addr().unwrap().port();
    let run = networked("socket_child", &path, &[("MINICLAW_SOCKET_FIXTURE", format!("connect:{port}:{blocked_port}"))], port, &[]).await.unwrap();
    assert_eq!(run.status, 0, "{}\n{}", run.output, run.errors);
    assert!(run.output.contains("proxy-only"));
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(1), allowed.accept())
            .await
            .is_ok()
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(200), denied.accept())
            .await
            .is_err()
    );
    fs::remove_dir_all(path).unwrap();
}

/// The installed service's Winsock manager, for read-only inspection of the real keys.
fn installed_manager() -> (miniclaw_server::windows_winsock_acl::Manager, String) {
    let config: serde_json::Value = serde_json::from_slice(&fs::read(std::env::var_os("MINICLAW_NETWORK_CONFIG").unwrap()).unwrap()).unwrap();
    let service = config["service_name"].as_str().unwrap().to_owned();
    let state = std::env::temp_dir().join(format!("mc-winsock-inspect-{}", uuid::Uuid::new_v4().simple()));
    (miniclaw_server::windows_winsock_acl::Manager::machine(&service, &state).unwrap(), service)
}

#[tokio::test]
#[ignore = "requires the installed Windows network service"]
/// A networked run can use Winsock and its proxy, and nothing the two capabilities might seem to
/// open: no user or machine registry data, no key listing, no outside file, no other endpoint.
async fn networked_run_stays_closed_elsewhere() {
    use std::net::TcpListener;
    let root = workspace("negative");
    let work = root.join("work");
    fs::create_dir(&work).unwrap();
    let outside = root.join("outside-secret");
    fs::write(&outside, "synthetic-file-secret").unwrap();
    let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
    let second = TcpListener::bind("127.0.0.1:0").unwrap();
    let other = TcpListener::bind("127.0.0.2:0").unwrap();
    let ipv6 = TcpListener::bind("[::1]:0").unwrap();
    // Live receivers plus an unsandboxed connection each rule out an unavailable service.
    for receiver in [&proxy, &second, &other, &ipv6] {
        let control = std::net::TcpStream::connect_timeout(&receiver.local_addr().unwrap(), std::time::Duration::from_secs(2)).unwrap();
        drop(receiver.accept().unwrap());
        drop(control);
        receiver.set_nonblocking(true).unwrap();
    }
    let port = proxy.local_addr().unwrap().port();
    // Datagram receivers on loopback and on this machine's own network address. An unsandboxed
    // datagram to each proves they work.
    let local = std::net::UdpSocket::bind("0.0.0.0:0").and_then(|socket| { socket.connect("192.0.2.1:9")?; socket.local_addr() }).unwrap().ip();
    let receivers: Vec<std::net::UdpSocket> = [std::net::IpAddr::from([127, 0, 0, 1]), local].into_iter()
        .map(|address| std::net::UdpSocket::bind((address, 0)).unwrap()).collect();
    let mut datagram = [0u8; 64];
    for receiver in &receivers {
        receiver.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
        std::net::UdpSocket::bind("0.0.0.0:0").unwrap().send_to(b"control", receiver.local_addr().unwrap()).unwrap();
        assert_eq!(receiver.recv(&mut datagram).unwrap(), 7, "The datagram receiver on {} does not work", receiver.local_addr().unwrap());
        receiver.set_read_timeout(Some(std::time::Duration::from_millis(300))).unwrap();
    }
    let udp = receivers.iter().map(|receiver| receiver.local_addr().unwrap().to_string()).chain(Some("1.1.1.1:53".to_owned())).collect::<Vec<_>>().join(",");
    println!("UDP receivers {udp}");
    let (manager, _) = installed_manager();
    let ports = format!("{port}:{}:{}:{}", second.local_addr().unwrap().port(), other.local_addr().unwrap().port(), ipv6.local_addr().unwrap().port());
    let run = networked("networked_child", &work, &[("MINICLAW_NETWORKED_FIXTURE", ports), ("MINICLAW_NETWORKED_SID", manager.sid().to_owned()), ("MINICLAW_NETWORKED_UDP", udp),
        ("MINICLAW_NETWORKED_OUTSIDE", outside.to_string_lossy().into_owned()),
        ("MINICLAW_NETWORKED_PROTECTED", std::env::var("MINICLAW_NETWORK_CONFIG").unwrap())], port, &[]).await.unwrap();
    println!("{}\n{}", run.output, run.errors);
    assert_eq!(run.status, 0);
    assert!(run.output.contains("WSAStartup=0") && run.output.contains("PROXY reachable=true"), "Winsock did not work in the production token");
    assert!(run.output.contains("NEG complete"));
    assert!(!run.output.contains("EXPOSED"), "A negative control was exposed");
    std::thread::sleep(std::time::Duration::from_millis(200));
    assert!(proxy.accept().is_ok(), "The approved proxy saw no connection");
    for (name, receiver) in [("second loopback port", &second), ("127.0.0.2", &other), ("::1", &ipv6)] {
        assert!(receiver.accept().is_err(), "{name} received a connection");
    }
    for receiver in &receivers {
        let arrived = receiver.recv(&mut datagram).is_ok();
        println!("UDP receiver {} got a sandboxed datagram: {arrived}", receiver.local_addr().unwrap());
        assert!(!arrived, "A datagram left the sandbox to {}", receiver.local_addr().unwrap());
    }
    assert_eq!(fs::read_to_string(&outside).unwrap(), "synthetic-file-secret");
    fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
#[ignore = "requires the installed Windows network service and an elevated token"]
/// Windows recreates catalog keys without our entry (catalog reset, VPN installer, servicing).
/// The service puts the entry back before it grants the next lease, and records the repair.
async fn dropped_catalog_entry_is_repaired_before_the_lease() {
    use miniclaw_server::windows_winsock_acl::{self as acl, Presence};
    use windows_sys::Win32::{Security::DACL_SECURITY_INFORMATION, System::Registry::*};
    let (manager, service) = installed_manager();
    let states = manager.inspect().unwrap();
    assert!(states.iter().all(|state| state.presence == Presence::Exact), "The service start must have installed every entry");
    let victim = states.iter().find(|state| state.path.ends_with(r"Protocol_Catalog9\Catalog_Entries64\000000000001")).expect("first protocol entry");
    // Simulate the loss on the real key: delete only this installation's entry, as a reset would.
    let parsed = acl::descriptor(&victim.descriptor).unwrap();
    let stripped = acl::dacl_descriptor(parsed.control, &acl::without_sid(parsed.dacl, manager.sid_bytes()).unwrap());
    let mut aligned = vec![0u32; stripped.len().div_ceil(4)];
    unsafe {
        std::ptr::copy_nonoverlapping(stripped.as_ptr(), aligned.as_mut_ptr().cast::<u8>(), stripped.len());
        let name: Vec<u16> = victim.path.encode_utf16().chain(Some(0)).collect();
        let mut key = std::ptr::null_mut();
        assert_eq!(RegOpenKeyExW(HKEY_LOCAL_MACHINE, name.as_ptr(), 0, 0x0006_0000, &mut key), 0);
        assert_eq!(RegSetKeySecurity(key, DACL_SECURITY_INFORMATION, aligned.as_mut_ptr().cast()), 0);
        RegCloseKey(key);
    }
    let missing: Vec<_> = manager.inspect().unwrap().into_iter().filter(|state| state.presence != Presence::Exact).map(|state| state.path).collect();
    assert_eq!(missing, vec![victim.path.clone()]);
    println!("REPAIR simulated loss on HKLM\\{}", victim.path);
    // The next networked run works: the service repaired the key before granting the lease.
    let path = workspace("repair");
    let allowed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let denied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = allowed.local_addr().unwrap().port();
    let run = networked("socket_child", &path, &[("MINICLAW_SOCKET_FIXTURE", format!("connect:{port}:{}", denied.local_addr().unwrap().port()))], port, &[]).await.unwrap();
    assert_eq!(run.status, 0, "{}\n{}", run.output, run.errors);
    let after = manager.inspect().unwrap();
    assert!(after.iter().all(|state| state.presence == Presence::Exact));
    assert_eq!(after.iter().find(|state| state.path == victim.path).unwrap().descriptor, victim.descriptor, "The repaired key equals its state before the loss");
    let status_path = std::path::PathBuf::from(std::env::var_os("ProgramData").unwrap()).join(&service).join("winsock-acl-status.json");
    let status: serde_json::Value = serde_json::from_slice(&fs::read(&status_path).unwrap()).unwrap();
    println!("REPAIR status {status}");
    assert_eq!(status["ok"], true);
    assert_eq!(status["repaired"], serde_json::json!([victim.path]));
    fs::remove_dir_all(path).unwrap();
}

/// Opens one real catalog key with the rights to change its access list. `backup` uses the
/// restore privilege, which reaches a key whose list no longer grants administrators that right.
fn writable_key(path: &str, backup: bool) -> windows_sys::Win32::System::Registry::HKEY {
    use windows_sys::Win32::System::Registry::*;
    let name: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let mut key = std::ptr::null_mut();
    let status = if backup {
        enable_privilege("SeRestorePrivilege");
        // REG_OPTION_BACKUP_RESTORE ignores the requested access and asks for restore rights.
        unsafe { RegCreateKeyExW(HKEY_LOCAL_MACHINE, name.as_ptr(), 0, std::ptr::null(), REG_OPTION_BACKUP_RESTORE, 0, std::ptr::null(), &mut key, std::ptr::null_mut()) }
    } else {
        unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, name.as_ptr(), 0, 0x0006_0000, &mut key) }
    };
    assert_eq!(status, 0, "open HKLM\\{path} backup={backup}");
    key
}

/// Turns on one privilege the elevated test token holds.
fn enable_privilege(name: &str) {
    use windows_sys::Win32::{Foundation::CloseHandle, Security::*, System::Threading::{GetCurrentProcess, OpenProcessToken}};
    unsafe {
        let mut token = std::ptr::null_mut();
        assert_ne!(OpenProcessToken(GetCurrentProcess(), TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY, &mut token), 0);
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let mut privileges: TOKEN_PRIVILEGES = std::mem::zeroed();
        privileges.PrivilegeCount = 1;
        privileges.Privileges[0].Attributes = SE_PRIVILEGE_ENABLED;
        assert_ne!(LookupPrivilegeValueW(std::ptr::null(), wide.as_ptr(), &mut privileges.Privileges[0].Luid), 0);
        assert_ne!(AdjustTokenPrivileges(token, 0, &privileges, 0, std::ptr::null_mut(), std::ptr::null_mut()), 0);
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(0), "{name} is held");
        CloseHandle(token);
    }
}

/// Replaces one key's access list with the descriptor's (owner and group stay).
fn set_dacl(key: windows_sys::Win32::System::Registry::HKEY, descriptor: &[u8]) {
    use windows_sys::Win32::{Security::DACL_SECURITY_INFORMATION, System::Registry::RegSetKeySecurity};
    let mut aligned = vec![0u32; descriptor.len().div_ceil(4)];
    unsafe {
        std::ptr::copy_nonoverlapping(descriptor.as_ptr(), aligned.as_mut_ptr().cast::<u8>(), descriptor.len());
        assert_eq!(RegSetKeySecurity(key, DACL_SECURITY_INFORMATION, aligned.as_mut_ptr().cast()), 0);
    }
}

/// The installed service's published access status, as the page reads it.
fn published_access() -> serde_json::Value {
    serde_json::to_value(miniclaw_server::windows_gate::access()).unwrap()
}

#[tokio::test]
#[ignore = "requires the installed Windows network service and an elevated token"]
/// "Set up again" without elevation: Windows drops an entry (a catalog reset), the page asks the
/// running service to repair, and the status says when it was repaired.
async fn access_is_set_up_again_by_the_running_service() {
    use miniclaw_server::windows_winsock_acl::{self as acl, Presence};
    let (manager, _) = installed_manager();
    assert_ne!(published_access()["state"], "failed", "the service start left working entries");
    let states = manager.inspect().unwrap();
    let victim = states.iter().find(|state| state.path.ends_with(r"NameSpace_Catalog5\Catalog_Entries64\000000000001")).expect("first namespace entry");
    let parsed = acl::descriptor(&victim.descriptor).unwrap();
    let key = writable_key(&victim.path, false);
    set_dacl(key, &acl::dacl_descriptor(parsed.control, &acl::without_sid(parsed.dacl, manager.sid_bytes()).unwrap()));
    unsafe { windows_sys::Win32::System::Registry::RegCloseKey(key); }
    assert!(manager.inspect().unwrap().iter().any(|state| state.path == victim.path && state.presence == Presence::Absent));
    println!("REPAIR simulated loss on HKLM\\{}", victim.path);
    let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    miniclaw_server::windows_network_broker::repair_access().expect("the running service repairs its own entries without elevation");
    assert!(manager.inspect().unwrap().iter().all(|state| state.presence == Presence::Exact));
    let access = published_access();
    println!("REPAIR status {access}");
    assert_eq!(access["state"], "repaired");
    assert!(access["time"].as_u64().unwrap() >= started);
    let report = serde_json::to_value(miniclaw_server::windows_gate::report()).unwrap();
    assert_eq!(report["access"]["state"], "repaired");
    assert_eq!(report["network"]["available"], true, "{report}");
    // A second request finds nothing to repair; the last repair stays on record.
    miniclaw_server::windows_network_broker::repair_access().unwrap();
    assert_eq!(published_access()["state"], "repaired");
}

#[tokio::test]
#[ignore = "requires the installed Windows network service and an elevated token"]
/// When the service cannot put an entry back (here its rights to change one key are taken away),
/// it says why, the page shows the reason, networked runs are refused before they start, and an
/// offline run still has no network. The key is then restored and the service repairs it.
async fn access_that_cannot_be_repaired_fails_closed() {
    use miniclaw_server::windows_winsock_acl::{self as acl, Presence};
    let (manager, _) = installed_manager();
    let states = manager.inspect().unwrap();
    let victim = states.iter().find(|state| state.path.ends_with(r"NameSpace_Catalog5\Catalog_Entries64\000000000001")).expect("first namespace entry");
    let original = victim.descriptor.clone();
    let parsed = acl::descriptor(&original).unwrap();
    // The entry is gone, and neither SYSTEM nor the key's owner may change the list: a deny of
    // WRITE_DAC for SYSTEM, and an OWNER RIGHTS entry that replaces the owner's implicit rights.
    let stripped = acl::without_sid(parsed.dacl, manager.sid_bytes()).unwrap();
    let mut text = String::new();
    unsafe {
        use windows_sys::Win32::{Foundation::LocalFree, Security::{Authorization::*, DACL_SECURITY_INFORMATION}};
        let blocked = acl::dacl_descriptor(parsed.control, &stripped);
        let mut aligned = vec![0u32; blocked.len().div_ceil(4)];
        std::ptr::copy_nonoverlapping(blocked.as_ptr(), aligned.as_mut_ptr().cast::<u8>(), blocked.len());
        let mut sddl = std::ptr::null_mut();
        assert_ne!(ConvertSecurityDescriptorToStringSecurityDescriptorW(aligned.as_mut_ptr().cast(), 1, DACL_SECURITY_INFORMATION, &mut sddl, std::ptr::null_mut()), 0);
        let mut length = 0;
        while *sddl.add(length) != 0 { length += 1; }
        text = String::from_utf16_lossy(std::slice::from_raw_parts(sddl, length));
        LocalFree(sddl.cast());
    }
    let body = text.strip_prefix("D:").unwrap();
    let (flags, aces) = body.split_at(body.find('(').unwrap_or(body.len()));
    let locked = format!("D:{flags}(D;;WD;;;SY)(A;;KR;;;OW){aces}");
    println!("LOCK HKLM\\{} {locked}", victim.path);
    let key = writable_key(&victim.path, false);
    unsafe {
        use windows_sys::Win32::{Foundation::LocalFree, Security::{Authorization::*, DACL_SECURITY_INFORMATION}, System::Registry::RegSetKeySecurity};
        let wide: Vec<u16> = locked.encode_utf16().chain(Some(0)).collect();
        let mut descriptor = std::ptr::null_mut();
        assert_ne!(ConvertStringSecurityDescriptorToSecurityDescriptorW(wide.as_ptr(), 1, &mut descriptor, std::ptr::null_mut()), 0);
        assert_eq!(RegSetKeySecurity(key, DACL_SECURITY_INFORMATION, descriptor), 0);
        LocalFree(descriptor);
        windows_sys::Win32::System::Registry::RegCloseKey(key);
    }
    let outcome = futures_util::FutureExt::catch_unwind(std::panic::AssertUnwindSafe(async {
        let refused = miniclaw_server::windows_network_broker::repair_access().expect_err("the service cannot change the locked key");
        println!("CANNOT REPAIR {refused}");
        assert!(refused.to_string().starts_with("Network access for sandboxed commands is unavailable"), "{refused}");
        let access = published_access();
        println!("CANNOT REPAIR status {access}");
        assert_eq!(access["state"], "failed");
        assert!(access["reason"].as_str().unwrap().contains(&victim.path), "{access}");
        let report = serde_json::to_value(miniclaw_server::windows_gate::report()).unwrap();
        assert_eq!(report["network"]["available"], false, "{report}");
        assert!(report["network"]["reason"].as_str().unwrap().contains("could not confirm"), "{report}");
        // A networked command is refused before it starts; its marker file is never written.
        let work = workspace("locked");
        let grants = vec![Grant { path: work.to_string_lossy().into_owned(), write: true, folder: true, blocked: false }];
        let result = miniclaw_server::executor::execute_host(miniclaw_server::scoped::Operation::RunShell { text: "echo started > marker".into(),
            cwd: work.to_string_lossy().into_owned(), hosts: vec!["example.com".into()] }, &grants).await;
        println!("CANNOT REPAIR networked run {result}");
        assert_eq!(result["status"], "denied", "{result}");
        assert!(result["text"].as_str().unwrap().contains("Network access for sandboxed commands is unavailable"), "{result}");
        assert!(!work.join("marker").exists(), "the networked command ran");
        // An offline command still runs, and still reaches nothing.
        let live = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        live.set_nonblocking(true).unwrap();
        let offline = miniclaw_server::executor::execute_host(miniclaw_server::scoped::Operation::RunShell {
            text: format!("echo ran > marker; nc 127.0.0.1 {} </dev/null && echo NETWORK || echo no-network", live.local_addr().unwrap().port()),
            cwd: work.to_string_lossy().into_owned(), hosts: vec![] }, &grants).await;
        println!("CANNOT REPAIR offline run {offline}");
        assert_eq!(offline["exitCode"], 0, "{offline}");
        assert!(offline["text"].as_str().unwrap().contains("no-network"), "{offline}");
        // Winsock itself no longer starts offline: one catalog key lost the entry it needs.
        println!("CANNOT REPAIR offline Winsock: {}", offline["stderr"]);
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(live.accept().is_err());
        miniclaw_server::windows_sandbox::drain_cleanup(std::time::Duration::from_secs(35)).unwrap();
        let _ = fs::remove_dir_all(work);
    }));
    let result = outcome.await;
    // Restore the key's own list byte for byte, with the restore privilege, whatever happened above.
    let key = writable_key(&victim.path, true);
    set_dacl(key, &acl::dacl_descriptor(parsed.control, parsed.dacl));
    unsafe { windows_sys::Win32::System::Registry::RegCloseKey(key); }
    let after = manager.inspect().unwrap();
    assert_eq!(after.iter().find(|state| state.path == victim.path).unwrap().descriptor, original, "the key's descriptor is restored exactly");
    miniclaw_server::windows_network_broker::repair_access().expect("the restored key is repaired again");
    assert!(manager.inspect().unwrap().iter().all(|state| state.presence == Presence::Exact));
    // The restored list already holds the entry, so the check finds nothing to repair.
    assert_eq!(published_access()["state"], "healthy");
    if let Err(panic) = result { std::panic::resume_unwind(panic); }
}

#[tokio::test]
#[ignore = "requires the installed Windows network service"]
/// A declared listening port is refused at once with a plain reason; the process never starts.
/// Windows drops inbound loopback to a sandboxed listener, so granting the lease would only hang.
async fn declared_listener_is_refused_as_unsupported() {
    let path = workspace("listener");
    let started = std::time::Instant::now();
    let error = networked("socket_child", &path, &[("MINICLAW_SOCKET_FIXTURE", "listen:45678".to_owned())], 45679, &[45678]).await.err()
        .expect("A declared listener must not receive a lease");
    assert_eq!(error.kind(), std::io::ErrorKind::Unsupported, "{error}");
    assert!(error.to_string().contains("not supported on Windows yet"), "{error}");
    assert!(started.elapsed() < std::time::Duration::from_secs(20), "The refusal must not hang");
    assert!(!path.join("positive-control").exists());
    fs::remove_dir_all(path).unwrap();
}

/// Runs socket operations in the exact AppContainer token without a shell dependency.
#[test]
fn socket_child() {
    let Ok(mode) = std::env::var("MINICLAW_SOCKET_FIXTURE") else { return };
    winsock_registry_probe();
    let mut data = unsafe { std::mem::zeroed() };
    let startup = unsafe { windows_sys::Win32::Networking::WinSock::WSAStartup(0x0202, &mut data) };
    assert_eq!(startup, 0, "WSAStartup returned its actual error code");
    let parts: Vec<_> = mode.split(':').collect();
    let port: u16 = parts[1].parse().unwrap();
    if parts[0] == "connect" {
        let blocked: u16 = parts[2].parse().unwrap();
        let address = |port| std::net::SocketAddr::from(([127, 0, 0, 1], port));
        std::net::TcpStream::connect_timeout(&address(port), std::time::Duration::from_secs(2)).unwrap();
        assert!(std::net::TcpStream::connect_timeout(&address(blocked), std::time::Duration::from_secs(2)).is_err());
        println!("proxy-only");
    } else {
        use std::io::Write;
        assert_eq!(parts[0], "listen");
        assert!(std::net::TcpListener::bind(std::net::SocketAddr::from(([0, 0, 0, 0], port))).is_err());
        assert!(std::net::TcpListener::bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0))).is_err());
        let listener = std::net::TcpListener::bind(std::net::SocketAddr::from(([127, 0, 0, 1], port))).unwrap();
        listener.set_nonblocking(true).unwrap();
        let start = std::time::Instant::now();
        loop {
            match listener.accept() {
                Ok((mut client, _)) => { client.write_all(b"declared-listener").unwrap(); break; }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && start.elapsed() < std::time::Duration::from_secs(10) => std::thread::sleep(std::time::Duration::from_millis(10)),
                Err(error) => panic!("Declared listener did not accept: {error}"),
            }
        }
    }
    unsafe { windows_sys::Win32::Networking::WinSock::WSACleanup(); }
}

/// Logs access results for known Winsock catalogs without reading their values or changing ACLs.
fn winsock_registry_probe() {
    use windows_sys::Win32::System::Registry::*;
    for name in [
        r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters",
        r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\Protocol_Catalog9",
        r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\NameSpace_Catalog5",
        r"SYSTEM\CurrentControlSet\Services\Tcpip\Parameters",
    ] {
        let wide: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        for access in [KEY_QUERY_VALUE, KEY_READ] {
            let mut key = std::ptr::null_mut();
            let status = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, wide.as_ptr(), 0, access, &mut key) };
            println!("Registry probe {name}, access={access:#x}, result={status}");
            if status == 0 { unsafe { RegCloseKey(key); } }
        }
    }
}

#[tokio::test]
#[ignore = "requires configured Windows networking service test account"]
/// A run without approved hosts carries only the installation's Winsock-read capability: Winsock
/// starts, but no socket can be created, no name resolves, and nothing else in the registry, no
/// outside file and no protected state opens. Live receivers see no traffic.
async fn offline_run_stays_closed() {
    let root = std::env::temp_dir().join(format!("mc-network-offline-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&root, false).unwrap();
    let work = root.join("work");
    fs::create_dir(&work).unwrap();
    let outside = root.join("outside-secret");
    fs::write(&outside, "synthetic-file-secret").unwrap();
    let program = std::env::current_exe().unwrap();
    let grants = vec![
        Grant { path: work.to_string_lossy().into_owned(), write: true, folder: true, blocked: false },
        Grant { path: program.to_string_lossy().into_owned(), write: false, folder: false, blocked: false },
    ];
    let (manager, _) = installed_manager();
    let sandbox = Sandbox::new(&grants, &work).unwrap();
    let security = sandbox.security();
    assert_eq!(security.capabilities.CapabilityCount, 1, "An offline token carries the Winsock-read capability only");
    assert!(security.low_privilege);
    // Live receivers: TCP on loopback, IPv6 loopback and this machine's address; datagrams on
    // loopback and this machine's address. An unsandboxed control proves each works.
    let local = std::net::UdpSocket::bind("0.0.0.0:0").and_then(|socket| { socket.connect("192.0.2.1:9")?; socket.local_addr() }).unwrap().ip();
    let listeners: Vec<std::net::TcpListener> = ["127.0.0.1:0".to_owned(), "[::1]:0".to_owned(), format!("{local}:0")].iter()
        .map(|address| std::net::TcpListener::bind(address.as_str()).unwrap()).collect();
    for listener in &listeners {
        let control = std::net::TcpStream::connect_timeout(&listener.local_addr().unwrap(), std::time::Duration::from_secs(2)).unwrap();
        drop(listener.accept().unwrap());
        drop(control);
        listener.set_nonblocking(true).unwrap();
    }
    let receivers: Vec<std::net::UdpSocket> = [std::net::IpAddr::from([127, 0, 0, 1]), local].into_iter()
        .map(|address| std::net::UdpSocket::bind((address, 0)).unwrap()).collect();
    let mut datagram = [0u8; 64];
    for receiver in &receivers {
        receiver.set_read_timeout(Some(std::time::Duration::from_secs(2))).unwrap();
        std::net::UdpSocket::bind("0.0.0.0:0").unwrap().send_to(b"control", receiver.local_addr().unwrap()).unwrap();
        assert_eq!(receiver.recv(&mut datagram).unwrap(), 7, "The datagram receiver on {} does not work", receiver.local_addr().unwrap());
        receiver.set_read_timeout(Some(std::time::Duration::from_millis(300))).unwrap();
    }
    let targets = listeners.iter().map(|listener| format!("tcp={}", listener.local_addr().unwrap()))
        .chain(receivers.iter().map(|receiver| format!("udp={}", receiver.local_addr().unwrap())))
        .chain(["tcp=1.1.1.1:443".to_owned(), "udp=1.1.1.1:53".to_owned()]).collect::<Vec<_>>().join(",");
    let args = ["--exact", "offline_child", "--nocapture"].map(str::to_owned);
    let mut env: Vec<_> = sandbox
        .environment()
        .into_iter()
        .map(|(key, value)| (key.into_string().unwrap(), value.into_string().unwrap()))
        .collect();
    env.push(("MINICLAW_OFFLINE_FIXTURE".into(), targets));
    env.push(("MINICLAW_OFFLINE_SID".into(), manager.sid().to_owned()));
    env.push(("MINICLAW_OFFLINE_OUTSIDE".into(), outside.to_string_lossy().into_owned()));
    // Protected MiniClaw state is probed by open only; no content is read or printed.
    env.push(("MINICLAW_OFFLINE_PROTECTED".into(), std::env::var("MINICLAW_NETWORK_CONFIG").unwrap()));
    let (output, errors, status) =
        miniclaw_server::windows_process::capture_restricted(&program, &args, &work, &env, &security)
            .await
            .unwrap();
    let text = String::from_utf8_lossy(&output);
    println!("{text}\n{}", String::from_utf8_lossy(&errors));
    assert_eq!(status, 0, "{text}\n{}", String::from_utf8_lossy(&errors));
    assert!(text.contains("WSAStartup=0"), "Winsock must start in the offline token: {text}");
    assert!(text.contains("NEG complete") && text.contains("offline-closed"), "{text}");
    assert!(!text.contains("EXPOSED"), "A negative control was exposed: {text}");
    std::thread::sleep(std::time::Duration::from_millis(200));
    for listener in &listeners {
        assert_eq!(listener.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock, "Offline child reached {}", listener.local_addr().unwrap());
    }
    for receiver in &receivers {
        assert!(receiver.recv(&mut datagram).is_err(), "A datagram left the offline sandbox to {}", receiver.local_addr().unwrap());
    }
    assert_eq!(fs::read_to_string(&outside).unwrap(), "synthetic-file-secret");
    drop(sandbox);
    miniclaw_server::windows_sandbox::drain_cleanup(std::time::Duration::from_secs(35)).unwrap();
    fs::remove_dir_all(root).unwrap();
}

/// Proves denial from inside the production offline token; opens handles only, never reads values.
#[test]
fn offline_child() {
    use windows_sys::Win32::{Networking::WinSock::*, Security::*, System::Registry::*};
    let Ok(targets) = std::env::var("MINICLAW_OFFLINE_FIXTURE") else { return };
    let groups = token_information(TokenCapabilities);
    let capabilities: Vec<String> = unsafe {
        let groups = groups.as_ptr().cast::<TOKEN_GROUPS>();
        (0..(*groups).GroupCount as usize).map(|index| sid_text((*(*groups).Groups.as_ptr().add(index)).Sid)).collect()
    };
    println!("TOKEN capabilities={capabilities:?}");
    assert_eq!(capabilities, vec![std::env::var("MINICLAW_OFFLINE_SID").unwrap()], "The offline token holds exactly the Winsock capability");
    let mut data = unsafe { std::mem::zeroed() };
    let startup = unsafe { WSAStartup(0x0202, &mut data) };
    println!("WSAStartup={startup}");
    crypto_probe();
    // Every socket the token could ask for is refused at creation.
    for (label, family, kind, protocol) in [("tcp4", AF_INET, SOCK_STREAM, IPPROTO_TCP), ("tcp6", AF_INET6, SOCK_STREAM, IPPROTO_TCP),
        ("udp4", AF_INET, SOCK_DGRAM, IPPROTO_UDP), ("udp6", AF_INET6, SOCK_DGRAM, IPPROTO_UDP), ("raw4", AF_INET, SOCK_RAW, IPPROTO_ICMP),
        ("unspecified", AF_INET, SOCK_STREAM, 0)] {
        let socket = unsafe { windows_sys::Win32::Networking::WinSock::socket(family as i32, kind, protocol) };
        if socket == INVALID_SOCKET { println!("NEG socket {label}: denied ({})", unsafe { WSAGetLastError() }); }
        else { unsafe { closesocket(socket); } println!("NEG socket {label}: EXPOSED"); }
    }
    for target in targets.split(',') {
        let (kind, address) = target.split_once('=').unwrap();
        let address: std::net::SocketAddr = address.parse().unwrap();
        let reached = match kind {
            "tcp" => std::net::TcpStream::connect_timeout(&address, std::time::Duration::from_secs(2)).map(|_| ()),
            _ => std::net::UdpSocket::bind(if address.is_ipv4() { "0.0.0.0:0" } else { "[::]:0" }).and_then(|socket| socket.send_to(b"miniclaw-offline", address)).map(|_| ()),
        };
        match reached { Ok(()) => println!("NEG {kind} {address}: EXPOSED"), Err(error) => println!("NEG {kind} {address}: denied ({:?})", error.raw_os_error()) }
    }
    for address in ["127.0.0.1:0", "0.0.0.0:0", "[::1]:0"] {
        match std::net::TcpListener::bind(address) {
            Ok(_) => println!("NEG listen {address}: EXPOSED"),
            Err(error) => println!("NEG listen {address}: denied ({:?})", error.raw_os_error()),
        }
    }
    for name in ["localhost", "example.com", "miniclaw-offline.invalid"] {
        match std::net::ToSocketAddrs::to_socket_addrs(&(name, 80)) {
            Ok(found) => println!("NEG resolve {name}: EXPOSED ({} addresses)", found.count()),
            Err(error) => println!("NEG resolve {name}: denied ({:?})", error.raw_os_error()),
        }
    }
    if startup == 0 { unsafe { WSACleanup(); } }
    let user = sid_text(unsafe { (*token_information(TokenUser).as_ptr().cast::<TOKEN_USER>()).User.Sid });
    for (label, path) in [("HKCU root", ""), ("HKCU Environment", "Environment"), ("HKCU Software", "SOFTWARE"),
        ("HKCU Software Classes", r"SOFTWARE\Classes"), ("HKCU Software Microsoft", r"SOFTWARE\Microsoft"), ("HKCU Volatile Environment", "Volatile Environment")] {
        let nt = if path.is_empty() { format!(r"\Registry\User\{user}") } else { format!(r"\Registry\User\{user}\{path}") };
        negative(label, HKEY_CURRENT_USER, path, &nt);
    }
    negative("HKU .DEFAULT", HKEY_USERS, ".DEFAULT", r"\Registry\User\.DEFAULT");
    for (label, path) in [("HKLM SOFTWARE", "SOFTWARE"), ("HKLM SOFTWARE Microsoft Cryptography", r"SOFTWARE\Microsoft\Cryptography"),
        ("HKLM SOFTWARE Windows NT CurrentVersion", r"SOFTWARE\Microsoft\Windows NT\CurrentVersion"),
        ("HKLM SYSTEM", "SYSTEM"), ("HKLM SYSTEM Control", r"SYSTEM\CurrentControlSet\Control"),
        ("HKLM Session Manager Environment", r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment"),
        ("HKLM Services", r"SYSTEM\CurrentControlSet\Services"),
        ("HKLM Services WinSock2", r"SYSTEM\CurrentControlSet\Services\WinSock2"), ("HKLM Services WinSock", r"SYSTEM\CurrentControlSet\Services\WinSock"),
        ("HKLM SAM", "SAM"), ("HKLM SECURITY", "SECURITY"), ("HKLM HARDWARE", "HARDWARE")] {
        negative(label, HKEY_LOCAL_MACHINE, path, &format!(r"\Registry\Machine\{path}"));
    }
    // The root key is granted query-value only: it opens, but lists nothing and reads no more.
    let empty = [0u16];
    for (label, access) in [("enumerate", KEY_ENUMERATE_SUB_KEYS), ("read", KEY_READ), ("maximum", 0x0200_0000)] {
        let mut key = std::ptr::null_mut();
        let status = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, empty.as_ptr(), 0, access, &mut key) };
        let mut listing = 5;
        if status == 0 {
            let mut name = [0u16; 256];
            let mut length = name.len() as u32;
            listing = unsafe { RegEnumKeyExW(key, 0, name.as_mut_ptr(), &mut length, std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
            unsafe { RegCloseKey(key); }
        }
        if listing == 0 { println!("NEG HKLM root {label}: EXPOSED open={status} listing={listing}"); }
        else { println!("NEG HKLM root {label}: denied (open={status} listing={listing})"); }
    }
    for path in [r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\MiniClawWriteProbe", r"SOFTWARE\MiniClawWriteProbe"] {
        let name: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        let mut created = std::ptr::null_mut();
        let create = unsafe { RegCreateKeyExW(HKEY_LOCAL_MACHINE, name.as_ptr(), 0, std::ptr::null(), REG_OPTION_VOLATILE, KEY_WRITE, std::ptr::null(), &mut created, std::ptr::null_mut()) };
        if create == 0 { unsafe { RegCloseKey(created); } println!("NEG registry write {path}: EXPOSED"); } else { println!("NEG registry write {path}: denied ({create})"); }
    }
    let name: Vec<u16> = r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters".encode_utf16().chain(Some(0)).collect();
    let mut key = std::ptr::null_mut();
    let write = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, name.as_ptr(), 0, KEY_SET_VALUE, &mut key) };
    if write == 0 { unsafe { RegCloseKey(key); } println!("NEG catalog set-value: EXPOSED"); } else { println!("NEG catalog set-value: denied ({write})"); }
    let outside = std::env::var("MINICLAW_OFFLINE_OUTSIDE").unwrap();
    let (read, write) = (fs::read(&outside).is_ok(), fs::write(&outside, "changed").is_ok());
    if read || write { println!("NEG outside-grant file: EXPOSED read={read} write={write}"); } else { println!("NEG outside-grant file: denied"); }
    if fs::File::open(std::env::var("MINICLAW_OFFLINE_PROTECTED").unwrap()).is_ok() { println!("NEG protected state: EXPOSED"); } else { println!("NEG protected state: denied"); }
    fs::write("positive-control", "writable").unwrap();
    assert_eq!(fs::read_to_string("positive-control").unwrap(), "writable");
    println!("NEG complete");
    println!("offline-closed");
}

// ---------------------------------------------------------------------------------------------
// Child of `networked_run_stays_closed_elsewhere`. Reports open statuses only, never a value.

#[repr(C)]
struct UnicodeString { length: u16, maximum: u16, buffer: *const u16 }
#[repr(C)]
struct ObjectAttributes { length: u32, root: *mut std::ffi::c_void, name: *const UnicodeString, attributes: u32, descriptor: *const std::ffi::c_void, quality: *const std::ffi::c_void }
#[link(name = "ntdll")]
unsafe extern "system" {
    fn NtOpenKey(handle: *mut *mut std::ffi::c_void, access: u32, attributes: *const ObjectAttributes) -> i32;
    fn NtClose(handle: *mut std::ffi::c_void) -> i32;
}

/// Opens an absolute NT registry path and returns the status.
fn nt_open(path: &str, access: u32) -> u32 {
    let text: Vec<u16> = path.encode_utf16().collect();
    let name = UnicodeString { length: (text.len() * 2) as u16, maximum: (text.len() * 2) as u16, buffer: text.as_ptr() };
    // OBJ_CASE_INSENSITIVE.
    let attributes = ObjectAttributes { length: std::mem::size_of::<ObjectAttributes>() as u32, root: std::ptr::null_mut(), name: &name,
        attributes: 0x40, descriptor: std::ptr::null(), quality: std::ptr::null() };
    let mut handle = std::ptr::null_mut();
    let status = unsafe { NtOpenKey(&mut handle, access, &attributes) };
    if status == 0 { unsafe { NtClose(handle); } }
    status as u32
}

/// Reports whether a key opens through either API with any mask, or lists a subkey.
fn negative(label: &str, hive: windows_sys::Win32::System::Registry::HKEY, path: &str, nt_path: &str) {
    use windows_sys::Win32::System::Registry::*;
    let wide: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
    let mut exposed = vec![];
    // The last mask is MAXIMUM_ALLOWED.
    for access in [KEY_QUERY_VALUE, KEY_ENUMERATE_SUB_KEYS, KEY_READ, 0x0200_0000] {
        let mut key = std::ptr::null_mut();
        let status = unsafe { RegOpenKeyExW(hive, wide.as_ptr(), 0, access, &mut key) };
        if status == 0 { unsafe { RegCloseKey(key); } }
        if status != 5 && status != 2 { exposed.push(format!("win32 access={access:#x} status={status}")); }
        let status = nt_open(nt_path, access);
        // STATUS_ACCESS_DENIED or STATUS_OBJECT_NAME_NOT_FOUND.
        if status != 0xC000_0022 && status != 0xC000_0034 { exposed.push(format!("nt access={access:#x} status={status:#x}")); }
    }
    if exposed.is_empty() { println!("NEG {label}: denied"); } else { println!("NEG {label}: EXPOSED {}", exposed.join("; ")); }
}

/// Reads one token information class into aligned storage.
fn token_information(class: i32) -> Vec<u64> {
    use windows_sys::Win32::{Foundation::CloseHandle, Security::*, System::Threading::{GetCurrentProcess, OpenProcessToken}};
    unsafe {
        let mut token = std::ptr::null_mut();
        assert_ne!(OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token), 0);
        let mut size = 0;
        GetTokenInformation(token, class, std::ptr::null_mut(), 0, &mut size);
        let mut buffer = vec![0u64; (size as usize).div_ceil(8).max(1)];
        let ok = GetTokenInformation(token, class, buffer.as_mut_ptr().cast(), (buffer.len() * 8) as u32, &mut size);
        CloseHandle(token);
        assert_ne!(ok, 0, "Token information class {class} unavailable");
        buffer
    }
}

fn sid_text(sid: windows_sys::Win32::Security::PSID) -> String {
    use windows_sys::Win32::{Foundation::LocalFree, Security::Authorization::ConvertSidToStringSidW};
    unsafe {
        let mut text = std::ptr::null_mut();
        assert_ne!(ConvertSidToStringSidW(sid, &mut text), 0);
        let mut length = 0;
        while *text.add(length) != 0 { length += 1; }
        let value = String::from_utf16_lossy(std::slice::from_raw_parts(text, length));
        LocalFree(text.cast());
        value
    }
}

/// Runs inside the production networked token: Winsock and the proxy work, everything else is refused.
#[test]
fn networked_child() {
    use std::net::{SocketAddr, TcpStream};
    use windows_sys::Win32::{Security::*, System::Registry::*};
    let Ok(ports) = std::env::var("MINICLAW_NETWORKED_FIXTURE") else { return };
    let container = token_information(TokenIsAppContainer)[0] as u32;
    let groups = token_information(TokenCapabilities);
    let mut capabilities: Vec<String> = unsafe {
        let groups = groups.as_ptr().cast::<TOKEN_GROUPS>();
        (0..(*groups).GroupCount as usize).map(|index| sid_text((*(*groups).Groups.as_ptr().add(index)).Sid)).collect()
    };
    capabilities.sort();
    println!("TOKEN appcontainer={container} capabilities={capabilities:?}");
    let mut expected = vec!["S-1-15-3-1".to_owned(), std::env::var("MINICLAW_NETWORKED_SID").unwrap()];
    expected.sort();
    assert_eq!(container, 1);
    assert_eq!(capabilities, expected, "The token holds exactly internetClient and the installation's Winsock capability");
    let mut data = unsafe { std::mem::zeroed() };
    let startup = unsafe { windows_sys::Win32::Networking::WinSock::WSAStartup(0x0202, &mut data) };
    println!("WSAStartup={startup}");
    crypto_probe();
    let ports: Vec<u16> = ports.split(':').map(|port| port.parse().unwrap()).collect();
    if startup == 0 {
        let connect = |address: SocketAddr| TcpStream::connect_timeout(&address, std::time::Duration::from_secs(2));
        let allowed = connect(SocketAddr::from(([127, 0, 0, 1], ports[0])));
        println!("PROXY reachable={} error={:?}", allowed.is_ok(), allowed.as_ref().err().and_then(|e| e.raw_os_error()));
        for (label, address) in [("second loopback port", SocketAddr::from(([127, 0, 0, 1], ports[1]))),
            ("127.0.0.2", SocketAddr::from(([127, 0, 0, 2], ports[2]))),
            ("::1", SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, ports[3]))),
            ("public address", SocketAddr::from(([1, 1, 1, 1], 443)))] {
            match connect(address) {
                Ok(_) => println!("NEG connect {label}: EXPOSED"),
                Err(error) => println!("NEG connect {label}: denied ({:?})", error.raw_os_error()),
            }
        }
        for address in ["127.0.0.1:0", "0.0.0.0:0", "[::1]:0"] {
            match std::net::TcpListener::bind(address) {
                Ok(_) => println!("NEG listen {address}: EXPOSED"),
                Err(error) => println!("NEG listen {address}: denied ({:?})", error.raw_os_error()),
            }
        }
        // A datagram send can report success and still be dropped; the parent's receivers decide.
        for target in std::env::var("MINICLAW_NETWORKED_UDP").unwrap().split(',') {
            let sent = std::net::UdpSocket::bind("0.0.0.0:0").and_then(|socket| socket.send_to(b"miniclaw-udp-probe", target));
            println!("INFO udp send to {target}: {:?}", sent.map_err(|error| error.raw_os_error()));
            let connected = std::net::UdpSocket::bind("0.0.0.0:0").and_then(|socket| { socket.connect(target)?; socket.send(b"miniclaw-udp-probe") });
            println!("INFO udp connected send to {target}: {:?}", connected.map_err(|error| error.raw_os_error()));
        }
        unsafe { windows_sys::Win32::Networking::WinSock::WSACleanup(); }
    }
    let user = sid_text(unsafe { (*token_information(TokenUser).as_ptr().cast::<TOKEN_USER>()).User.Sid });
    for (label, path) in [("HKCU root", ""), ("HKCU Environment", "Environment"), ("HKCU Software", "SOFTWARE"),
        ("HKCU Software Classes", r"SOFTWARE\Classes"), ("HKCU Software Microsoft", r"SOFTWARE\Microsoft"), ("HKCU Volatile Environment", "Volatile Environment")] {
        let nt = if path.is_empty() { format!(r"\Registry\User\{user}") } else { format!(r"\Registry\User\{user}\{path}") };
        negative(label, HKEY_CURRENT_USER, path, &nt);
    }
    negative("HKU .DEFAULT", HKEY_USERS, ".DEFAULT", r"\Registry\User\.DEFAULT");
    for (label, path) in [("HKLM SOFTWARE", "SOFTWARE"), ("HKLM SOFTWARE Windows NT CurrentVersion", r"SOFTWARE\Microsoft\Windows NT\CurrentVersion"),
        ("HKLM SOFTWARE Classes", r"SOFTWARE\Classes"), ("HKLM SYSTEM", "SYSTEM"), ("HKLM SYSTEM Control", r"SYSTEM\CurrentControlSet\Control"),
        ("HKLM Session Manager Environment", r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment"),
        ("HKLM Services", r"SYSTEM\CurrentControlSet\Services"), ("HKLM Services LanmanServer", r"SYSTEM\CurrentControlSet\Services\LanmanServer"),
        ("HKLM Services WinSock2", r"SYSTEM\CurrentControlSet\Services\WinSock2"), ("HKLM Services WinSock", r"SYSTEM\CurrentControlSet\Services\WinSock"),
        ("HKLM SAM", "SAM"), ("HKLM SECURITY", "SECURITY"), ("HKLM HARDWARE", "HARDWARE")] {
        negative(label, HKEY_LOCAL_MACHINE, path, &format!(r"\Registry\Machine\{path}"));
    }
    // The root key is granted query-value only: it opens, but lists nothing and reads no more.
    let empty = [0u16];
    for (label, access) in [("enumerate", KEY_ENUMERATE_SUB_KEYS), ("read", KEY_READ), ("maximum", 0x0200_0000)] {
        let mut key = std::ptr::null_mut();
        let status = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, empty.as_ptr(), 0, access, &mut key) };
        let mut listing = 5;
        if status == 0 {
            let mut name = [0u16; 256];
            let mut length = name.len() as u32;
            // Only the status is used; the name is discarded.
            listing = unsafe { RegEnumKeyExW(key, 0, name.as_mut_ptr(), &mut length, std::ptr::null(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()) };
            unsafe { RegCloseKey(key); }
        }
        if listing == 0 { println!("NEG HKLM root {label}: EXPOSED open={status} listing={listing}"); }
        else { println!("NEG HKLM root {label}: denied (open={status} listing={listing})"); }
    }
    for path in [r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\MiniClawWriteProbe", r"SOFTWARE\MiniClawWriteProbe"] {
        let name: Vec<u16> = path.encode_utf16().chain(Some(0)).collect();
        let mut created = std::ptr::null_mut();
        let create = unsafe { RegCreateKeyExW(HKEY_LOCAL_MACHINE, name.as_ptr(), 0, std::ptr::null(), REG_OPTION_VOLATILE, KEY_WRITE, std::ptr::null(), &mut created, std::ptr::null_mut()) };
        if create == 0 { unsafe { RegCloseKey(created); } println!("NEG registry write {path}: EXPOSED"); } else { println!("NEG registry write {path}: denied ({create})"); }
    }
    // A catalog key is readable but not writable.
    let name: Vec<u16> = r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters".encode_utf16().chain(Some(0)).collect();
    let mut key = std::ptr::null_mut();
    let write = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, name.as_ptr(), 0, KEY_SET_VALUE, &mut key) };
    if write == 0 { unsafe { RegCloseKey(key); } println!("NEG catalog set-value: EXPOSED"); } else { println!("NEG catalog set-value: denied ({write})"); }
    let outside = std::env::var("MINICLAW_NETWORKED_OUTSIDE").unwrap();
    let (read, write) = (fs::read(&outside).is_ok(), fs::write(&outside, "changed").is_ok());
    if read || write { println!("NEG outside-grant file: EXPOSED read={read} write={write}"); } else { println!("NEG outside-grant file: denied"); }
    if fs::File::open(std::env::var("MINICLAW_NETWORKED_PROTECTED").unwrap()).is_ok() { println!("NEG protected state: EXPOSED"); } else { println!("NEG protected state: denied"); }
    fs::write("positive-control", "writable").unwrap();
    assert_eq!(fs::read_to_string("positive-control").unwrap(), "writable");
    println!("NEG complete");
}

#[link(name = "bcrypt")]
unsafe extern "system" {
    fn BCryptGenRandom(algorithm: *mut std::ffi::c_void, buffer: *mut u8, length: u32, flags: u32) -> i32;
    fn BCryptOpenAlgorithmProvider(algorithm: *mut *mut std::ffi::c_void, id: *const u16, implementation: *const u16, flags: u32) -> i32;
    fn BCryptCloseAlgorithmProvider(algorithm: *mut std::ffi::c_void, flags: u32) -> i32;
}
#[link(name = "advapi32")]
unsafe extern "system" {
    fn SystemFunction036(buffer: *mut u8, length: u32) -> u8;
    fn CryptAcquireContextW(provider: *mut usize, container: *const u16, name: *const u16, kind: u32, flags: u32) -> i32;
}

/// Reports which Windows random-number and crypto entry points work in this token. TLS libraries
/// seed themselves through one of these; the statuses explain a tool that cannot start.
fn crypto_probe() {
    let mut bytes = [0u8; 16];
    // BCRYPT_USE_SYSTEM_PREFERRED_RNG.
    let preferred = unsafe { BCryptGenRandom(std::ptr::null_mut(), bytes.as_mut_ptr(), 16, 2) };
    let legacy = unsafe { SystemFunction036(bytes.as_mut_ptr(), 16) };
    let mut results = vec![format!("BCryptGenRandom(system preferred)={preferred:#x}"), format!("RtlGenRandom={legacy}")];
    for name in ["RNG", "SHA256", "AES", "RSA", "ECDH_P256"] {
        let id: Vec<u16> = name.encode_utf16().chain(Some(0)).collect();
        let mut provider = std::ptr::null_mut();
        let status = unsafe { BCryptOpenAlgorithmProvider(&mut provider, id.as_ptr(), std::ptr::null(), 0) };
        if status == 0 { unsafe { BCryptCloseAlgorithmProvider(provider, 0); } }
        results.push(format!("BCryptOpenAlgorithmProvider({name})={status:#x}"));
    }
    let mut provider = 0usize;
    // PROV_RSA_FULL with CRYPT_VERIFYCONTEXT | CRYPT_SILENT.
    let acquired = unsafe { CryptAcquireContextW(&mut provider, std::ptr::null(), std::ptr::null(), 1, 0xF000_0040) };
    results.push(format!("CryptAcquireContext={acquired} error={:?}", if acquired == 0 { std::io::Error::last_os_error().raw_os_error() } else { None }));
    println!("INFO crypto {}", results.join(" "));
}
