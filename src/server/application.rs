// SPDX-License-Identifier: GPL-3.0-or-later
use anyhow::{Result, bail};
use std::{collections::HashMap, io};

pub(super) fn use_x11(filename: &str, entry_backend: Option<&str>) -> Result<bool> {
    let policies: HashMap<String, String> =
        serde_json::from_str(include_str!("../application-backends.json"))?;
    match entry_backend.or_else(|| policies.get(filename).map(String::as_str)) {
        Some("x11") => Ok(true),
        None | Some("wayland") => Ok(false),
        Some(value) => bail!("unsupported application backend: {value}"),
    }
}

/// Runs only in the child between fork and exec; use raw, allocation-free calls.
pub(super) fn clear_capabilities() -> io::Result<()> {
    #[repr(C)]
    struct Header {
        version: u32,
        pid: i32,
    }
    // Linux capability ABI v3 uses two 32-bit effective/permitted/inheritable
    // words. Clearing all sets also removes ambient capabilities. The bounding
    // set and privilege acquisition through ordinary system helpers are intact.
    let header = Header {
        version: 0x2008_0522,
        pid: 0,
    };
    let data = [[0u32; 3]; 2];
    let result = unsafe { libc::syscall(libc::SYS_capset, &header, data.as_ptr()) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Exact desktop/app IDs and namespace suffixes cover single-instance activation.
pub(super) fn same_app(desktop: &str, app: &str) -> bool {
    let desktop = desktop.trim_end_matches(".desktop").to_lowercase();
    let app = app.to_lowercase();
    !app.is_empty() && (desktop == app || desktop.rsplit('.').next() == Some(app.as_str()))
}
