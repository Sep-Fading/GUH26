// Checks port complements across every possible endpoint.
use miniclaw_server::windows_ports::denied_ranges;

#[test]
/// Separate denied intervals cover precisely the ports absent from the explicit allowlist.
fn exact_complement() {
    for allowed in [
        vec![],
        vec![1],
        vec![65535],
        vec![2, 3, 22, 443, 8080, 65534],
    ] {
        let denied = denied_ranges(&allowed);
        for port in 0..=u16::MAX {
            let blocked = denied
                .iter()
                .any(|(start, end)| (*start..=*end).contains(&port));
            assert_eq!(
                blocked,
                !allowed.contains(&port),
                "port {port} with {allowed:?}"
            );
        }
    }
}
