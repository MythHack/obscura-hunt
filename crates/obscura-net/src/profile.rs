//! Private, atomic profile storage shared by cookies and origin localStorage.
use std::io::{self, Write};
use std::path::Path;

pub fn private_directory(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)?;
    private_path(path, true)
}

pub fn private_file(path: &Path) -> io::Result<()> {
    private_path(path, false)
}

fn private_path(path: &Path, directory: bool) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "profile entry is not a regular file or directory"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = if directory { 0o700 } else { 0o600 };
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
        if std::fs::metadata(path)?.permissions().mode() & 0o777 != mode {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "private profile permissions were not established"));
        }
        Ok(())
    }
    #[cfg(windows)]
    { windows::private_path(path, directory) }
    #[cfg(not(any(unix, windows)))]
    { Err(io::Error::new(io::ErrorKind::Unsupported, "private profile permissions are unsupported on this platform")) }
}

pub fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().filter(|path| !path.as_os_str().is_empty()).unwrap_or(Path::new("."));
    // BrowserContext secures its explicitly managed directories. A manual
    // cookie export may name a file in /tmp or the working directory; never
    // repermission those unrelated existing parents.
    match std::fs::symlink_metadata(parent) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {},
        Ok(_) => return Err(io::Error::new(io::ErrorKind::InvalidInput, "profile parent is not a regular directory")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => private_directory(parent)?,
        Err(error) => return Err(error),
    }
    // Never replace a directory, symlink, or other special entry.
    match std::fs::symlink_metadata(path) {
        Ok(_) => private_file(path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {},
        Err(error) => return Err(error),
    }
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    private_file(temporary.path())?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    sync_directory(parent)
}

pub fn remove_file(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => private_file(path)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    }
    std::fs::remove_file(path)?;
    sync_directory(path.parent().unwrap_or(Path::new(".")))
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    { std::fs::File::open(path)?.sync_all() }
    // Windows replacement is performed by tempfile's MoveFileExW implementation.
    // File data has been flushed before replacement; directories cannot be
    // opened for FlushFileBuffers through the ordinary file API.
    #[cfg(not(unix))]
    { let _ = path; Ok(()) }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::null_mut;

    type Handle = *mut c_void;
    const DACL_SECURITY_INFORMATION: u32 = 4;
    const PROTECTED_DACL_SECURITY_INFORMATION: u32 = 0x8000_0000;
    const TOKEN_QUERY: u32 = 8;
    const TOKEN_USER: u32 = 1;
    const SDDL_REVISION_1: u32 = 1;

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
        fn GetTokenInformation(token: Handle, class: u32, data: *mut c_void, size: u32, needed: *mut u32) -> i32;
        fn ConvertSidToStringSidW(sid: *const c_void, text: *mut *mut u16) -> i32;
        fn ConvertStringSecurityDescriptorToSecurityDescriptorW(text: *const u16, revision: u32, descriptor: *mut *mut c_void, size: *mut u32) -> i32;
        fn ConvertSecurityDescriptorToStringSecurityDescriptorW(descriptor: *const c_void, revision: u32, information: u32, text: *mut *mut u16, length: *mut u32) -> i32;
        fn SetFileSecurityW(path: *const u16, information: u32, descriptor: *const c_void) -> i32;
        fn GetFileSecurityW(path: *const u16, information: u32, descriptor: *mut c_void, size: u32, needed: *mut u32) -> i32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> Handle;
        fn CloseHandle(handle: Handle) -> i32;
        fn LocalFree(memory: *mut c_void) -> *mut c_void;
    }

    struct LocalMemory(*mut c_void);
    impl Drop for LocalMemory {
        fn drop(&mut self) { unsafe { LocalFree(self.0); } }
    }
    struct Token(Handle);
    impl Drop for Token {
        fn drop(&mut self) { unsafe { CloseHandle(self.0); } }
    }

    unsafe fn wide_string(text: *const u16) -> Vec<u16> {
        let mut length = 0;
        while unsafe { *text.add(length) } != 0 { length += 1; }
        unsafe { std::slice::from_raw_parts(text, length) }.to_vec()
    }

    fn descriptor_text(descriptor: *const c_void) -> io::Result<Vec<u16>> {
        let mut text = null_mut();
        if unsafe { ConvertSecurityDescriptorToStringSecurityDescriptorW(
            descriptor, SDDL_REVISION_1, DACL_SECURITY_INFORMATION, &mut text, null_mut(),
        ) } == 0 { return Err(io::Error::last_os_error()); }
        let text = LocalMemory(text.cast());
        Ok(unsafe { wide_string(text.0.cast()) })
    }

    pub(super) fn private_path(path: &Path, directory: bool) -> io::Result<()> {
        // Reject reparse points as well as ordinary symlinks: applying an ACL to
        // a junction must not change a directory outside the managed profile.
        use std::os::windows::fs::MetadataExt;
        if std::fs::symlink_metadata(path)?.file_attributes() & 0x400 != 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "profile reparse points are not supported"));
        }
        let mut token = null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let token = Token(token);
        let mut needed = 0;
        unsafe { GetTokenInformation(token.0, TOKEN_USER, null_mut(), 0, &mut needed); }
        if needed == 0 { return Err(io::Error::last_os_error()); }
        // TOKEN_USER begins with SID_AND_ATTRIBUTES; pointer alignment is required.
        let mut user = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
        if unsafe { GetTokenInformation(token.0, TOKEN_USER, user.as_mut_ptr().cast(), needed, &mut needed) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut sid = null_mut();
        if unsafe { ConvertSidToStringSidW(user[0] as *const c_void, &mut sid) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let sid = LocalMemory(sid.cast());
        let sid = String::from_utf16(&unsafe { wide_string(sid.0.cast()) })
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid current-user SID"))?;
        let inheritance = if directory { "OICI" } else { "" };
        // Protected DACL: only the current user and LocalSystem. Do not inherit
        // Users/Everyone permissions from a permissive parent or mounted volume.
        let sddl: Vec<u16> = format!("D:P(A;{inheritance};FA;;;SY)(A;{inheritance};FA;;;{sid})")
            .encode_utf16().chain(Some(0)).collect();
        let mut descriptor = null_mut();
        if unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(), SDDL_REVISION_1, &mut descriptor, null_mut(),
        ) } == 0 { return Err(io::Error::last_os_error()); }
        let descriptor = LocalMemory(descriptor);
        let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        if unsafe { SetFileSecurityW(path.as_ptr(), DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, descriptor.0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut needed = 0;
        unsafe { GetFileSecurityW(path.as_ptr(), DACL_SECURITY_INFORMATION, null_mut(), 0, &mut needed); }
        if needed == 0 { return Err(io::Error::last_os_error()); }
        let mut actual = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
        if unsafe { GetFileSecurityW(path.as_ptr(), DACL_SECURITY_INFORMATION, actual.as_mut_ptr().cast(), needed, &mut needed) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if descriptor_text(actual.as_ptr().cast())? != descriptor_text(descriptor.0)? {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "private profile DACL was not established"));
        }
        Ok(())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn read_dacl(path: &Path) -> Vec<u16> {
            let path: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let mut needed = 0;
            unsafe { GetFileSecurityW(path.as_ptr(), DACL_SECURITY_INFORMATION, null_mut(), 0, &mut needed); }
            let mut descriptor = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
            assert_ne!(unsafe { GetFileSecurityW(path.as_ptr(), DACL_SECURITY_INFORMATION, descriptor.as_mut_ptr().cast(), needed, &mut needed) }, 0);
            descriptor_text(descriptor.as_ptr().cast()).unwrap()
        }

        #[test]
        fn permissive_parent_is_unchanged_and_profile_excludes_everyone() {
            let temporary = tempfile::tempdir().unwrap();
            let sddl: Vec<u16> = "D:P(A;OICI;FA;;;WD)".encode_utf16().chain(Some(0)).collect();
            let mut descriptor = null_mut();
            assert_ne!(unsafe { ConvertStringSecurityDescriptorToSecurityDescriptorW(sddl.as_ptr(), SDDL_REVISION_1, &mut descriptor, null_mut()) }, 0);
            let descriptor = LocalMemory(descriptor);
            let parent: Vec<u16> = temporary.path().as_os_str().encode_wide().chain(Some(0)).collect();
            assert_ne!(unsafe { SetFileSecurityW(parent.as_ptr(), DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, descriptor.0) }, 0);
            let before = read_dacl(temporary.path());
            let root = temporary.path().join("profile");
            super::super::private_directory(&root).unwrap();
            let file = root.join("cookies.json");
            super::super::write_atomically(&file, b"secret").unwrap();
            let file_path: Vec<u16> = file.as_os_str().encode_wide().chain(Some(0)).collect();
            assert_ne!(unsafe { SetFileSecurityW(file_path.as_ptr(), DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION, descriptor.0) }, 0);
            super::super::write_atomically(&file, b"updated secret").unwrap();
            assert_eq!(read_dacl(temporary.path()), before);
            for path in [&root, &file] {
                let acl = String::from_utf16(&read_dacl(path)).unwrap();
                assert!(acl.starts_with("D:P"), "profile DACL must block inherited broad grants");
                assert!(!acl.contains(";;;WD)"));
                assert!(!acl.contains(";;;BU)"));
                assert!(acl.contains(";;;SY)"));
                assert_eq!(acl.matches("(A;").count(), 2, "only current user and LocalSystem may access profile secrets");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacement_and_removal_keep_private_files_without_repermissioning_parent() {
        let temporary = tempfile::tempdir().unwrap();
        let unrelated = temporary.path();
        #[cfg(unix)]
        let parent_mode = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(unrelated, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::metadata(unrelated).unwrap().permissions().mode()
        };
        let root = unrelated.join("profile");
        private_directory(&root).unwrap();
        let file = root.join("cookies.json");
        write_atomically(&file, b"old session").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        write_atomically(&file, b"new session").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"new session");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(unrelated).unwrap().permissions().mode(), parent_mode);
            assert_eq!(std::fs::metadata(&root).unwrap().permissions().mode() & 0o777, 0o700);
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        }
        remove_file(&file).unwrap();
        assert!(!file.exists());
    }

    #[cfg(windows)]
    #[test]
    fn locked_destination_preserves_previous_commit_and_can_retry() {
        use std::os::windows::fs::OpenOptionsExt;
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("cookies.json");
        write_atomically(&file, b"committed").unwrap();
        let locked = std::fs::OpenOptions::new().read(true).share_mode(0).open(&file).unwrap();
        assert!(write_atomically(&file, b"not committed").is_err());
        drop(locked);
        assert_eq!(std::fs::read(&file).unwrap(), b"committed");
        write_atomically(&file, b"retry committed").unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"retry committed");
    }

    #[cfg(unix)]
    #[test]
    fn symlink_parent_cannot_replace_or_repermission_previous_commit() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let temporary = tempfile::tempdir().unwrap();
        let real = temporary.path().join("real");
        std::fs::create_dir(&real).unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();
        let file = real.join("cookies.json");
        write_atomically(&file, b"committed").unwrap();
        let alias = temporary.path().join("alias");
        symlink(&real, &alias).unwrap();
        assert!(write_atomically(&alias.join("cookies.json"), b"not committed").is_err());
        assert_eq!(std::fs::read(&file).unwrap(), b"committed");
        assert_eq!(std::fs::metadata(real).unwrap().permissions().mode() & 0o777, 0o755);
    }
}
