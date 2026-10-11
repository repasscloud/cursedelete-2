//! Local device identity for Business activation.
//!
//! The device id is an opaque, privacy-preserving value: a SHA-256 hash
//! of the OS's own stable machine identifier (Windows `MachineGuid`,
//! Linux `/etc/machine-id`, macOS `IOPlatformUUID`) under an
//! application-chosen namespace. It distinguishes installations without
//! sending hardware details, hostnames, or personal information -- the
//! raw identifier never leaves the machine. The licensing server only
//! stores and compares it; it never needs to recompute it.
//!
//! Only Business activation uses this. Community, Education, and
//! Enterprise licences never compute or send a device id.

use sha2::{Digest, Sha256};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalDeviceIdentity {
    /// 64 uppercase hex characters.
    pub device_id: String,
    /// Which stable identifier was used, e.g. `macos-platform-uuid`, or
    /// `machine-name-fallback` when none was available (a weak binding,
    /// shown by `license status` so support can recognise it).
    pub source: &'static str,
}

/// The identity of this machine under `namespace` (e.g.
/// `"CurseDelete.DeviceIdentity.v1"`). Deterministic per machine and
/// namespace, so different applications get unlinkable ids.
pub fn current(namespace: &str) -> LocalDeviceIdentity {
    let (source, stable_value) = read_stable_identifier();
    let normalized = stable_value.trim().to_uppercase();
    let mut hasher = Sha256::new();
    hasher.update(format!("{namespace}\n{source}\n{normalized}").as_bytes());
    LocalDeviceIdentity {
        device_id: hex::encode_upper(hasher.finalize()),
        source,
    }
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "unknown-host".to_string())
}

#[cfg(target_os = "windows")]
fn read_stable_identifier() -> (&'static str, String) {
    match read_windows_machine_guid() {
        Some(v) if !v.trim().is_empty() => ("windows-machine-guid", v),
        _ => ("machine-name-fallback", hostname()),
    }
}

#[cfg(target_os = "windows")]
fn read_windows_machine_guid() -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};

    let subkey: Vec<u16> = "SOFTWARE\\Microsoft\\Cryptography\0"
        .encode_utf16()
        .collect();
    let value_name: Vec<u16> = "MachineGuid\0".encode_utf16().collect();

    let mut buffer = [0u16; 64];
    let mut size = (buffer.len() * std::mem::size_of::<u16>()) as u32;

    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(subkey.as_ptr()),
            PCWSTR(value_name.as_ptr()),
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };

    if status != ERROR_SUCCESS {
        return None;
    }

    let chars_written = (size as usize / std::mem::size_of::<u16>()).min(buffer.len());
    let end = buffer[..chars_written]
        .iter()
        .position(|&c| c == 0)
        .unwrap_or(chars_written);
    Some(String::from_utf16_lossy(&buffer[..end]))
}

#[cfg(target_os = "linux")]
fn read_stable_identifier() -> (&'static str, String) {
    for path in ["/etc/machine-id", "/var/lib/dbus/machine-id"] {
        if let Ok(content) = std::fs::read_to_string(path) {
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                return ("linux-machine-id", trimmed.to_string());
            }
        }
    }
    ("machine-name-fallback", hostname())
}

#[cfg(target_os = "macos")]
fn read_stable_identifier() -> (&'static str, String) {
    match read_macos_platform_uuid() {
        Some(v) => ("macos-platform-uuid", v),
        None => ("machine-name-fallback", hostname()),
    }
}

#[cfg(target_os = "macos")]
fn read_macos_platform_uuid() -> Option<String> {
    let output = std::process::Command::new("ioreg")
        .args(["-rd1", "-c", "IOPlatformExpertDevice"])
        .output()
        .ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    for line in text.lines() {
        if let Some(idx) = line.find("IOPlatformUUID") {
            let after = &line[idx..];
            if let Some((_, rhs)) = after.split_once('=') {
                let value = rhs.trim().trim_matches('"');
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
    }
    None
}

#[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
fn read_stable_identifier() -> (&'static str, String) {
    ("machine-name-fallback", hostname())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn produces_a_64_char_uppercase_hex_device_id() {
        let id = current("Test.v1").device_id;
        assert_eq!(id.len(), 64);
        assert!(id
            .chars()
            .all(|c| c.is_ascii_digit() || c.is_ascii_uppercase()));
    }

    #[test]
    fn is_deterministic_and_namespaced() {
        assert_eq!(current("A.v1"), current("A.v1"));
        assert_ne!(current("A.v1").device_id, current("B.v1").device_id);
    }
}
