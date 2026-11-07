//! Persistent AppContainer network restrictions installed by the authenticated service.
// Serializes recovery records without raw native pointers.
use serde::{Deserialize, Serialize};
// Owns filter engines and serializes exemption edits.
use std::{io, ptr};
// Installs package-specific WFP blocks and loopback exemptions.
use windows_sys::{
    Win32::{
        Foundation::*,
        NetworkManagement::{WindowsFilteringPlatform::*, WindowsFirewall::*},
        Security::{Authorization::*, *},
        System::{
            Memory::{GetProcessHeap, HeapFree},
            Rpc::RPC_C_AUTHN_WINNT,
        },
    },
    core::GUID,
};

const PROVIDER: GUID = GUID::from_u128(0xf365cf8b_f28b_466e_b7ad_3c159d5a5fe4);
const SUBLAYER: GUID = GUID::from_u128(0x60972bdd_c6b2_459b_a129_56a6120e04de);
// Weight ranges inside the sublayer: exact filters are evaluated before the listener catch-all block.
const EXACT_WEIGHT: u8 = 15;
const CATCH_ALL_WEIGHT: u8 = 14;

/// The private service journal retains these IDs until it proves the sandbox job has exited.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub package_sid: String,
    pub filter_ids: Vec<u64>,
}

struct Engine(HANDLE);
impl Drop for Engine {
    /// Closes this management connection without removing persistent deny filters.
    fn drop(&mut self) {
        unsafe {
            FwpmEngineClose0(self.0);
        }
    }
}
struct Sid(PSID);
impl Drop for Sid {
    /// Releases the SID converted from its validated string representation.
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}

/// A persistent deny policy. Dropping it never opens network access.
pub struct Lease {
    engine: Engine,
    sid: Sid,
    record: Record,
    inbound: Option<InboundHelper>,
}
// Native handles are owned and operations are serialized by the service lease owner.
unsafe impl Send for Lease {}

impl Lease {
    /// Commits persistent blocks before granting access to one IPv4 TCP loopback proxy.
    pub fn install(package_sid: &str, proxy_port: u16) -> io::Result<Self> {
        Self::install_ports(package_sid, proxy_port, &[], &[])
    }

    /// Adds separate reviewed local-client and local-listener ports without allowing LAN traffic.
    pub fn install_ports(
        package_sid: &str,
        proxy_port: u16,
        connect_ports: &[u16],
        listen_ports: &[u16],
    ) -> io::Result<Self> {
        Self::install_ports_recorded(package_sid, proxy_port, connect_ports, listen_ports, |_| {
            Ok(())
        })
    }

    /// Persists filter identity before committing policy or granting any loopback exemption.
    pub fn install_ports_recorded(
        package_sid: &str,
        proxy_port: u16,
        connect_ports: &[u16],
        listen_ports: &[u16],
        save: impl FnOnce(&Record) -> io::Result<()>,
    ) -> io::Result<Self> {
        for ports in [connect_ports, listen_ports] {
            if ports.len() > 8
                || ports.contains(&0)
                || ports.windows(2).any(|pair| pair[0] >= pair[1])
            {
                return Err(io::ErrorKind::InvalidInput.into());
            }
        }
        if proxy_port == 0 {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let sid = sid(package_sid)?;
        let engine = engine()?;
        provider(&engine)?;
        let mut lease = Self {
            engine,
            sid,
            inbound: None,
            record: Record {
                package_sid: package_sid.into(),
                filter_ids: Vec::new(),
            },
        };
        status(unsafe { FwpmTransactionBegin0(lease.engine.0, 0) })?;
        let result = lease.add_blocks(proxy_port, connect_ports, listen_ports);
        if let Err(error) = result {
            unsafe {
                FwpmTransactionAbort0(lease.engine.0);
            }
            return Err(error);
        }
        if let Err(error) = save(&lease.record) {
            unsafe {
                FwpmTransactionAbort0(lease.engine.0);
            }
            return Err(error);
        }
        if let Err(error) = status(unsafe { FwpmTransactionCommit0(lease.engine.0) }) {
            unsafe {
                FwpmTransactionAbort0(lease.engine.0);
            }
            return Err(error);
        }
        // On exemption failure the already-committed blocks deliberately remain.
        exemption(lease.sid.0, true)?;
        Ok(lease)
    }

    /// Starts only the fixed Windows inbound-loopback utility after SID-bound filters exist.
    pub fn start_inbound(&mut self, profile: &str) -> io::Result<()> {
        if self.inbound.is_some() {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        let suffix = profile
            .strip_prefix("MiniClaw.Run.")
            .ok_or(io::ErrorKind::InvalidInput)?;
        if suffix.len() != 32 || !suffix.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let mut derived = ptr::null_mut();
        let result = unsafe {
            Isolation::DeriveAppContainerSidFromAppContainerName(
                wide(profile).as_ptr(),
                &mut derived,
            )
        };
        if result < 0 {
            return Err(io::Error::other(format!(
                "Profile SID derivation failed: {result:#x}"
            )));
        }
        let equal = unsafe { EqualSid(derived, self.sid.0) } != 0;
        unsafe {
            FreeSid(derived);
        }
        if !equal {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        if !self.installed()? {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let mut directory = vec![0u16; 32768];
        let length = unsafe {
            windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW(
                directory.as_mut_ptr(),
                directory.len() as u32,
            )
        };
        if length == 0 || length as usize >= directory.len() {
            return Err(io::Error::last_os_error());
        }
        let directory = std::path::PathBuf::from(
            String::from_utf16(&directory[..length as usize]).map_err(io::Error::other)?,
        );
        let program = directory.join("CheckNetIsolation.exe");
        let args = vec![
            "LoopbackExempt".into(),
            "-is".into(),
            format!("-n={profile}"),
        ];
        let server = crate::windows_process::stdio(&program, &args, &directory, &[])?;
        let diagnostics = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        for mut stream in [server.output, server.errors] {
            let output = diagnostics.clone();
            tokio::task::spawn_blocking(move || {
                use std::io::Read;
                let mut buffer = [0u8; 1024];
                loop {
                    let Ok(count) = stream.read(&mut buffer) else {
                        break;
                    };
                    if count == 0 {
                        break;
                    }
                    if let Ok(mut output) = output.lock() {
                        let count = count.min(8192usize.saturating_sub(output.len()));
                        output.extend_from_slice(&buffer[..count]);
                    }
                }
            });
        }
        self.inbound = Some(InboundHelper {
            process: server.process,
            _input: server.input,
            diagnostics,
        });
        Ok(())
    }

    /// Reports utility exit and bounded diagnostics; process existence is not a readiness proof.
    pub fn inbound_status(&self) -> io::Result<Option<(Option<u32>, String)>> {
        self.inbound
            .as_ref()
            .map(|helper| {
                let status = helper.process.try_wait()?;
                let output = helper
                    .diagnostics
                    .lock()
                    .map_err(|_| io::Error::other("Helper diagnostic lock poisoned"))?;
                Ok((status, String::from_utf8_lossy(&output).into_owned()))
            })
            .transpose()
    }

    /// Reopens a journaled lease while verifying every surviving filter belongs to its SID.
    pub fn recover(record: Record) -> io::Result<Self> {
        if record.filter_ids.is_empty() || record.filter_ids.len() > 64 {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let lease = Self {
            sid: sid(&record.package_sid)?,
            engine: engine()?,
            record,
            inbound: None,
        };
        for id in &lease.record.filter_ids {
            if let Err(error) = lease.owned_filter(*id) {
                if error.raw_os_error() != Some(FWP_E_FILTER_NOT_FOUND) {
                    return Err(error);
                }
            }
        }
        Ok(lease)
    }

    /// Returns the bounded private journal record, never a raw handle or security token.
    pub fn record(&self) -> Record {
        self.record.clone()
    }

    /// Confirms that every deny filter still exists and belongs to this package.
    pub fn installed(&self) -> io::Result<bool> {
        if self.record.filter_ids.is_empty() {
            return Ok(false);
        }
        for id in &self.record.filter_ids {
            self.owned_filter(*id)?;
        }
        Ok(true)
    }

    /// Revokes exemption first, then removes filters after the caller proves the job dead.
    pub fn revoke(&mut self) -> io::Result<()> {
        if let Some(helper) = &self.inbound {
            helper.stop()?;
        }
        self.inbound = None;
        exemption(self.sid.0, false)?;
        for id in &self.record.filter_ids {
            if let Err(error) = self.owned_filter(*id) {
                if error.raw_os_error() != Some(FWP_E_FILTER_NOT_FOUND) {
                    return Err(error);
                }
            }
        }
        status(unsafe { FwpmTransactionBegin0(self.engine.0, 0) })?;
        for id in &self.record.filter_ids {
            if let Err(error) = status(unsafe { FwpmFilterDeleteById0(self.engine.0, *id) }) {
                if error.raw_os_error() == Some(FWP_E_FILTER_NOT_FOUND) {
                    continue;
                }
                unsafe {
                    FwpmTransactionAbort0(self.engine.0);
                }
                return Err(error);
            }
        }
        if let Err(error) = status(unsafe { FwpmTransactionCommit0(self.engine.0) }) {
            unsafe {
                FwpmTransactionAbort0(self.engine.0);
            }
            return Err(error);
        }
        self.record.filter_ids.clear();
        Ok(())
    }

    /// Blocks all IPv6 and every IPv4 endpoint outside separately reviewed directions.
    fn add_blocks(
        &mut self,
        port: u16,
        connect_ports: &[u16],
        listen_ports: &[u16],
    ) -> io::Result<()> {
        let package = FWPM_FILTER_CONDITION0 {
            fieldKey: FWPM_CONDITION_ALE_PACKAGE_ID,
            matchType: FWP_MATCH_EQUAL,
            conditionValue: FWP_CONDITION_VALUE0 {
                r#type: FWP_SID,
                Anonymous: FWP_CONDITION_VALUE0_0 {
                    sid: self.sid.0.cast(),
                },
            },
        };
        let address = |field_key| FWPM_FILTER_CONDITION0 {
            fieldKey: field_key,
            matchType: FWP_MATCH_NOT_EQUAL,
            conditionValue: FWP_CONDITION_VALUE0 {
                r#type: FWP_UINT32,
                Anonymous: FWP_CONDITION_VALUE0_0 { uint32: 0x7f000001 },
            },
        };
        let protocol = FWPM_FILTER_CONDITION0 {
            fieldKey: FWPM_CONDITION_IP_PROTOCOL,
            matchType: FWP_MATCH_NOT_EQUAL,
            conditionValue: FWP_CONDITION_VALUE0 {
                r#type: FWP_UINT8,
                Anonymous: FWP_CONDITION_VALUE0_0 { uint8: 6 },
            },
        };
        for condition in [address(FWPM_CONDITION_IP_REMOTE_ADDRESS), protocol] {
            self.add(FWPM_LAYER_ALE_AUTH_CONNECT_V4, &mut [package, condition])?;
        }
        let mut outbound = connect_ports.to_vec();
        outbound.push(port);
        self.block_ports(
            FWPM_LAYER_ALE_AUTH_CONNECT_V4,
            package,
            FWPM_CONDITION_IP_REMOTE_PORT,
            &outbound,
        )?;
        // Listening is default-deny. A wildcard bind is classified with an FWP_EMPTY local address
        // (Microsoft "ALE Layers"), which a "not loopback" block never matches, so the only way
        // through is an exact permit for 127.0.0.1 on a declared TCP port from a loopback peer.
        let exact = |field_key, r#type, value| FWPM_FILTER_CONDITION0 {
            fieldKey: field_key,
            matchType: FWP_MATCH_EQUAL,
            conditionValue: FWP_CONDITION_VALUE0 {
                r#type,
                Anonymous: value,
            },
        };
        let loopback = |field_key| {
            exact(
                field_key,
                FWP_UINT32,
                FWP_CONDITION_VALUE0_0 { uint32: 0x7f000001 },
            )
        };
        for layer in [
            FWPM_LAYER_ALE_AUTH_LISTEN_V4,
            FWPM_LAYER_ALE_AUTH_RECV_ACCEPT_V4,
        ] {
            self.add_weighted(layer, &mut [package], FWP_ACTION_BLOCK, CATCH_ALL_WEIGHT)?;
        }
        for listen in listen_ports {
            let local_port = exact(
                FWPM_CONDITION_IP_LOCAL_PORT,
                FWP_UINT16,
                FWP_CONDITION_VALUE0_0 { uint16: *listen },
            );
            self.add_weighted(
                FWPM_LAYER_ALE_AUTH_LISTEN_V4,
                &mut [package, loopback(FWPM_CONDITION_IP_LOCAL_ADDRESS), local_port],
                FWP_ACTION_PERMIT,
                EXACT_WEIGHT,
            )?;
            self.add_weighted(
                FWPM_LAYER_ALE_AUTH_RECV_ACCEPT_V4,
                &mut [
                    package,
                    loopback(FWPM_CONDITION_IP_LOCAL_ADDRESS),
                    loopback(FWPM_CONDITION_IP_REMOTE_ADDRESS),
                    exact(
                        FWPM_CONDITION_IP_PROTOCOL,
                        FWP_UINT8,
                        FWP_CONDITION_VALUE0_0 { uint8: 6 },
                    ),
                    local_port,
                ],
                FWP_ACTION_PERMIT,
                EXACT_WEIGHT,
            )?;
        }
        for layer in [
            FWPM_LAYER_ALE_AUTH_CONNECT_V6,
            FWPM_LAYER_ALE_AUTH_LISTEN_V6,
            FWPM_LAYER_ALE_AUTH_RECV_ACCEPT_V6,
        ] {
            self.add(layer, &mut [package])?;
        }
        Ok(())
    }

    /// Builds port-complement ranges instead of relying on duplicate-condition boolean rules.
    fn block_ports(
        &mut self,
        layer: GUID,
        package: FWPM_FILTER_CONDITION0,
        field: GUID,
        allowed: &[u16],
    ) -> io::Result<()> {
        for (start, end) in crate::windows_ports::denied_ranges(allowed) {
            let value = |port| FWP_VALUE0 {
                r#type: FWP_UINT16,
                Anonymous: FWP_VALUE0_0 { uint16: port },
            };
            let mut range = FWP_RANGE0 {
                valueLow: value(start),
                valueHigh: value(end),
            };
            let condition = FWPM_FILTER_CONDITION0 {
                fieldKey: field,
                matchType: FWP_MATCH_RANGE,
                conditionValue: FWP_CONDITION_VALUE0 {
                    r#type: FWP_RANGE_TYPE,
                    Anonymous: FWP_CONDITION_VALUE0_0 {
                        rangeValue: &mut range,
                    },
                },
            };
            self.add(layer, &mut [package, condition])?;
        }
        Ok(())
    }

    /// Stores one package-tagged block under MiniClaw's dedicated persistent sublayer.
    fn add(&mut self, layer: GUID, conditions: &mut [FWPM_FILTER_CONDITION0]) -> io::Result<()> {
        self.add_weighted(layer, conditions, FWP_ACTION_BLOCK, EXACT_WEIGHT)
    }

    /// Stores one package-tagged filter. A permit only exempts its exact match from this
    /// sublayer's lower-weight catch-all block; it cannot override blocks in other sublayers.
    fn add_weighted(
        &mut self,
        layer: GUID,
        conditions: &mut [FWPM_FILTER_CONDITION0],
        action: FWP_ACTION_TYPE,
        weight: u8,
    ) -> io::Result<()> {
        let mut name = wide("MiniClaw isolated process network");
        let mut provider = PROVIDER;
        let mut data = self.record.package_sid.as_bytes().to_vec();
        let filter = FWPM_FILTER0 {
            filterKey: GUID::from_u128(uuid::Uuid::new_v4().as_u128()),
            displayData: FWPM_DISPLAY_DATA0 {
                name: name.as_mut_ptr(),
                description: ptr::null_mut(),
            },
            flags: FWPM_FILTER_FLAG_PERSISTENT,
            providerKey: &mut provider,
            providerData: FWP_BYTE_BLOB {
                size: data.len() as u32,
                data: data.as_mut_ptr(),
            },
            layerKey: layer,
            subLayerKey: SUBLAYER,
            weight: FWP_VALUE0 {
                r#type: FWP_UINT8,
                Anonymous: FWP_VALUE0_0 { uint8: weight },
            },
            numFilterConditions: conditions.len() as u32,
            filterCondition: conditions.as_mut_ptr(),
            action: FWPM_ACTION0 {
                r#type: action,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut id = 0;
        status(unsafe { FwpmFilterAdd0(self.engine.0, &filter, ptr::null_mut(), &mut id) })?;
        self.record.filter_ids.push(id);
        Ok(())
    }

    /// Refuses journal IDs that point outside this provider and exact package record.
    fn owned_filter(&self, id: u64) -> io::Result<()> {
        unsafe {
            let mut filter = ptr::null_mut();
            status(FwpmFilterGetById0(self.engine.0, id, &mut filter))?;
            let data = &(*filter).providerData;
            let valid = !(*filter).providerKey.is_null()
                && guid_equal(*(*filter).providerKey, PROVIDER)
                && guid_equal((*filter).subLayerKey, SUBLAYER)
                && data.size as usize == self.record.package_sid.len()
                && !data.data.is_null()
                && std::slice::from_raw_parts(data.data, data.size as usize)
                    == self.record.package_sid.as_bytes();
            FwpmFreeMemory0((&mut filter as *mut *mut FWPM_FILTER0).cast());
            if !valid {
                return Err(io::ErrorKind::PermissionDenied.into());
            }
            Ok(())
        }
    }
}

struct InboundHelper {
    process: crate::windows_process::Process,
    _input: std::fs::File,
    diagnostics: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
}

impl InboundHelper {
    /// Waits for the fixed helper to stop before its package filters can be removed.
    fn stop(&self) -> io::Result<()> {
        self.process.kill()?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while self.process.try_wait()?.is_none() {
            if std::time::Instant::now() >= deadline {
                return Err(io::ErrorKind::TimedOut.into());
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        Ok(())
    }
}

/// Opens an authenticated non-dynamic engine so failures retain persistent restrictions.
fn engine() -> io::Result<Engine> {
    let mut handle = ptr::null_mut();
    status(unsafe {
        FwpmEngineOpen0(
            ptr::null(),
            RPC_C_AUTHN_WINNT,
            ptr::null(),
            ptr::null(),
            &mut handle,
        )
    })?;
    Ok(Engine(handle))
}

/// Creates persistent policy containers without replacing another provider's settings.
fn provider(engine: &Engine) -> io::Result<()> {
    let mut name = wide("MiniClaw network isolation");
    let display = FWPM_DISPLAY_DATA0 {
        name: name.as_mut_ptr(),
        description: ptr::null_mut(),
    };
    let provider = FWPM_PROVIDER0 {
        providerKey: PROVIDER,
        displayData: display,
        flags: FWPM_PROVIDER_FLAG_PERSISTENT,
        ..Default::default()
    };
    let result = unsafe { FwpmProviderAdd0(engine.0, &provider, ptr::null_mut()) };
    if result != FWP_E_ALREADY_EXISTS as u32 {
        status(result)?;
    }
    let mut key = PROVIDER;
    let layer = FWPM_SUBLAYER0 {
        subLayerKey: SUBLAYER,
        displayData: display,
        flags: FWPM_SUBLAYER_FLAG_PERSISTENT,
        providerKey: &mut key,
        weight: u16::MAX,
        ..Default::default()
    };
    let result = unsafe { FwpmSubLayerAdd0(engine.0, &layer, ptr::null_mut()) };
    if result != FWP_E_ALREADY_EXISTS as u32 {
        status(result)?;
    }
    Ok(())
}

/// Changes only the requested SID while preserving all other configured loopback exemptions.
fn exemption(sid: PSID, enabled: bool) -> io::Result<()> {
    let _lock = crate::windows_security_lock::acquire(true)?;
    unsafe {
        let mut count = 0;
        let mut allocation = ptr::null_mut();
        status(NetworkIsolationGetAppContainerConfig(
            &mut count,
            &mut allocation,
        ))?;
        let original = if count == 0 {
            &[][..]
        } else {
            std::slice::from_raw_parts(allocation, count as usize)
        };
        let mut entries: Vec<_> = original
            .iter()
            .copied()
            .filter(|entry| EqualSid(entry.Sid, sid) == 0)
            .collect();
        if enabled {
            entries.push(SID_AND_ATTRIBUTES {
                Sid: sid,
                Attributes: 0,
            });
        }
        let result = NetworkIsolationSetAppContainerConfig(entries.len() as u32, entries.as_ptr());
        for entry in original {
            HeapFree(GetProcessHeap(), 0, entry.Sid);
        }
        if !allocation.is_null() {
            HeapFree(GetProcessHeap(), 0, allocation.cast());
        }
        status(result)
    }
}

/// Parses only full AppContainer package SIDs, excluding users, capabilities and groups.
fn sid(text: &str) -> io::Result<Sid> {
    let parts: Vec<_> = text.split('-').collect();
    if parts.len() != 11
        || parts[..4] != ["S", "1", "15", "2"]
        || parts[4..].iter().any(|part| part.parse::<u32>().is_err())
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let mut sid = ptr::null_mut();
    if unsafe { ConvertStringSidToSidW(wide(text).as_ptr(), &mut sid) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(Sid(sid))
}

/// Converts a checked native status to a standard I/O result.
fn status(code: u32) -> io::Result<()> {
    if code == 0 {
        Ok(())
    } else {
        Err(io::Error::from_raw_os_error(code as i32))
    }
}
/// Encodes constant or validated text for a synchronous native call.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
/// Compares GUID values without relying on binding trait implementations.
fn guid_equal(left: GUID, right: GUID) -> bool {
    left.data1 == right.data1
        && left.data2 == right.data2
        && left.data3 == right.data3
        && left.data4 == right.data4
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads one uncommitted filter back as (layer, action, weight, [(field, match type)]).
    fn shape(lease: &Lease, id: u64) -> (GUID, FWP_ACTION_TYPE, u8, Vec<(GUID, FWP_MATCH_TYPE)>) {
        unsafe {
            let mut filter = ptr::null_mut();
            status(FwpmFilterGetById0(lease.engine.0, id, &mut filter)).unwrap();
            let value = &*filter;
            let conditions = std::slice::from_raw_parts(value.filterCondition, value.numFilterConditions as usize)
                .iter().map(|condition| (condition.fieldKey, condition.matchType)).collect();
            assert_eq!(value.weight.r#type, FWP_UINT8);
            let result = (value.layerKey, value.action.r#type, value.weight.Anonymous.uint8, conditions);
            FwpmFreeMemory0((&mut filter as *mut *mut FWPM_FILTER0).cast());
            result
        }
    }

    /// Wildcard listens carry an empty local address, so a "not loopback" block cannot stop them:
    /// declared-listener filters must be a catch-all block plus exact loopback permits.
    #[test]
    #[ignore = "requires an elevated Windows token; aborts its transaction and persists nothing"]
    fn declared_listener_is_default_deny_with_exact_loopback_permits() {
        for listen_ports in [&[][..], &[9876, 9877][..]] {
            let package_sid = "S-1-15-2-1-2-3-4-5-6-7";
            let engine = engine().unwrap();
            provider(&engine).unwrap();
            let mut lease = Lease { engine, sid: sid(package_sid).unwrap(), inbound: None,
                record: Record { package_sid: package_sid.into(), filter_ids: Vec::new() } };
            status(unsafe { FwpmTransactionBegin0(lease.engine.0, 0) }).unwrap();
            lease.add_blocks(8123, &[], listen_ports).unwrap();
            let shapes: Vec<_> = lease.record.filter_ids.iter().map(|id| shape(&lease, *id)).collect();
            unsafe { FwpmTransactionAbort0(lease.engine.0); }
            for layer in [FWPM_LAYER_ALE_AUTH_LISTEN_V4, FWPM_LAYER_ALE_AUTH_RECV_ACCEPT_V4,
                FWPM_LAYER_ALE_AUTH_LISTEN_V6, FWPM_LAYER_ALE_AUTH_RECV_ACCEPT_V6] {
                let inbound: Vec<_> = shapes.iter().filter(|shape| guid_equal(shape.0, layer)).collect();
                let blocks: Vec<_> = inbound.iter().filter(|shape| shape.1 == FWP_ACTION_BLOCK).collect();
                assert_eq!(blocks.len(), 1, "one catch-all block per inbound layer");
                assert_eq!(blocks[0].3.len(), 1, "the catch-all block is conditioned on the package only");
                assert!(guid_equal(blocks[0].3[0].0, FWPM_CONDITION_ALE_PACKAGE_ID));
                let permits: Vec<_> = inbound.iter().filter(|shape| shape.1 == FWP_ACTION_PERMIT).collect();
                let v4 = guid_equal(layer, FWPM_LAYER_ALE_AUTH_LISTEN_V4) || guid_equal(layer, FWPM_LAYER_ALE_AUTH_RECV_ACCEPT_V4);
                assert_eq!(permits.len(), if v4 { listen_ports.len() } else { 0 });
                assert_eq!(inbound.len(), blocks.len() + permits.len(), "only block and permit actions");
                for permit in permits {
                    assert!(permit.2 > blocks[0].2, "exact permits outrank the catch-all block");
                    assert!(permit.3.iter().all(|(_, kind)| *kind == FWP_MATCH_EQUAL), "permits use exact matches only");
                    for field in [FWPM_CONDITION_ALE_PACKAGE_ID, FWPM_CONDITION_IP_LOCAL_ADDRESS, FWPM_CONDITION_IP_LOCAL_PORT] {
                        assert!(permit.3.iter().any(|(key, _)| guid_equal(*key, field)));
                    }
                    if guid_equal(layer, FWPM_LAYER_ALE_AUTH_RECV_ACCEPT_V4) {
                        for field in [FWPM_CONDITION_IP_REMOTE_ADDRESS, FWPM_CONDITION_IP_PROTOCOL] {
                            assert!(permit.3.iter().any(|(key, _)| guid_equal(*key, field)));
                        }
                    }
                }
            }
            // No permit exists outside the two IPv4 inbound layers.
            assert!(shapes.iter().filter(|shape| shape.1 == FWP_ACTION_PERMIT).all(|shape|
                guid_equal(shape.0, FWPM_LAYER_ALE_AUTH_LISTEN_V4) || guid_equal(shape.0, FWPM_LAYER_ALE_AUTH_RECV_ACCEPT_V4)));
        }
    }
}
