//! Process-wide hardening, applied once at startup (#24).
//!
//! - **Linux:** `PR_SET_DUMPABLE = 0` (no core dumps, and other processes of
//!   the same user can no longer `ptrace` Pasta or read `/proc/<pid>/mem`) and
//!   a core-file size limit of 0.
//! - **macOS:** core-file size limit of 0. The hardened runtime of signed
//!   builds (#56) blocks debugger attachment.
//! - **Windows:** crash reports (WER) leave the heap out of their dumps.
//!
//! Debug builds skip it when `PASTA_DEBUGGABLE=1`, so a debugger can attach.

/// What was done, for the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub applied: Vec<&'static str>,
    pub failed: Vec<String>,
}

pub fn harden_process() -> Report {
    let mut report = Report { applied: Vec::new(), failed: Vec::new() };
    if cfg!(debug_assertions) && std::env::var_os("PASTA_DEBUGGABLE").is_some_and(|value| value == "1") {
        report.failed.push("skipped: PASTA_DEBUGGABLE=1".into());
        return report;
    }
    platform(&mut report);
    report
}

#[cfg(unix)]
fn platform(report: &mut Report) {
    let limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: plain syscall with a valid pointer to a local struct.
    if unsafe { libc::setrlimit(libc::RLIMIT_CORE, &limit) } == 0 {
        report.applied.push("core dumps disabled (RLIMIT_CORE=0)");
    } else {
        report.failed.push(format!("RLIMIT_CORE: {}", std::io::Error::last_os_error()));
    }
    #[cfg(target_os = "linux")]
    {
        // SAFETY: PR_SET_DUMPABLE takes one integer argument; the rest are ignored.
        if unsafe { libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0) } == 0 {
            report.applied.push("not dumpable or traceable by other processes (PR_SET_DUMPABLE=0)");
        } else {
            report.failed.push(format!("PR_SET_DUMPABLE: {}", std::io::Error::last_os_error()));
        }
    }
}

#[cfg(windows)]
fn platform(report: &mut Report) {
    use windows::Win32::System::ErrorReporting::{WER_FAULT_REPORTING_FLAG_NOHEAP, WerSetFlags};
    // SAFETY: sets a flag for the current process; no pointers involved.
    match unsafe { WerSetFlags(WER_FAULT_REPORTING_FLAG_NOHEAP) } {
        Ok(()) => report.applied.push("crash reports exclude the heap (WER NOHEAP)"),
        Err(err) => report.failed.push(format!("WerSetFlags: {err}")),
    }
}

#[cfg(not(any(unix, windows)))]
fn platform(report: &mut Report) {
    report.failed.push("no hardening on this platform".into());
}

/// Whether this process can be core-dumped / traced (Linux only; for tests and diagnostics).
#[cfg(target_os = "linux")]
pub fn is_dumpable() -> bool {
    // SAFETY: PR_GET_DUMPABLE takes no arguments.
    unsafe { libc::prctl(libc::PR_GET_DUMPABLE, 0, 0, 0, 0) != 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_without_failures() {
        let report = harden_process();
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert!(!report.applied.is_empty());
        #[cfg(target_os = "linux")]
        assert!(!is_dumpable(), "still dumpable / traceable");
    }
}
