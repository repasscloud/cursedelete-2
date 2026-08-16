//! Local device identity for licence activation/verification.
//!
//! The wire format only requires two things to interoperate with the
//! server: the literal `scheme` string `"os-machine-id-sha256-v1"`
//! (checked by [`crate::schema::optional_device_binding`], mirroring the
//! server's own schema check), and a 64-character hex `deviceId`. The
//! *hash formula* itself does not need to match `Licensing.Core`'s C#
//! implementation byte-for-byte -- unlike the signature envelope, the
//! device ID is an opaque, client-chosen value: the server only ever
//! stores and echoes back whatever the client sent during activation, it
//! never recomputes it. What matters is that this Rust implementation is
//! deterministic per machine (so a later verification recomputes the same
//! value that was sent at activation time) and that different machines
//! produce different values with overwhelming probability (so a licence
//! can't be silently reused elsewhere without an explicit deactivate/
//! reactivate). See `docs/adr/0004-licensing-integration.md`.
//!
//! This is also an opportunity to do *better* than the reference
//! implementation on macOS: `Licensing.Core`'s own doc comments flag its
//! macOS behaviour as a weak fallback to `Environment.MachineName` (a
//! trivially user-changeable string). This implementation instead uses
//! `IOPlatformUUID`, a real hardware-tied stable identifier that survives
//! a user renaming their Mac and even most OS reinstalls.

use sha2::{Digest, Sha256};

pub const DEVICE_SCHEME: &str = "os-machine-id-sha256-v1";
const NAMESPACE: &str = "CurseDelete.DeviceIdentity.v1";

#[derive(Debug, Clone)]
pub struct LocalDeviceIdentity {
    pub scheme: &'static str,
    pub device_id: String,
    pub device_name: String,
    /// Which stable identifier was actually used. Explicit so a security
    /// reviewer -- or a support agent debugging an activation issue -- can
    /// see immediately whether this device is bound by a strong
    /// (`*-machine-id`/`*-platform-uuid`) or weak (`machine-name-fallback`)
    /// identifier.
    pub source: &'static str,
}

pub fn current() -> LocalDeviceIdentity {
    let (source, stable_value) = read_stable_identifier();
    let normalized = stable_value.trim().to_uppercase();
    let mut hasher = Sha256::new();
    hasher.update(format!("{NAMESPACE}\n{source}\n{normalized}").as_bytes());
    let digest = hasher.finalize();

    LocalDeviceIdentity {
        scheme: DEVICE_SCHEME,
        device_id: hex::encode_upper(digest),
        device_name: hostname(),
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
    fn produces_a_valid_64_char_hex_device_id() {
        let identity = current();
        assert_eq!(identity.scheme, DEVICE_SCHEME);
        assert!(crate::schema::is_valid_device_id(&identity.device_id));
    }

    #[test]
    fn is_deterministic_across_calls_on_the_same_machine() {
        let a = current();
        let b = current();
        assert_eq!(a.device_id, b.device_id);
    }
}
