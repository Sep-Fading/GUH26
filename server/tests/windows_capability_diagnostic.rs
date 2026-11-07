#![cfg(windows)]
// Isolated native experiment only; no production capability configuration exists.
use miniclaw_server::{platform_fs, seatbelt::Grant, windows_network::Lease, windows_sandbox::Sandbox};
use std::{fs, ptr, time::Duration};
use windows_sys::Win32::{Foundation::*, Security::{Authorization::*, *}, System::Registry::*};

/// Encodes only local diagnostic names for Win32 APIs.
fn wide(value: &str) -> Vec<u16> { value.encode_utf16().chain(Some(0)).collect() }

/// Uses the actual token user, never the elevated token's possible Administrators default owner.
fn owner_sid() -> String {
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token = ptr::null_mut();
        assert_ne!(OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token), 0);
        let mut size = 0;
        GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut size);
        let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
        let ok = GetTokenInformation(token, TokenUser, buffer.as_mut_ptr().cast(), size, &mut size);
        CloseHandle(token);
        assert_ne!(ok, 0);
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut text = ptr::null_mut();
        assert_ne!(ConvertSidToStringSidW(user.User.Sid, &mut text), 0);
        let mut length = 0;
        while *text.add(length) != 0 { length += 1; }
        let result = String::from_utf16(std::slice::from_raw_parts(text, length)).unwrap();
        LocalFree(text.cast());
        result
    }
}

struct PrivateKey { hive: HKEY, name: Vec<u16>, key: HKEY }
impl PrivateKey {
    /// Creates an owner-only volatile sentinel without changing an existing registry key.
    fn new(hive: HKEY, name: &str) -> Self { Self::create(hive, name, true) }

    /// Creates a sentinel with the parent's inherited ACL, as ordinary applications store settings.
    fn inherited(hive: HKEY, name: &str) -> Self { Self::create(hive, name, false) }

    fn create(hive: HKEY, name: &str, owner_only: bool) -> Self {
        unsafe {
            let mut descriptor = ptr::null_mut();
            let owner = owner_sid();
            let sddl = format!("O:{owner}D:P(A;;KA;;;{owner})");
            assert_ne!(ConvertStringSecurityDescriptorToSecurityDescriptorW(wide(&sddl).as_ptr(), SDDL_REVISION_1,
                &mut descriptor, ptr::null_mut()), 0);
            let attributes = SECURITY_ATTRIBUTES { nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor, bInheritHandle: 0 };
            let name = wide(name);
            let mut key = ptr::null_mut();
            let mut disposition = 0;
            let status = RegCreateKeyExW(hive, name.as_ptr(), 0, ptr::null(), REG_OPTION_VOLATILE,
                KEY_ALL_ACCESS, if owner_only { &attributes } else { ptr::null() }, &mut key, &mut disposition);
            LocalFree(descriptor);
            assert_eq!(status, 0);
            assert_eq!(disposition, REG_CREATED_NEW_KEY);
            let fixture = Self { hive, name, key };
            let value = wide("synthetic-private-sentinel");
            assert_eq!(RegSetValueExW(key, wide("sentinel").as_ptr(), 0, REG_SZ,
                value.as_ptr().cast(), (value.len() * 2) as u32), 0);
            let mut control = ptr::null_mut();
            assert_eq!(RegOpenKeyExW(hive, fixture.name.as_ptr(), 0, KEY_QUERY_VALUE, &mut control), 0);
            let mut bytes = [0u8; 128];
            let mut size = bytes.len() as u32;
            let mut kind = 0;
            let queried = RegQueryValueExW(control, wide("sentinel").as_ptr(), ptr::null(), &mut kind, bytes.as_mut_ptr(), &mut size);
            RegCloseKey(control);
            assert_eq!(queried, 0);
            assert_eq!(kind, REG_SZ);
            assert_eq!(&bytes[..size as usize], std::slice::from_raw_parts(value.as_ptr().cast::<u8>(), value.len() * 2));
            fixture
        }
    }
}
impl Drop for PrivateKey {
    fn drop(&mut self) { unsafe { RegCloseKey(self.key); assert_eq!(RegDeleteKeyW(self.hive, self.name.as_ptr()), 0); } }
}

/// Prints one key's DACL as SDDL; descriptors name trustees and rights, never stored values.
fn print_dacl(hive: HKEY, label: &str, name: &str) {
    unsafe {
        let mut key = ptr::null_mut();
        let status = RegOpenKeyExW(hive, wide(name).as_ptr(), 0, 0x0002_0000, &mut key);
        if status != 0 { println!("DACL {label}: open failed {status}"); return; }
        let mut descriptor = ptr::null_mut();
        let status = GetSecurityInfo(key, SE_REGISTRY_KEY, DACL_SECURITY_INFORMATION, ptr::null_mut(), ptr::null_mut(),
            ptr::null_mut(), ptr::null_mut(), &mut descriptor);
        RegCloseKey(key);
        if status != 0 { println!("DACL {label}: query failed {status}"); return; }
        let mut text = ptr::null_mut();
        let mut length = 0;
        if ConvertSecurityDescriptorToStringSecurityDescriptorW(descriptor, SDDL_REVISION_1, DACL_SECURITY_INFORMATION,
            &mut text, &mut length) != 0 {
            let mut size = 0;
            while *text.add(size) != 0 { size += 1; }
            println!("DACL {label}: {}", String::from_utf16_lossy(std::slice::from_raw_parts(text, size)));
            LocalFree(text.cast());
        }
        LocalFree(descriptor);
    }
}

/// Lists which immediate subkeys carry the registryRead capability ACE; names and ACLs only, no values.
fn capability_ace_survey(label: &str, parent: &str) {
    const REGISTRY_READ: &str = "S-1-15-3-1024-1065365936-1281604716-3511738428-1654721687-432734479-3232135806-4053264122-3456934681";
    unsafe {
        let mut key = ptr::null_mut();
        if RegOpenKeyExW(HKEY_CURRENT_USER, wide(parent).as_ptr(), 0, KEY_ENUMERATE_SUB_KEYS, &mut key) != 0 { return; }
        for index in 0..512 {
            let mut name = [0u16; 256];
            let mut length = name.len() as u32;
            if RegEnumKeyExW(key, index, name.as_mut_ptr(), &mut length, ptr::null(), ptr::null_mut(), ptr::null_mut(), ptr::null_mut()) != 0 { break; }
            let mut child = ptr::null_mut();
            if RegOpenKeyExW(key, name.as_ptr(), 0, 0x0002_0000, &mut child) != 0 { continue; }
            let mut descriptor = ptr::null_mut();
            let status = GetSecurityInfo(child, SE_REGISTRY_KEY, DACL_SECURITY_INFORMATION, ptr::null_mut(), ptr::null_mut(),
                ptr::null_mut(), ptr::null_mut(), &mut descriptor);
            RegCloseKey(child);
            if status != 0 { continue; }
            let mut text = ptr::null_mut();
            if ConvertSecurityDescriptorToStringSecurityDescriptorW(descriptor, SDDL_REVISION_1, DACL_SECURITY_INFORMATION,
                &mut text, ptr::null_mut()) != 0 {
                let mut size = 0;
                while *text.add(size) != 0 { size += 1; }
                let sddl = String::from_utf16_lossy(std::slice::from_raw_parts(text, size));
                LocalFree(text.cast());
                // An ACE is "(A;flags;rights;;;SID)"; the flags field shows whether descendants inherit it.
                if let Some(ace) = sddl.split('(').find(|ace| ace.contains(REGISTRY_READ)) {
                    let flags = ace.split(';').nth(1).unwrap_or("");
                    println!("Survey {label}\\{}: registryRead ACE flags='{flags}'", String::from_utf16_lossy(&name[..length as usize]));
                }
            }
            LocalFree(descriptor);
        }
        RegCloseKey(key);
    }
}

/// Reports, without asserting, whether ordinary inherited-ACL registry settings are readable.
fn inherited_exposure() {
    let Ok(name) = std::env::var("MC_INHERITED_KEY") else { return };
    for (label, hive) in [("HKCU", HKEY_CURRENT_USER), ("HKLM", HKEY_LOCAL_MACHINE)] {
        // Root and Software opens obtain handles only; no real value or subkey name is read.
        for (target, path) in [("root", String::new()), ("Software", "SOFTWARE".to_owned()), ("Environment", "Environment".to_owned()),
            ("Software Classes", r"SOFTWARE\Classes".to_owned()), ("inherited sentinel", name.clone())] {
            let mut key = ptr::null_mut();
            let status = unsafe { RegOpenKeyExW(hive, wide(&path).as_ptr(), 0, KEY_QUERY_VALUE, &mut key) };
            let mut value = String::new();
            if status == 0 {
                if target == "inherited sentinel" {
                    let mut bytes = [0u8; 128];
                    let mut size = bytes.len() as u32;
                    let query = unsafe { RegQueryValueExW(key, wide("sentinel").as_ptr(), ptr::null(), ptr::null_mut(), bytes.as_mut_ptr(), &mut size) };
                    value = format!(", sentinel value query={query}");
                    let mut created = ptr::null_mut();
                    let write = unsafe { RegCreateKeyExW(key, wide("child-write").as_ptr(), 0, ptr::null(), REG_OPTION_VOLATILE,
                        KEY_WRITE, ptr::null(), &mut created, ptr::null_mut()) };
                    if write == 0 { unsafe { RegCloseKey(created); } }
                    value += &format!(", subkey create={write}");
                }
                unsafe { RegCloseKey(key); }
            }
            println!("Exposure {label} {target}: open={status}{value}");
        }
    }
}

struct Capabilities { allocations: Vec<PSID>, entries: Vec<SID_AND_ATTRIBUTES> }
impl Capabilities {
    /// Derives only the two reviewed hardcoded capabilities, retaining all native allocations.
    fn new(names: &[&str]) -> Self {
        let mut result = Self { allocations: vec![], entries: vec![] };
        for name in names {
            assert!(["internetClient", "registryRead"].contains(name));
            unsafe {
                let (mut groups, mut capabilities) = (ptr::null_mut(), ptr::null_mut());
                let (mut group_count, mut count) = (0, 0);
                assert_ne!(DeriveCapabilitySidsFromName(wide(name).as_ptr(), &mut groups, &mut group_count,
                    &mut capabilities, &mut count), 0);
                for i in 0..group_count { LocalFree(*groups.add(i as usize)); }
                LocalFree(groups.cast());
                for i in 0..count {
                    let sid = *capabilities.add(i as usize);
                    result.allocations.push(sid);
                    // SE_GROUP_ENABLED from winnt.h; this test adds no production API feature.
                    result.entries.push(SID_AND_ATTRIBUTES { Sid: sid, Attributes: 0x4 });
                }
                LocalFree(capabilities.cast());
            }
        }
        result
    }
}
impl Drop for Capabilities {
    fn drop(&mut self) { for sid in &self.allocations { unsafe { LocalFree(*sid); } } }
}

#[tokio::test]
#[ignore = "isolated elevated Windows capability experiment; not production acceptance"]
/// Compares fixed capabilities under identical WFP denial, with synthetic privacy sentinels.
async fn capability_matrix() {
    let path = std::env::temp_dir().join(format!("mc-capability-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&path, false).unwrap();
    let outside = path.join("outside-secret");
    fs::write(&outside, "synthetic-file-secret").unwrap();
    let work = path.join("work");
    fs::create_dir(&work).unwrap();
    let name = format!("SOFTWARE\\MiniClawCapabilityDiagnostic-{}", uuid::Uuid::new_v4());
    let _user = PrivateKey::new(HKEY_CURRENT_USER, &name);
    let _machine = PrivateKey::new(HKEY_LOCAL_MACHINE, &name);
    let inherited = format!("SOFTWARE\\MiniClawCapabilityInherited-{}", uuid::Uuid::new_v4());
    let _user_inherited = PrivateKey::inherited(HKEY_CURRENT_USER, &inherited);
    let _machine_inherited = PrivateKey::inherited(HKEY_LOCAL_MACHINE, &inherited);
    for (label, hive, key) in [("HKCU root", HKEY_CURRENT_USER, ""), ("HKCU Software", HKEY_CURRENT_USER, "SOFTWARE"), ("HKCU Environment", HKEY_CURRENT_USER, "Environment"),
        ("HKCU inherited sentinel", HKEY_CURRENT_USER, inherited.as_str()), ("HKCU owner-only sentinel", HKEY_CURRENT_USER, name.as_str()),
        ("HKLM Software", HKEY_LOCAL_MACHINE, "SOFTWARE"), ("HKLM inherited sentinel", HKEY_LOCAL_MACHINE, inherited.as_str()),
        ("HKLM Winsock catalog", HKEY_LOCAL_MACHINE, r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\Protocol_Catalog9")] {
        print_dacl(hive, label, key);
    }
    for (label, parent) in [("HKCU", ""), ("HKCU\\Software", "SOFTWARE"), ("HKCU\\Software\\Microsoft", r"SOFTWARE\Microsoft"),
        ("HKCU\\Software\\Microsoft\\Windows\\CurrentVersion", r"SOFTWARE\Microsoft\Windows\CurrentVersion")] {
        capability_ace_survey(label, parent);
    }
    let program = std::env::current_exe().unwrap();
    for names in [vec![], vec!["internetClient"], vec!["registryRead"], vec!["internetClient", "registryRead"]] {
        let mut sandbox = Sandbox::new(&[
            Grant { path: work.to_string_lossy().into_owned(), write: true, folder: true, blocked: false },
            Grant { path: program.to_string_lossy().into_owned(), write: false, folder: false, blocked: false },
        ], &work).unwrap();
        let allowed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let denied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = allowed.local_addr().unwrap().port();
        let blocked = denied.local_addr().unwrap().port();
        let mut capabilities = Capabilities::new(&names);
        let native = SECURITY_CAPABILITIES { AppContainerSid: sandbox.security().capabilities.AppContainerSid,
            Capabilities: if capabilities.entries.is_empty() { ptr::null_mut() } else { capabilities.entries.as_mut_ptr() },
            CapabilityCount: capabilities.entries.len() as u32, Reserved: 0 };
        let security = miniclaw_server::windows_process::SecurityContext { capabilities: &native, low_privilege: true };
        let mut env: Vec<_> = sandbox.environment().into_iter().map(|(k,v)| (k.into_string().unwrap(),v.into_string().unwrap())).collect();
        env.extend([("MC_CAPABILITY_CHILD".into(), format!("{port}:{blocked}")), ("MC_PRIVATE_KEY".into(), name.clone()),
            ("MC_INHERITED_KEY".into(), inherited.clone()),
            ("MC_PRIVATE_FILE".into(), outside.to_string_lossy().into_owned())]);
        let pending = miniclaw_server::windows_process::pending_stdio_restricted(
            &program, &["--exact".into(), "capability_child".into(), "--nocapture".into()], &work, &env, &security).unwrap();
        sandbox.bind_process(&pending.server.process).unwrap();
        let tree = pending.server.process.tree_guard();
        let mut lease = Lease::install(&sandbox.package_sid().unwrap(), port).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(30), miniclaw_server::windows_process::capture_stdio(pending.resume().unwrap())).await;
        tree.terminate_and_wait(Duration::from_secs(10)).unwrap();
        lease.revoke().unwrap();
        let (output, errors, code) = result.unwrap().unwrap();
        println!("CAPABILITY MODE {names:?}\n{}\n{}", String::from_utf8_lossy(&output), String::from_utf8_lossy(&errors));
        assert_eq!(code, 0);
        assert_eq!(fs::read_to_string(&outside).unwrap(), "synthetic-file-secret");
        drop(sandbox);
        miniclaw_server::windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    }
    fs::remove_dir_all(path).unwrap();
}

#[test]
/// Child never reads real private data: only fixed catalog handles and disposable sentinels.
fn capability_child() {
    let Ok(ports) = std::env::var("MC_CAPABILITY_CHILD") else { return };
    private_denials();
    inherited_exposure();
    for catalog in [r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\Protocol_Catalog9",
        r"SYSTEM\CurrentControlSet\Services\WinSock2\Parameters\NameSpace_Catalog5"] {
        let mut key = ptr::null_mut();
        let status = unsafe { RegOpenKeyExW(HKEY_LOCAL_MACHINE, wide(catalog).as_ptr(), 0, KEY_READ, &mut key) };
        println!("Public catalog {catalog}: {status}");
        if status == 0 { unsafe { RegCloseKey(key); } }
    }
    let mut data = unsafe { std::mem::zeroed() };
    let status = unsafe { windows_sys::Win32::Networking::WinSock::WSAStartup(0x0202, &mut data) };
    println!("WSAStartup={status}");
    if status == 0 {
        let ports: Vec<u16> = ports.split(':').map(|p| p.parse().unwrap()).collect();
        let connect = |port| std::net::TcpStream::connect_timeout(&std::net::SocketAddr::from(([127,0,0,1],port)), Duration::from_secs(2));
        let allowed = connect(ports[0]);
        println!("Exact proxy reachable={}, error={:?}", allowed.is_ok(), allowed.as_ref().err());
        assert!(connect(ports[1]).is_err(), "WFP blocked endpoint became reachable");
        unsafe { windows_sys::Win32::Networking::WinSock::WSACleanup(); }
    }
}

/// Uses identical positive/negative privacy controls in each native diagnostic child.
fn private_denials() {
    let name = wide(&std::env::var("MC_PRIVATE_KEY").unwrap());
    for hive in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let mut key = ptr::null_mut();
        let status = unsafe { RegOpenKeyExW(hive, name.as_ptr(), 0, KEY_QUERY_VALUE, &mut key) };
        if status == 0 { unsafe { RegCloseKey(key); } }
        assert!([ERROR_ACCESS_DENIED, ERROR_FILE_NOT_FOUND].contains(&status), "Private registry sentinel became accessible: {status}");
    }
    let secret = std::env::var("MC_PRIVATE_FILE").unwrap();
    assert!(fs::read(&secret).is_err());
    assert!(fs::write(&secret, "changed").is_err());
    fs::write("positive-control", "writable").unwrap();
    assert_eq!(fs::read_to_string("positive-control").unwrap(), "writable");
}

#[tokio::test]
#[ignore = "reviewed isolated network boundary experiment with fixed capabilities"]
/// Proves endpoint/protocol restrictions and descendant inheritance before production adoption.
async fn capability_network_boundaries() {
    use std::net::{TcpListener, TcpStream, UdpSocket};
    let root = std::env::temp_dir().join(format!("mc-capability-boundary-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&root, false).unwrap();
    let work = root.join("work");
    fs::create_dir(&work).unwrap();
    let secret = root.join("private-file");
    fs::write(&secret, "outside-sentinel").unwrap();
    let registry = format!("SOFTWARE\\MiniClawCapabilityDiagnostic-{}", uuid::Uuid::new_v4());
    let _user = PrivateKey::new(HKEY_CURRENT_USER, &registry);
    let _machine = PrivateKey::new(HKEY_LOCAL_MACHINE, &registry);
    let program = std::env::current_exe().unwrap();
    for mode in ["offline", "proxy", "listener"] {
        let proxy = TcpListener::bind("127.0.0.1:0").unwrap();
        let denied = TcpListener::bind("127.0.0.1:0").unwrap();
        let other_address = TcpListener::bind("127.0.0.2:0").unwrap();
        let ipv6 = TcpListener::bind("[::1]:0").unwrap();
        // Live receivers and same-protocol positive controls rule out unavailable services.
        for receiver in [&proxy, &denied, &other_address, &ipv6] {
            let control = TcpStream::connect_timeout(&receiver.local_addr().unwrap(), Duration::from_secs(2)).unwrap();
            drop(receiver.accept().unwrap());
            drop(control);
            receiver.set_nonblocking(true).unwrap();
        }
        let proxy_port = proxy.local_addr().unwrap().port();
        let udp = UdpSocket::bind((std::net::Ipv4Addr::LOCALHOST, proxy_port)).unwrap();
        udp.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").unwrap();
        sender.send_to(b"positive-udp", udp.local_addr().unwrap()).unwrap();
        let mut bytes = [0u8; 64];
        let count = udp.recv(&mut bytes).unwrap();
        assert_eq!(&bytes[..count], b"positive-udp");
        udp.set_nonblocking(true).unwrap();
        let reservation = TcpListener::bind("127.0.0.1:0").unwrap();
        let listen_port = reservation.local_addr().unwrap().port();
        drop(reservation);
        let mut sandbox = Sandbox::new(&[
            Grant { path: work.to_string_lossy().into_owned(), write: true, folder: true, blocked: false },
            Grant { path: program.to_string_lossy().into_owned(), write: false, folder: false, blocked: false },
        ], &work).unwrap();
        let mut capabilities = Capabilities::new(if mode == "offline" { &[] } else { &["internetClient", "registryRead"] });
        let native = SECURITY_CAPABILITIES { AppContainerSid: sandbox.security().capabilities.AppContainerSid,
            Capabilities: if capabilities.entries.is_empty() { ptr::null_mut() } else { capabilities.entries.as_mut_ptr() },
            CapabilityCount: capabilities.entries.len() as u32, Reserved: 0 };
        let security = if mode == "offline" { sandbox.security() } else {
            miniclaw_server::windows_process::SecurityContext { capabilities: &native, low_privilege: true }
        };
        let mut env: Vec<_> = sandbox.environment().into_iter().map(|(k,v)| (k.into_string().unwrap(),v.into_string().unwrap())).collect();
        env.extend([("MC_BOUNDARY_MODE".into(), mode.into()), ("MC_PRIVATE_KEY".into(), registry.clone()),
            ("MC_PRIVATE_FILE".into(), secret.to_string_lossy().into_owned()),
            ("MC_BOUNDARY_PORTS".into(), format!("{proxy_port}:{}:{}:{}:{listen_port}", denied.local_addr().unwrap().port(),
                ipv6.local_addr().unwrap().port(), other_address.local_addr().unwrap().port()))]);
        let pending = miniclaw_server::windows_process::pending_stdio_restricted(&program,
            &["--exact".into(), "network_boundary_child".into(), "--nocapture".into()], &work, &env, &security).unwrap();
        sandbox.bind_process(&pending.server.process).unwrap();
        let tree = pending.server.process.tree_guard();
        let mut lease = if mode == "offline" { None } else {
            Some(Lease::install_ports(&sandbox.package_sid().unwrap(), proxy_port, &[],
                if mode == "listener" { std::slice::from_ref(&listen_port) } else { &[] }).unwrap())
        };
        if mode == "listener" { lease.as_mut().unwrap().start_inbound(&sandbox.profile_name()).unwrap(); }
        let running = miniclaw_server::windows_process::capture_stdio(pending.resume().unwrap());
        let connect = async {
            if mode != "listener" { return Ok::<_, std::io::Error>(String::new()); }
            use tokio::io::AsyncReadExt;
            loop {
                if let Ok(mut stream) = tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, listen_port)).await {
                    let mut response = String::new();
                    stream.read_to_string(&mut response).await?;
                    return Ok(response);
                }
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        };
        let (captured, inbound) = tokio::join!(tokio::time::timeout(Duration::from_secs(40), running),
            tokio::time::timeout(Duration::from_secs(20), connect));
        tree.terminate_and_wait(Duration::from_secs(10)).unwrap();
        if let Some(lease) = lease.as_mut() { lease.revoke().unwrap(); }
        let (output, errors, code) = captured.unwrap().unwrap();
        println!("NETWORK BOUNDARY {mode}\n{}\n{}", String::from_utf8_lossy(&output), String::from_utf8_lossy(&errors));
        assert_eq!(code, 0);
        if mode == "listener" { assert_eq!(inbound.unwrap().unwrap(), "declared-listener"); }
        tokio::time::sleep(Duration::from_millis(200)).await;
        for receiver in [&denied, &other_address, &ipv6] { assert_eq!(receiver.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock); }
        assert_eq!(udp.recv(&mut bytes).unwrap_err().kind(), std::io::ErrorKind::WouldBlock, "Denied UDP reached live receiver");
        if mode == "offline" { assert_eq!(proxy.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock); }
        else { drop(proxy.accept().expect("Positive proxy connection absent")); }
        assert_eq!(fs::read_to_string(&secret).unwrap(), "outside-sentinel");
        drop(sandbox);
        miniclaw_server::windows_sandbox::drain_cleanup(Duration::from_secs(35)).unwrap();
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
/// Repeats denied sockets in a normally inherited child token, without new token creation.
fn network_boundary_child() {
    use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
    let Ok(mode) = std::env::var("MC_BOUNDARY_MODE") else { return };
    private_denials();
    let ports: Vec<u16> = std::env::var("MC_BOUNDARY_PORTS").unwrap().split(':').map(|p| p.parse().unwrap()).collect();
    let mut data = unsafe { std::mem::zeroed() };
    let startup = unsafe { windows_sys::Win32::Networking::WinSock::WSAStartup(0x0202, &mut data) };
    let connect = |address| TcpStream::connect_timeout(&address, Duration::from_secs(2));
    if startup == 0 {
        let allowed = connect(SocketAddr::from(([127,0,0,1],ports[0])));
        if mode == "offline" { assert!(allowed.is_err()); } else { allowed.unwrap(); }
        assert!(connect(SocketAddr::from(([127,0,0,1],ports[1]))).is_err());
        assert!(connect(SocketAddr::from((std::net::Ipv6Addr::LOCALHOST,ports[2]))).is_err());
        assert!(connect(SocketAddr::from(([127,0,0,2],ports[3]))).is_err());
        if let Ok(udp) = UdpSocket::bind("127.0.0.1:0") {
            let _ = udp.send_to(b"must-not-arrive", SocketAddr::from(([127,0,0,1],ports[0])));
        }
        assert!(TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, ports[4])).is_err());
        assert!(TcpListener::bind("127.0.0.1:0").is_err());
        assert!(TcpListener::bind("[::1]:0").is_err());
    } else { assert_eq!(mode, "offline", "Enabled network could not initialize: {startup}"); }
    let descendant = std::env::var_os("MC_BOUNDARY_DESCENDANT").is_some();
    if !descendant {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "network_boundary_child", "--nocapture"]).env("MC_BOUNDARY_DESCENDANT", "1").output().unwrap();
        assert!(output.status.success(), "Descendant failed: {} {}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        assert!(String::from_utf8_lossy(&output.stdout).contains("descendant-network-boundary"));
        if mode == "listener" {
            use std::io::Write;
            let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, ports[4])).unwrap();
            let (mut client, _) = listener.accept().unwrap();
            client.write_all(b"declared-listener").unwrap();
        }
    } else { println!("descendant-network-boundary"); }
    if startup == 0 { unsafe { windows_sys::Win32::Networking::WinSock::WSACleanup(); } }
}
