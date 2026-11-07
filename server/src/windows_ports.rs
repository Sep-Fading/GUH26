//! Exact port complements for deny-first Windows network filters.

/// Returns disjoint inclusive ranges covering every port outside the allowlist.
pub fn denied_ranges(allowed: &[u16]) -> Vec<(u16, u16)> {
    let mut ports = allowed.to_vec();
    ports.sort_unstable();
    ports.dedup();
    let mut next = 0u32;
    let mut ranges = Vec::new();
    for port in ports {
        let port = u32::from(port);
        if next < port {
            ranges.push((next as u16, (port - 1) as u16));
        }
        next = port + 1;
    }
    if next <= u32::from(u16::MAX) {
        ranges.push((next as u16, u16::MAX));
    }
    ranges
}
