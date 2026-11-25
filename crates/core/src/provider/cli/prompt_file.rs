use std::io::{self, Write};
use std::path::Path;
use tempfile::{Builder, NamedTempFile};

pub(super) fn write(prompt: &str, dir: &Path) -> io::Result<NamedTempFile> {
    let mut builder = Builder::new();
    builder.prefix("supereditr-claude-prompt-");
    let mut file = create(&builder, dir)?;
    file.write_all(prompt.as_bytes())?;
    Ok(file)
}

#[cfg(unix)]
fn create(builder: &Builder<'_, '_>, dir: &Path) -> io::Result<NamedTempFile> {
    use std::os::unix::fs::PermissionsExt;

    // tempfile creates the file exclusively with mode 0600. Check that the volume honored it
    // before writing. If killed before cleanup, the app can leave this owner-only file in temp.
    let file = builder.tempfile_in(dir)?;
    if file.as_file().metadata()?.permissions().mode() & 0o077 != 0 {
        return Err(io::Error::new(io::ErrorKind::PermissionDenied, "the temporary file is accessible to other users"));
    }
    #[cfg(target_os = "macos")]
    check_macos_acl(file.as_file())?;
    Ok(file)
}

#[cfg(target_os = "macos")]
fn check_macos_acl(file: &std::fs::File) -> io::Result<()> {
    use std::os::fd::AsRawFd;

    unsafe extern "C" {
        fn acl_get_fd(fd: libc::c_int) -> *mut libc::c_void;
        fn acl_free(acl: *mut libc::c_void) -> libc::c_int;
    }

    // A macOS file can inherit an ACL that grants access beyond its mode bits. Reject it before
    // writing, since another user could already have opened the file with those permissions.
    // SAFETY: The file keeps its descriptor valid, and a non-null ACL belongs to this call.
    let acl = unsafe { acl_get_fd(file.as_raw_fd()) };
    if !acl.is_null() {
        unsafe { acl_free(acl) };
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the temporary file inherited an access control list",
        ));
    }
    let error = io::Error::last_os_error();
    if matches!(error.raw_os_error(), Some(libc::ENOENT | libc::ENOTSUP)) { Ok(()) } else { Err(error) }
}

#[cfg(windows)]
fn create(builder: &Builder<'_, '_>, dir: &Path) -> io::Result<NamedTempFile> {
    // The handle removes the name when it closes, including after a process crash.
    builder.make_in(dir, create_windows_file)
}

#[cfg(windows)]
fn create_windows_file(path: &Path) -> io::Result<std::fs::File> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::Storage::FileSystem::{
        CREATE_NEW, CreateFileW, DELETE, FILE_ATTRIBUTE_TEMPORARY, FILE_FLAG_DELETE_ON_CLOSE, FILE_SHARE_DELETE,
        FILE_SHARE_READ, GetVolumeInformationByHandleW,
    };
    use windows_sys::Win32::System::SystemServices::FILE_PERSISTENT_ACLS;

    let name: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // Protected DACL (no inherited access), granting full access to the file's owner.
    let sddl: Vec<u16> = "D:P(A;;FA;;;OW)\0".encode_utf16().collect();
    let mut descriptor = null_mut();
    // SAFETY: Both strings are nul-terminated. The descriptor lives through CreateFileW and must
    // be freed with LocalFree. A successful CreateFileW returns a new owned, non-inherited handle.
    unsafe {
        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
            sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            null_mut(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let security = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let handle = CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE | DELETE,
            FILE_SHARE_READ | FILE_SHARE_DELETE,
            &security,
            CREATE_NEW,
            FILE_ATTRIBUTE_TEMPORARY | FILE_FLAG_DELETE_ON_CLOSE,
            null_mut(),
        );
        let file = if handle == INVALID_HANDLE_VALUE {
            Err(io::Error::last_os_error())
        } else {
            Ok(std::fs::File::from_raw_handle(handle))
        };
        LocalFree(descriptor);
        let file = file?;
        let mut flags = 0;
        if GetVolumeInformationByHandleW(handle, null_mut(), 0, null_mut(), null_mut(), &mut flags, null_mut(), 0) == 0
        {
            return Err(io::Error::last_os_error());
        }
        if flags & FILE_PERSISTENT_ACLS == 0 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "the temporary folder does not support access controls",
            ));
        }
        Ok(file)
    }
}

#[cfg(not(any(unix, windows)))]
fn create(_: &Builder<'_, '_>, _: &Path) -> io::Result<NamedTempFile> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "private prompt files are unavailable on this platform"))
}

#[cfg(all(test, any(unix, windows)))]
#[test]
fn prompt_is_readable_by_path_until_the_guard_drops() {
    let dir = tempfile::tempdir().unwrap();
    let prompt = "  Preserve QuillPoint — Brynlee’s dictionary entry.\r\n\n";
    let file = write(prompt, dir.path()).unwrap();
    let path = file.path().to_owned();
    assert_eq!(std::fs::read(&path).unwrap(), prompt.as_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
    drop(file);
    assert!(!path.exists());
}

#[cfg(all(test, target_os = "macos"))]
#[test]
fn rejects_inherited_macos_acl_before_writing() {
    let dir = tempfile::tempdir().unwrap();
    let status = std::process::Command::new("/bin/chmod")
        .args(["+a", "everyone allow read,file_inherit"])
        .arg(dir.path())
        .status()
        .unwrap();
    assert!(status.success());
    let error = write("QuillPoint", dir.path()).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
    assert_eq!(dir.path().read_dir().unwrap().count(), 0);
}
