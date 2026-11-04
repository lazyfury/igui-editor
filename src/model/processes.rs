//! Process registry for native children that must die on AppKit termination.
//!
//! AppKit's terminate path can call `exit()` without unwinding Rust
//! destructors, so `Drop` is not enough for LSP servers and PTY shells. The
//! macOS bridge calls [`kill_all`] synchronously before returning
//! `NSTerminateNow` (or before replying to a deferred quit).

use std::sync::Mutex;

static CHILDREN: Mutex<Vec<u32>> = Mutex::new(Vec::new());

pub fn register(pid: u32) {
    if let Ok(mut children) = CHILDREN.lock() {
        if !children.contains(&pid) {
            children.push(pid);
        }
    }
}

pub fn unregister(pid: u32) {
    if let Ok(mut children) = CHILDREN.lock() {
        children.retain(|candidate| *candidate != pid);
    }
}

/// Kills every registered child. Used only on the quit path.
pub fn kill_all() {
    let pids: Vec<u32> = match CHILDREN.lock() {
        Ok(mut children) => std::mem::take(&mut *children),
        Err(error) => std::mem::take(&mut *error.into_inner()),
    };
    for pid in pids {
        kill(pid);
    }
}

#[cfg(unix)]
fn kill(pid: u32) {
    let _ = std::process::Command::new("kill")
        .args(["-9", &pid.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(windows)]
fn kill(pid: u32) {
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

#[cfg(not(any(unix, windows)))]
fn kill(_pid: u32) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn kill_all_terminates_a_registered_child() {
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();
        register(child.id());
        kill_all();
        let status = child.wait().unwrap();
        assert!(!status.success());
        unregister(child.id());
    }
}
