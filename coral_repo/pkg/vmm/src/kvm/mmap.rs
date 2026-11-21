//! RAII wrapper around anonymous and file-backed `mmap` regions.

#[cfg(target_arch = "x86_64")]
pub(crate) struct MmapGuard {
    pub(crate) ptr: *mut u8,
    pub(crate) len: usize,
}

#[cfg(target_arch = "x86_64")]
impl MmapGuard {
    pub(crate) fn anon_rw(len: usize) -> anyhow::Result<Self> {
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            anyhow::bail!(
                "mmap guest RAM ({len} bytes) failed: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(Self {
            ptr: ptr.cast(),
            len,
        })
    }

    pub(crate) fn fd_rw(fd: i32, len: usize) -> anyhow::Result<Self> {
        let ptr = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_SHARED,
                fd,
                0,
            )
        };
        if ptr == libc::MAP_FAILED {
            anyhow::bail!(
                "mmap kvm_run failed: {}",
                std::io::Error::last_os_error()
            );
        }
        Ok(Self {
            ptr: ptr.cast(),
            len,
        })
    }
}

#[cfg(target_arch = "x86_64")]
impl Drop for MmapGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = libc::munmap(self.ptr.cast(), self.len);
        }
    }
}
