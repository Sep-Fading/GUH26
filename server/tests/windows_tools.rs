#![cfg(windows)]
// Runs real tools inside a networked sandbox run, through the production shell path.
use miniclaw_server::{executor, platform_fs, scoped::Operation, seatbelt::Grant, windows_sandbox};
// Keeps disposable working folders.
use std::{fs, path::Path, time::Duration};

/// Runs one shell command the way the product does: pinned shell, fresh sandbox, the approved
/// hosts' proxy, and a lease from the installed network service.
async fn shell(text: &str, hosts: &[&str], runtime: Option<&Path>) -> serde_json::Value {
    let work = std::env::temp_dir().join(format!("mc-tools-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&work, false).unwrap();
    let result = shell_in(&work, text, hosts, runtime).await;
    let _ = fs::remove_dir_all(work);
    result
}

/// Runs one command in an existing working folder.
async fn shell_in(work: &Path, text: &str, hosts: &[&str], runtime: Option<&Path>) -> serde_json::Value {
    let cwd = work.to_string_lossy().into_owned();
    let mut grants = vec![Grant { path: cwd.clone(), folder: true, write: true, blocked: false }];
    if let Some(folder) = runtime {
        grants.push(Grant { path: folder.to_string_lossy().into_owned(), folder: true, write: false, blocked: false });
    }
    let operation = Operation::RunShell { text: text.to_owned(), cwd, hosts: hosts.iter().map(|host| (*host).to_owned()).collect() };
    let result = executor::execute_host(operation, &grants).await;
    windows_sandbox::drain_cleanup(Duration::from_secs(60)).unwrap();
    println!("COMMAND {text}\nRESULT {result}");
    result
}

/// A Windows path in the form the pinned shell takes, single-quoted.
fn quoted(path: &Path) -> String {
    format!("'{}'", path.to_string_lossy().replace('\\', "/"))
}

#[tokio::test]
#[ignore = "requires the installed network service, the bundled Git and internet access to github.com and badssl.com"]
/// The bundled Git lists a public repository over HTTPS through the approved host's proxy, from a
/// folder that is not a repository (discovery stops at the grant). Certificates are still verified
/// against the bundled CA file, a host that was not approved is refused and named, and a run
/// without approved hosts has no network.
async fn git_ls_remote_through_the_approved_proxy() {
    let git = miniclaw_server::windows_git::load().unwrap().expect("the bundled Git is installed");
    println!("GIT {}", git.root.join(&git.program).display());
    let work = std::env::temp_dir().join(format!("mc-tools-git-{}", uuid::Uuid::new_v4()));
    platform_fs::private_dir(&work, false).unwrap();
    let version = shell_in(&work, "git --version && printf 'ceiling=%s\\n' \"$GIT_CEILING_DIRECTORIES\"", &[], None).await;
    assert_eq!(version["exitCode"], 0, "{version}");
    let listed = shell_in(&work, "git ls-remote https://github.com/git/git.git HEAD", &["github.com"], None).await;
    assert_eq!(listed["exitCode"], 0, "{listed}");
    let text = listed["text"].as_str().unwrap();
    let line = text.lines().find(|line| line.ends_with("\tHEAD")).unwrap_or_else(|| panic!("no HEAD line: {listed}"));
    assert!(line.len() == 45 && line[..40].bytes().all(|b| b.is_ascii_hexdigit()), "{line}");
    assert_eq!(listed["refusedHosts"], serde_json::json!([]));
    // An approved host whose certificate is invalid is still rejected by OpenSSL with the bundled CA file
    // (curl 8.22 reports "SSL certificate OpenSSL verify result: <reason> (<code>)").
    for (host, reason) in [("expired.badssl.com", "certificate has expired"), ("self-signed.badssl.com", "self-signed certificate")] {
        let invalid = shell_in(&work, &format!("git ls-remote https://{host}/"), &[host], None).await;
        assert_eq!(invalid["exitCode"], 128, "{invalid}");
        let stderr = invalid["stderr"].as_str().unwrap();
        assert!(stderr.contains("SSL certificate") && stderr.contains(reason), "{invalid}");
        assert_eq!(invalid["refusedHosts"], serde_json::json!([]), "{invalid}");
    }
    let refused = shell_in(&work, "git ls-remote https://gitlab.com/gitlab-org/gitlab-foss.git HEAD", &["github.com"], None).await;
    assert_ne!(refused["exitCode"], 0, "{refused}");
    assert!(refused["refusedHosts"].as_array().unwrap().iter().any(|host| host.as_str().unwrap().contains("gitlab.com")), "{refused}");
    // Without approved hosts the same command has no network at all.
    let offline = shell_in(&work, "git ls-remote https://github.com/git/git.git HEAD", &[], None).await;
    assert_ne!(offline["exitCode"], 0, "{offline}");
    let _ = fs::remove_dir_all(work);
}

/// The runtime named by a test variable, or nothing when the test setup has none.
fn runtime(variable: &str) -> Option<std::path::PathBuf> {
    let Some(path) = std::env::var_os(variable).map(std::path::PathBuf::from) else {
        println!("SKIPPED: {variable} is not set; this runtime is not available in the test setup");
        return None;
    };
    assert!(path.is_file(), "{variable} names a missing file: {}", path.display());
    Some(path)
}

#[tokio::test]
#[ignore = "requires the installed network service, a Node runtime and internet access to example.com"]
/// Node makes an HTTPS request through the proxy; a host that was not approved is refused.
async fn node_fetches_https_through_the_approved_proxy() {
    let Some(node) = runtime("MINICLAW_TEST_NODE") else { return };
    // Node's OpenSSL stops when its default configuration file exists somewhere it may not read;
    // an empty file the run can read stands in for it.
    let script = |url: &str| format!("OPENSSL_CONF=\"$GIT_CONFIG_GLOBAL\" NODE_USE_ENV_PROXY=1 {} -e 'fetch(\"{url}\").then(r=>console.log(\"STATUS \"+r.status)).catch(e=>{{console.log(\"ERROR \"+e.message+\" \"+(e.cause&&e.cause.code));process.exit(3)}})'", quoted(&node));
    let fetched = shell(&script("https://example.com/"), &["example.com"], node.parent()).await;
    assert_eq!(fetched["exitCode"], 0, "{fetched}");
    assert!(fetched["text"].as_str().unwrap().contains("STATUS 200"), "{fetched}");
    let refused = shell(&script("https://www.iana.org/"), &["example.com"], node.parent()).await;
    assert_eq!(refused["exitCode"], 3, "{refused}");
    assert!(refused["refusedHosts"].as_array().unwrap().iter().any(|host| host.as_str().unwrap().contains("iana.org")), "{refused}");
}

#[tokio::test]
#[ignore = "requires the installed network service, a Python runtime and internet access to example.com"]
/// Python (x64 under emulation on ARM64) makes an HTTPS request through the proxy. Its default
/// trust store is the Windows certificate store, which the sandbox cannot read, so the result
/// with the default store is reported and the request is asserted with an explicit CA file.
async fn python_fetches_https_through_the_approved_proxy() {
    let Some(python) = runtime("MINICLAW_TEST_PYTHON") else { return };
    let program = "import platform, sys, urllib.request\nprint(\"PYTHON\", sys.version.split()[0], platform.machine())\ntry:\n    print(\"STATUS\", urllib.request.urlopen(sys.argv[1], timeout=30).status)\nexcept Exception as error:\n    print(\"ERROR\", type(error).__name__, error)\n    sys.exit(3)\n";
    let script = |prefix: &str, url: &str| format!("PYTHONDONTWRITEBYTECODE=1 {prefix}{} -c '{program}' {url}", quoted(&python));
    let default_store = shell(&script("", "https://example.com/"), &["example.com"], python.parent()).await;
    println!("PYTHON DEFAULT TRUST STORE: exit={} {}", default_store["exitCode"], default_store["text"]);
    assert!(default_store["text"].as_str().unwrap().contains("PYTHON 3"), "Python did not start in the sandbox: {default_store}");
    let fetched = shell(&script("SSL_CERT_FILE=\"$GIT_SSL_CAINFO\" ", "https://example.com/"), &["example.com"], python.parent()).await;
    assert_eq!(fetched["exitCode"], 0, "{fetched}");
    assert!(fetched["text"].as_str().unwrap().contains("STATUS 200"), "{fetched}");
    let refused = shell(&script("SSL_CERT_FILE=\"$GIT_SSL_CAINFO\" ", "https://www.iana.org/"), &["example.com"], python.parent()).await;
    assert_eq!(refused["exitCode"], 3, "{refused}");
    assert!(refused["refusedHosts"].as_array().unwrap().iter().any(|host| host.as_str().unwrap().contains("iana.org")), "{refused}");
}

#[tokio::test]
#[ignore = "diagnostic: reports which runtimes and shell network tools start inside the sandbox"]
/// Prints, without asserting, whether each available runtime starts in an offline run and what the
/// pinned shell's own network tools report in a networked run.
async fn sandbox_runtime_report() {
    for (variable, arguments) in [("MINICLAW_TEST_NODE", "--version"), ("MINICLAW_TEST_NODE_X64", "--version"), ("MINICLAW_TEST_PYTHON", "--version"), ("MINICLAW_TEST_PYTHON_ARM64", "--version")] {
        let Some(program) = runtime(variable) else { continue };
        // Started directly, without the shell, so the Windows error is not translated.
        let work = std::env::temp_dir().join(format!("mc-tools-direct-{}", uuid::Uuid::new_v4()));
        platform_fs::private_dir(&work, false).unwrap();
        let grants = vec![Grant { path: work.to_string_lossy().into_owned(), folder: true, write: true, blocked: false },
            Grant { path: program.parent().unwrap().to_string_lossy().into_owned(), folder: true, write: false, blocked: false }];
        let sandbox = windows_sandbox::Sandbox::new(&grants, &work).unwrap();
        let env: Vec<_> = sandbox.environment().into_iter().map(|(key, value)| (key.into_string().unwrap(), value.into_string().unwrap())).collect();
        let direct = miniclaw_server::windows_process::capture_restricted(&program, &[arguments.to_owned()], &work, &env, &sandbox.security()).await;
        match direct {
            Ok((output, errors, status)) => println!("REPORT {variable} direct offline: exit={status} out={:?} err={:?}", String::from_utf8_lossy(&output), String::from_utf8_lossy(&errors).chars().take(300).collect::<String>()),
            Err(error) => println!("REPORT {variable} direct offline: could not start: {error:?}"),
        }
        drop(sandbox);
        windows_sandbox::drain_cleanup(Duration::from_secs(60)).unwrap();
        let _ = fs::remove_dir_all(work);
        let through_shell = shell(&format!("{} {arguments}", quoted(&program)), &[], program.parent()).await;
        println!("REPORT {variable} shell offline: {through_shell}");
    }
    let tools = shell("port=${HTTP_PROXY##*:}; echo proxy=$HTTP_PROXY; printf 'CONNECT probe.invalid:443 HTTP/1.1\\r\\n\\r\\n' | nc 127.0.0.1 $port; echo nc-exit=$?; nc 127.0.0.1 $port </dev/null; echo nc-null-exit=$?; nc 127.0.0.1 9 </dev/null; echo nc-denied-exit=$?; hostname; echo hostname-exit=$?", &["example.com"], None).await;
    println!("REPORT shell network tools: {tools}");
}
