//! Selects the native platform engine for the host OS at compile time.
//! Exactly one of these branches is ever compiled per target -- there is
//! no runtime dispatch or lowest-common-denominator fallback engine.

use cursdel_core::engine::PlatformEngine;

#[cfg(target_os = "macos")]
pub fn make_engine() -> Box<dyn PlatformEngine> {
    Box::new(cursdel_macos::MacEngine)
}

#[cfg(target_os = "linux")]
pub fn make_engine() -> Box<dyn PlatformEngine> {
    Box::new(cursdel_linux::LinuxEngine)
}

#[cfg(windows)]
pub fn make_engine() -> Box<dyn PlatformEngine> {
    Box::new(cursdel_windows::WindowsEngine)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub fn make_engine() -> Box<dyn PlatformEngine> {
    compile_error!("cursdel has no native deletion engine for this target platform");
}
