//! Exact non-interactive PowerShell invocation shared by Windows command execution.

// Encodes exact scripts without shell quoting.
use base64::Engine;
// Reports invalid scripts before process creation.
use std::io;

/// Builds UTF-16LE encoded arguments and fixes output encoding without loading profiles.
pub fn arguments(script: &str) -> io::Result<Vec<String>> {
    if script.is_empty() || script.len() > 16_384 || script.contains('\0') {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let prepared = format!("[Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false); $OutputEncoding = [Console]::OutputEncoding;\n{script}");
    let bytes: Vec<u8> = prepared.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    // Leave room for the executable path and fixed arguments below CreateProcessW’s limit.
    if encoded.len() > 30_000 { return Err(io::ErrorKind::InvalidInput.into()); }
    Ok(vec!["-NoLogo".into(), "-NoProfile".into(), "-NonInteractive".into(),
        "-InputFormat".into(), "Text".into(), "-OutputFormat".into(), "Text".into(),
        // Windows PowerShell ignores OutputFormat for redirected errors in EncodedCommand mode.
        // Keep the payload encoded, but invoke it through the ordinary textual host mode.
        "-Command".into(), format!("& ([scriptblock]::Create([Text.Encoding]::Unicode.GetString([Convert]::FromBase64String('{encoded}'))))")])
}

/// Routes Windows PowerShell 5 HTTP requests through the approved loopback proxy.
/// Its .NET Framework web stack does not honor PowerShell 7's proxy environment support.
pub fn proxy_script(port: u16, script: &str) -> io::Result<String> {
    if port == 0 { return Err(io::ErrorKind::InvalidInput.into()); }
    Ok(format!("$mcProxy=[Net.WebProxy]::new('http://127.0.0.1:{port}'); $mcProxy.BypassProxyOnLocal=$false; $mcProxy.Credentials=$null; [Net.WebRequest]::DefaultWebProxy=$mcProxy;\n{script}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_unicode_and_metacharacters_without_shell_interpolation() {
        let script = "Write-Output 'a & b; $x ` ✓'\nexit 7";
        let args = arguments(script).unwrap();
        assert_eq!(&args[..5], &["-NoLogo", "-NoProfile", "-NonInteractive", "-InputFormat", "Text"]);
        assert_eq!(&args[5..7], &["-OutputFormat", "Text"]);
        assert_eq!(args[7], "-Command");
        use base64::Engine;
        let encoded = args[8].strip_prefix("& ([scriptblock]::Create([Text.Encoding]::Unicode.GetString([Convert]::FromBase64String('")
            .and_then(|value| value.strip_suffix("'))))")).unwrap();
        let bytes = base64::engine::general_purpose::STANDARD.decode(encoded).unwrap();
        let decoded = String::from_utf16(&bytes.chunks_exact(2).map(|v| u16::from_le_bytes([v[0],v[1]])).collect::<Vec<_>>()).unwrap();
        assert!(decoded.ends_with(script));
        assert!(decoded.contains("UTF8Encoding"));
    }

    #[test]
    fn configures_legacy_powershell_proxy_without_implicit_bypass_or_credentials() {
        let script = proxy_script(8123, "Write-Output 'kept'").unwrap();
        assert!(script.contains("http://127.0.0.1:8123"));
        assert!(script.contains("BypassProxyOnLocal=$false"));
        assert!(script.contains("Credentials=$null"));
        assert!(script.ends_with("Write-Output 'kept'"));
        assert!(proxy_script(0, "x").is_err());
    }

    #[test]
    fn refuses_empty_nul_and_oversized_commands() {
        for script in [String::new(), "x\0y".into(), "x".repeat(16_385), "x".repeat(12_000)] {
            assert!(arguments(&script).is_err());
        }
    }
    #[cfg(windows)]
    #[tokio::test]
    async fn legacy_invoke_webrequest_uses_explicit_loopback_proxy() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let script = proxy_script(listener.local_addr().unwrap().port(),
            "(Invoke-WebRequest -UseBasicParsing -Uri 'http://miniclaw-proxy-test.invalid/probe' -TimeoutSec 10).Content").unwrap();
        let proxy = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            while !request.ends_with(b"\r\n\r\n") && request.len() < 8192 {
                request.push(socket.read_u8().await.unwrap());
            }
            assert!(String::from_utf8_lossy(&request).starts_with("GET http://miniclaw-proxy-test.invalid/probe HTTP/"));
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: 12\r\nConnection: close\r\n\r\nproxy-proven").await.unwrap();
        });
        let program = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        let args = arguments(&script).unwrap();
        let result = tokio::time::timeout(std::time::Duration::from_secs(20),
            crate::windows_process::capture(&program, &args, &std::env::temp_dir())).await;
        if result.is_err() { proxy.abort(); }
        let (stdout, stderr, code) = result.unwrap().unwrap();
        if code != 0 { proxy.abort(); }
        assert_eq!(code, 0, "{}", String::from_utf8_lossy(&stderr));
        assert!(String::from_utf8_lossy(&stdout).contains("proxy-proven"), "stdout={:?}; stderr={:?}", String::from_utf8_lossy(&stdout), String::from_utf8_lossy(&stderr));
        tokio::time::timeout(std::time::Duration::from_secs(1), proxy).await.unwrap().unwrap();
    }

}
