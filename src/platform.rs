use anyhow::Result;
use std::{
    env,
    path::{Path, PathBuf},
};

#[cfg(target_os = "linux")]
pub use linux::*;
#[cfg(windows)]
pub use windows::*;

pub fn require_standard_user() -> Result<()> {
    anyhow::ensure!(
        !is_elevated()?,
        "{}",
        tr!(
            "Открой CarryCode из обычного терминала без прав администратора или root.",
            "Open CarryCode in a regular terminal without administrator or root privileges."
        )
    );
    Ok(())
}

pub fn private_dir() -> Result<tempfile::TempDir> {
    let base = runtime_dir().unwrap_or(app_dir()?.join("sessions"));
    std::fs::create_dir_all(&base)?;
    let dir = tempfile::Builder::new()
        .prefix("flashgpt-")
        .tempdir_in(base)?;
    protect(dir.path())?;
    Ok(dir)
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use anyhow::ensure;
    use std::fs;
    use std::{
        ffi::CString,
        os::unix::{
            ffi::OsStrExt,
            fs::{MetadataExt, PermissionsExt},
        },
    };

    pub fn is_elevated() -> Result<bool> {
        Ok(unsafe { libc::getuid() == 0 || libc::geteuid() == 0 })
    }

    pub fn app_dir() -> Result<PathBuf> {
        let base = env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute());
        Ok(base
            .unwrap_or_else(|| home().join(".local/share"))
            .join("flashgpt"))
    }

    pub fn home() -> PathBuf {
        env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir)
    }

    pub fn runtime_dir() -> Option<PathBuf> {
        env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|path| {
                let Ok(metadata) = fs::symlink_metadata(path) else {
                    return false;
                };
                path.is_absolute()
                    && metadata.is_dir()
                    && metadata.uid() == unsafe { libc::geteuid() }
                    && metadata.mode() & 0o077 == 0
            })
    }

    pub fn protect(path: &Path) -> Result<()> {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        Ok(())
    }

    pub fn sync_directory(path: &Path) -> Result<()> {
        fs::File::open(path)?.sync_all()?;
        Ok(())
    }

    pub fn available(path: &Path) -> Result<u64> {
        let path = CString::new(path.as_os_str().as_bytes())?;
        let mut stat = std::mem::MaybeUninit::<libc::statvfs>::uninit();
        ensure!(
            unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) } == 0,
            "{}",
            trf!(
                "Не удалось прочитать свойства тома: {}",
                "Could not read volume information: {}",
                std::io::Error::last_os_error()
            )
        );
        let stat = unsafe { stat.assume_init() };
        Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
    }

    pub fn check_execution(path: &Path) -> Result<()> {
        let path = CString::new(path.as_os_str().as_bytes())?;
        ensure!(
            unsafe { libc::access(path.as_ptr(), libc::X_OK) } == 0,
            "{}",
            tr!(
                "Linux запрещает запуск файла. Проверь права выполнения и параметры монтирования.",
                "Linux denied execution. Check executable permissions and mount options."
            )
        );
        Ok(())
    }

    pub fn run_child(mut command: std::process::Command) -> Result<std::process::ExitStatus> {
        use std::os::unix::process::CommandExt;
        // A foreground child handles Ctrl-C itself. The launcher stays alive to save.
        let mut ignore = unsafe { std::mem::zeroed::<libc::sigaction>() };
        let mut previous = unsafe { std::mem::zeroed::<libc::sigaction>() };
        ignore.sa_sigaction = libc::SIG_IGN;
        unsafe {
            libc::sigemptyset(&mut ignore.sa_mask);
            if libc::sigaction(libc::SIGINT, &ignore, &mut previous) != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            command.pre_exec(|| {
                if libc::signal(libc::SIGINT, libc::SIG_DFL) == libc::SIG_ERR {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let result = command.status();
        unsafe {
            libc::sigaction(libc::SIGINT, &previous, std::ptr::null_mut());
        }
        Ok(result?)
    }
}

#[cfg(windows)]
mod windows {
    use super::*;
    use anyhow::{Context, ensure};
    use std::{ffi::OsStr, io::IsTerminal, os::windows::ffi::OsStrExt, process::Command, ptr};
    use windows_sys::Win32::{
        Foundation::{CloseHandle, LocalFree},
        Security::{
            Authorization::{
                ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
                SDDL_REVISION_1,
            },
            DACL_SECURITY_INFORMATION, GetTokenInformation, PROTECTED_DACL_SECURITY_INFORMATION,
            SetFileSecurityW, TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER, TokenElevation, TokenUser,
        },
        Storage::FileSystem::{GetDiskFreeSpaceExW, GetLogicalDrives},
        System::{
            Console::{
                CTRL_C_EVENT, ENABLE_VIRTUAL_TERMINAL_PROCESSING, GetConsoleMode, GetStdHandle,
                STD_OUTPUT_HANDLE, SetConsoleCtrlHandler, SetConsoleMode,
            },
            Threading::{GetCurrentProcess, OpenProcessToken},
        },
    };

    pub fn is_elevated() -> Result<bool> {
        let mut token = ptr::null_mut();
        let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
        let mut written = 0;
        unsafe {
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return Err(std::io::Error::last_os_error().into());
            }
            let result = GetTokenInformation(
                token,
                TokenElevation,
                (&mut elevation as *mut TOKEN_ELEVATION).cast(),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut written,
            );
            let error = std::io::Error::last_os_error();
            CloseHandle(token);
            if result == 0 {
                return Err(error.into());
            }
        }
        Ok(elevation.TokenIsElevated != 0)
    }

    pub fn wide(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(Some(0)).collect()
    }

    pub fn app_dir() -> Result<PathBuf> {
        Ok(PathBuf::from(
            env::var_os("LOCALAPPDATA")
                .context(tr!("Нет LOCALAPPDATA", "LOCALAPPDATA is missing"))?,
        )
        .join("flashGPT"))
    }

    pub fn home() -> PathBuf {
        env::var_os("USERPROFILE")
            .map(PathBuf::from)
            .unwrap_or_else(env::temp_dir)
    }

    pub fn runtime_dir() -> Option<PathBuf> {
        None
    }

    fn current_user_sid() -> Result<String> {
        let mut token = ptr::null_mut();
        unsafe {
            ensure!(
                OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) != 0,
                "{}",
                std::io::Error::last_os_error()
            );
        }
        let result = (|| -> Result<String> {
            let mut needed = 0u32;
            unsafe {
                GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut needed);
            }
            ensure!(needed > 0, "{}", std::io::Error::last_os_error());
            let mut buffer = vec![0usize; (needed as usize).div_ceil(std::mem::size_of::<usize>())];
            let mut written = 0u32;
            unsafe {
                ensure!(
                    GetTokenInformation(
                        token,
                        TokenUser,
                        buffer.as_mut_ptr().cast(),
                        needed,
                        &mut written,
                    ) != 0,
                    "{}",
                    std::io::Error::last_os_error()
                );
                let user = &*(buffer.as_ptr().cast::<TOKEN_USER>());
                let mut raw: *mut u16 = ptr::null_mut();
                ensure!(
                    ConvertSidToStringSidW(user.User.Sid, &mut raw) != 0,
                    "{}",
                    std::io::Error::last_os_error()
                );
                let length = (0..).take_while(|index| *raw.add(*index) != 0).count();
                let sid = String::from_utf16_lossy(std::slice::from_raw_parts(raw, length));
                LocalFree(raw.cast());
                Ok(sid)
            }
        })();
        unsafe {
            CloseHandle(token);
        }
        result
    }

    pub fn protect(path: &Path) -> Result<()> {
        // A filtered or scheduled-task token can have an owner SID other than
        // the current user. Keep the explicit user ACE as well as OWNER RIGHTS.
        let sid = current_user_sid()?;
        let sddl = wide(OsStr::new(&format!(
            "D:P(A;OICI;FA;;;OW)(A;OICI;FA;;;SY)(A;OICI;FA;;;{sid})"
        )));
        let path = wide(path.as_os_str());
        let mut descriptor = ptr::null_mut();
        // SAFETY: both strings are NUL terminated. The descriptor is released with LocalFree.
        unsafe {
            ensure!(
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    sddl.as_ptr(),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    ptr::null_mut()
                ) != 0,
                "{}",
                std::io::Error::last_os_error()
            );
            let result = SetFileSecurityW(
                path.as_ptr(),
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                descriptor,
            );
            let error = std::io::Error::last_os_error();
            LocalFree(descriptor);
            ensure!(
                result != 0,
                "{}",
                trf!(
                    "Не удалось ограничить доступ к временному каталогу: {error}",
                    "Could not restrict access to the temporary folder: {error}"
                )
            );
        }
        Ok(())
    }

    pub fn sync_directory(_: &Path) -> Result<()> {
        // The new snapshot file is flushed after the rename.
        Ok(())
    }

    pub fn available(path: &Path) -> Result<u64> {
        let path = wide(path.as_os_str());
        let mut free = 0;
        let result = unsafe {
            GetDiskFreeSpaceExW(path.as_ptr(), &mut free, ptr::null_mut(), ptr::null_mut())
        };
        ensure!(
            result != 0,
            "{}",
            trf!(
                "Не удалось прочитать свойства тома: {}",
                "Could not read volume information: {}",
                std::io::Error::last_os_error()
            )
        );
        Ok(free)
    }

    pub fn check_execution(_: &Path) -> Result<()> {
        Ok(())
    }

    pub fn folder_roots() -> Vec<PathBuf> {
        let drives = unsafe { GetLogicalDrives() };
        (0..26)
            .filter(|bit| drives & (1 << bit) != 0)
            .map(|bit| PathBuf::from(format!("{}:\\", char::from(b'A' + bit))))
            .collect()
    }

    pub fn configure_terminal(command: &mut Command) {
        if !env::var("TERM").is_ok_and(|value| value.eq_ignore_ascii_case("dumb"))
            || !std::io::stdout().is_terminal()
        {
            return;
        }
        let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
        let mut mode = 0;
        // Probe the actual Windows console before overriding an inherited TERM value.
        let vt_works = unsafe {
            GetConsoleMode(output, &mut mode) != 0
                && SetConsoleMode(output, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
        };
        if vt_works {
            command.env("TERM", "xterm-256color");
        }
    }

    unsafe extern "system" fn menu_interrupt(event: u32) -> i32 {
        i32::from(event == CTRL_C_EVENT)
    }

    pub fn run_child(mut command: std::process::Command) -> Result<std::process::ExitStatus> {
        // A registered handler applies only to this process. The child retains Ctrl-C.
        ensure!(
            unsafe { SetConsoleCtrlHandler(Some(menu_interrupt), 1) } != 0,
            "{}",
            std::io::Error::last_os_error()
        );
        let result = command.status();
        unsafe {
            SetConsoleCtrlHandler(Some(menu_interrupt), 0);
        }
        Ok(result?)
    }
}
