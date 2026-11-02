//! Key material kept in its own locked, zeroed-on-drop memory page (#24).
//!
//! Each [`Secret32`] gets a whole page from the OS (`mmap` / `VirtualAlloc`)
//! that is locked into RAM (`mlock` / `VirtualLock`), so the bytes never reach
//! swap or the pagefile, and is wiped before it's freed. A page of its own
//! means unlocking one secret never unlocks another that shared its page.
//!
//! Locking can fail (e.g. `RLIMIT_MEMLOCK` on Linux); the secret then still
//! works and is still wiped, and the failure is logged once.

use std::fmt;
use std::sync::atomic::{AtomicBool, Ordering};

use zeroize::Zeroize;

/// 32 bytes of key material in locked memory.
pub struct Secret32 {
    // Field order matters: the lock guard drops (unlocks) before the allocation is freed.
    lock: Option<region::LockGuard>,
    page: Page,
}

/// An owned page allocation. Only ever reached through `&Secret32` / `&mut Secret32`.
struct Page(region::Allocation);

// SAFETY: the allocation is plain memory owned exclusively by its `Secret32`;
// shared access only reads it, writes need `&mut`.
unsafe impl Send for Page {}
// SAFETY: as above.
unsafe impl Sync for Page {}

static LOCK_FAILED: AtomicBool = AtomicBool::new(false);

impl Secret32 {
    /// A zeroed secret. Fill it with [`Secret32::expose_mut`].
    pub fn zeroed() -> Self {
        let allocation = region::alloc(32, region::Protection::READ_WRITE)
            .unwrap_or_else(|err| panic!("couldn't allocate memory for a secret: {err}"));
        let lock = region::lock(allocation.as_ptr::<u8>(), allocation.len()).or_else(|err| {
            // Windows caps locked pages at the process's minimum working set; raise it and retry.
            if grow_working_set(allocation.len()) {
                region::lock(allocation.as_ptr::<u8>(), allocation.len())
            } else {
                Err(err)
            }
        });
        let lock = match lock {
            Ok(guard) => Some(guard),
            Err(err) => {
                if !LOCK_FAILED.swap(true, Ordering::Relaxed) {
                    tracing::warn!(error = %err, "couldn't lock key memory in RAM; keys may reach swap");
                }
                None
            }
        };
        // Fresh pages from the OS are zeroed.
        Self { lock, page: Page(allocation) }
    }

    /// 32 random bytes from the OS.
    pub fn random() -> Self {
        let mut secret = Self::zeroed();
        crate::fill_random(secret.expose_mut());
        secret
    }

    /// Moves `bytes` into locked memory and wipes the original.
    pub fn from_bytes(bytes: &mut [u8; 32]) -> Self {
        let mut secret = Self::zeroed();
        secret.expose_mut().copy_from_slice(bytes);
        bytes.zeroize();
        secret
    }

    /// Copies `bytes` into locked memory. The caller wipes its copy.
    pub fn from_slice(bytes: &[u8]) -> Option<Self> {
        let bytes: &[u8; 32] = bytes.try_into().ok()?;
        let mut secret = Self::zeroed();
        secret.expose_mut().copy_from_slice(bytes);
        Some(secret)
    }

    pub fn expose(&self) -> &[u8; 32] {
        // SAFETY: the allocation is at least one page (>= 32 bytes), page-aligned,
        // readable and writable, and lives as long as `self`.
        unsafe { &*self.page.0.as_ptr::<[u8; 32]>() }
    }

    pub fn expose_mut(&mut self) -> &mut [u8; 32] {
        // SAFETY: as in `expose`, and `&mut self` makes this the only reference.
        unsafe { &mut *self.page.0.as_mut_ptr::<[u8; 32]>() }
    }

    /// Whether the page is locked in RAM.
    pub fn is_locked(&self) -> bool {
        self.lock.is_some()
    }

    pub fn duplicate(&self) -> Self {
        let mut copy = Self::zeroed();
        copy.expose_mut().copy_from_slice(self.expose());
        copy
    }
}

/// Raises the process's minimum (and maximum) working set by `bytes` plus some room,
/// so `VirtualLock` can lock another page. Returns whether it did.
#[cfg(windows)]
fn grow_working_set(bytes: usize) -> bool {
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessWorkingSetSize, SetProcessWorkingSetSize};
    let (mut min, mut max) = (0usize, 0usize);
    // SAFETY: the pseudo-handle of the current process and pointers to two locals.
    unsafe {
        let process = GetCurrentProcess();
        if GetProcessWorkingSetSize(process, &mut min, &mut max).is_err() {
            return false;
        }
        let grow = bytes.saturating_add(64 * 1024);
        SetProcessWorkingSetSize(process, min.saturating_add(grow), max.saturating_add(grow)).is_ok()
    }
}

#[cfg(not(windows))]
fn grow_working_set(_bytes: usize) -> bool {
    false
}

impl Drop for Secret32 {
    fn drop(&mut self) {
        self.expose_mut().zeroize();
        // Then `lock` unlocks and `page` frees, in field order.
    }
}

impl PartialEq for Secret32 {
    /// Constant-time comparison.
    fn eq(&self, other: &Self) -> bool {
        use subtle::ConstantTimeEq;
        self.expose().ct_eq(other.expose()).into()
    }
}

impl Eq for Secret32 {}

impl fmt::Debug for Secret32 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Secret32(redacted)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_wipes_and_locks() {
        let mut source = [7u8; 32];
        let secret = Secret32::from_bytes(&mut source);
        assert_eq!(source, [0; 32], "the original is wiped");
        assert_eq!(secret.expose(), &[7u8; 32]);
        assert_eq!(secret.duplicate(), secret);
        assert_ne!(Secret32::random(), Secret32::random());
        // Locking a single page fits every default limit (Linux RLIMIT_MEMLOCK, Windows working set).
        assert!(secret.is_locked(), "page wasn't locked");
        assert_eq!(format!("{secret:?}"), "Secret32(redacted)");
    }
}
