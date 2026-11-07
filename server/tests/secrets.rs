// Exercises disposable secrets without touching provider credentials.
use miniclaw_server::secrets::Secrets;

/// The disposable store replaces values and missing deletion succeeds.
#[test]
fn secret_round_trip() {
    let path = std::env::temp_dir().join(format!("mc-secrets-{}", uuid::Uuid::new_v4()));
    let secrets = Secrets::File(path.clone());
    assert_eq!(secrets.get("test").unwrap(), None);
    secrets.set("test", "one").unwrap();
    secrets.set("test", "two").unwrap();
    assert_eq!(secrets.get("test").unwrap().as_deref(), Some("two"));
    secrets.remove("test").unwrap();
    secrets.remove("test").unwrap();
    assert_eq!(secrets.get("test").unwrap(), None);
    std::fs::remove_file(path).unwrap();
}

/// Windows rejects oversized credentials before invoking Credential Manager.
#[cfg(windows)]
#[test]
fn windows_secret_limits() {
    let secrets = Secrets::Keychain;
    assert!(secrets.set("bad\0key", "value").is_err());
    assert!(secrets.set("limit-test", &"x".repeat(2561)).is_err());
}

/// Credential Manager owns only a fresh disposable credential during this test.
#[cfg(windows)]
#[test]
fn native_credential_round_trip() {
    let secrets = Secrets::Keychain;
    let key = format!("miniclaw-port-test-{}", uuid::Uuid::new_v4());
    assert_eq!(secrets.get(&key).unwrap(), None);
    if !logon_session_keeps_credentials() {
        // An elevated scheduled task on a machine with User Account Control on runs in a logon
        // session without a credential set: Windows answers ERROR_NO_SUCH_LOGON_SESSION (1312) to
        // any persisted credential (measured on the second test guest; the same account's ordinary
        // token passes below). There the store must say so and keep nothing.
        assert!(secrets.set(&key, "disposable-test-☃").is_err(), "a session without a credential set stores nothing");
        assert_eq!(secrets.get(&key).unwrap(), None);
        println!("CREDENTIALS this logon session has no credential set (1312); the round trip needs an ordinary token");
        return;
    }
    secrets.set(&key, "disposable-test-☃").unwrap();
    let actual = secrets.get(&key);
    let cleanup = secrets.remove(&key);
    assert_eq!(actual.unwrap().as_deref(), Some("disposable-test-☃"));
    cleanup.unwrap();
    secrets.remove(&key).unwrap();
}

/// Asks Windows directly whether this logon session can keep a persisted credential, with a
/// disposable name that is removed again. Only "no such logon session" counts as no.
#[cfg(windows)]
fn logon_session_keeps_credentials() -> bool {
    use windows_sys::Win32::{Foundation::GetLastError, Security::Credentials::*};
    let mut name: Vec<u16> = format!("MiniClaw/session-probe-{}", uuid::Uuid::new_v4()).encode_utf16().chain(Some(0)).collect();
    let mut credential: CREDENTIALW = unsafe { std::mem::zeroed() };
    credential.Type = CRED_TYPE_GENERIC;
    credential.TargetName = name.as_mut_ptr();
    credential.CredentialBlobSize = 1;
    credential.CredentialBlob = b"x".as_ptr().cast_mut();
    credential.Persist = CRED_PERSIST_LOCAL_MACHINE;
    if unsafe { CredWriteW(&credential, 0) } == 0 {
        let error = unsafe { GetLastError() };
        assert_eq!(error, 1312, "CredWriteW failed for another reason: {error}");
        return false;
    }
    unsafe { CredDeleteW(name.as_ptr(), CRED_TYPE_GENERIC, 0); }
    true
}
