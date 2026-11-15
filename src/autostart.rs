use crate::{bundle, platform};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    fs::{File, OpenOptions, TryLockError},
    path::{Path, PathBuf},
    process::{Child, Command},
    time::{Duration, Instant},
};

#[derive(Serialize, Deserialize, PartialEq, Eq)]
struct Registration {
    volume: String,
    relative: PathBuf,
    id: String,
}

type Volume = (String, PathBuf);
type VolumeScan = (Vec<Volume>, Vec<PathBuf>);

fn directory() -> Result<PathBuf> {
    Ok(platform::app_dir()?.join("autostart"))
}

pub fn enabled() -> Result<bool> {
    Ok(directory()?.join("registration.json").is_file() && os::installed()?)
}

fn try_lock(path: &Path) -> Result<Option<File>> {
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub fn run(action: &str, root: &Path) -> Result<()> {
    configure(action, root, false)
}

pub fn toggle(resolve_root: impl FnOnce() -> Result<PathBuf>) -> Result<()> {
    if enabled()? {
        configure("uninstall", Path::new(""), true)
    } else {
        configure("install", &resolve_root()?, true)
    }
}

fn configure(action: &str, root: &Path, from_menu: bool) -> Result<()> {
    let directory = directory()?;
    let registration = directory.join("registration.json");
    let helper = directory.join(if cfg!(windows) {
        "flashgpt-watch.exe"
    } else {
        "flashgpt-watch"
    });
    match action {
        "install" => {
            let root = root.canonicalize()?;
            let marker = bundle::read_marker(&root)?;
            let mut volumes: Vec<_> = os::volumes()?
                .0
                .into_iter()
                .filter_map(|(volume, path)| Some((volume, path.canonicalize().ok()?)))
                .filter(|(_, path)| root.starts_with(path))
                .collect();
            volumes.sort_by_key(|(_, path)| std::cmp::Reverse(path.components().count()));
            let (volume, mount) = volumes.first().context(tr!(
                "Комплект должен находиться на съёмном или смонтированном exFAT-томе",
                "The bundle must be on a removable drive or mounted exFAT volume"
            ))?;
            let relative = root.strip_prefix(mount)?.to_owned();
            ensure!(
                relative.as_os_str().is_empty() || bundle::valid(&relative),
                "{}",
                tr!("Недопустимый путь на носителе", "Invalid path on the drive")
            );
            fs::create_dir_all(&directory)?;
            platform::protect(&directory)?;
            let source = root.join(if cfg!(windows) {
                "flashGPT.exe"
            } else {
                "flashgpt"
            });
            if helper.exists() {
                ensure!(
                    fs::read(&helper)? == fs::read(&source)?,
                    "{}",
                    tr!(
                        "Сначала выполни autostart uninstall для предыдущей версии",
                        "Run autostart uninstall for the previous version first"
                    )
                );
            } else {
                let draft = tempfile::NamedTempFile::new_in(&directory)?;
                fs::copy(source, draft.path())?;
                draft.persist_noclobber(&helper)?;
            }
            let mut draft = tempfile::NamedTempFile::new_in(&directory)?;
            serde_json::to_writer_pretty(
                draft.as_file_mut(),
                &Registration {
                    volume: volume.clone(),
                    relative,
                    id: marker.id,
                },
            )?;
            draft.persist(&registration)?;
            os::install(&helper)?;
            os::start_watcher(&helper, from_menu)?;
            println!(
                "{}",
                tr!(
                    "Автооткрытие CarryCode включено для этой флешки.",
                    "CarryCode autostart is enabled for this USB drive."
                )
            );
        }
        "status" => {
            if enabled()? && helper.is_file() {
                println!(
                    "{}",
                    tr!(
                        "Автооткрытие CarryCode настроено.",
                        "CarryCode autostart is configured."
                    )
                );
            } else {
                println!(
                    "{}",
                    tr!(
                        "Автооткрытие CarryCode выключено.",
                        "CarryCode autostart is off."
                    )
                );
            }
        }
        "uninstall" => {
            os::uninstall()?;
            remove(&registration)?;
            if let Some(_watch) = wait_for_watcher(&directory.join("watch.lock"))? {
                // Windows keeps the helper executable mapped while its menu is open.
                let _ = remove(&helper);
            }
            println!(
                "{}",
                tr!(
                    "Автозапуск CarryCode выключен.",
                    "CarryCode autostart is off."
                )
            );
        }
        _ => anyhow::bail!(
            "{}",
            tr!(
                "Использование: flashgpt autostart install|status|uninstall",
                "Usage: flashgpt autostart install|status|uninstall"
            )
        ),
    }
    Ok(())
}

fn wait_for_watcher(path: &Path) -> Result<Option<File>> {
    if !path.exists() {
        return Ok(None);
    }
    for _ in 0..15 {
        if let Some(lock) = try_lock(path)? {
            return Ok(Some(lock));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(None)
}

fn remove(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

pub fn watch(skip_current: bool) -> Result<()> {
    let executable = env::current_exe()?;
    let directory = directory()?;
    let Some(_lock) = try_lock(&directory.join("watch.lock"))? else {
        return Ok(());
    };
    let registration = directory.join("registration.json");
    let mut previous_config = None;
    let mut last_mount: Option<PathBuf> = None;
    let mut attachment = Attachment::default();
    let mut skip_current = skip_current;
    let mut windows = Vec::<Child>::new();
    os::watch(|removed| {
        windows.retain_mut(|child| matches!(child.try_wait(), Ok(None)));
        let bytes = match fs::read(&registration) {
            Ok(bytes) => bytes,
            Err(error) => return error.kind() != std::io::ErrorKind::NotFound,
        };
        let Ok(config) = serde_json::from_slice::<Registration>(&bytes) else {
            return true;
        };
        let Ok((volumes, unreadable)) = os::volumes() else {
            return true;
        };
        if previous_config.as_ref() != Some(&config) {
            attachment = Attachment::default();
            last_mount = None;
        }
        if let Some(path) = &last_mount
            && os::removed_volume(removed, path)
        {
            attachment = Attachment::default();
            last_mount = None;
        }
        let mount = volumes
            .iter()
            .find(|(id, _)| id == &config.volume)
            .map(|(_, path)| path);
        if let Some(mount) = mount {
            last_mount = Some(mount.clone());
        }
        let root = locate(&volumes, &config);
        if std::mem::take(&mut skip_current) && root.is_some() {
            attachment.opened = true;
        }
        let present = mount.is_some()
            || last_mount
                .as_ref()
                .is_some_and(|path| unreadable.contains(path));
        if attachment.should_open(Instant::now(), present, root.as_deref()) {
            let result = (|| -> Result<()> {
                // The menu holds this lock for its lifetime. A reconnect can reuse it.
                let Some(lock) = try_lock(&directory.join("menu.lock"))? else {
                    return Ok(());
                };
                drop(lock);
                if let Some(child) = os::open_menu(&executable)? {
                    windows.push(child);
                }
                Ok(())
            })();
            match result {
                Ok(()) => attachment.opened = true,
                Err(error) => eprintln!("CarryCode: {error:#}"),
            }
        }
        previous_config = Some(config);
        true
    })
}

#[derive(Default)]
struct Attachment {
    ready: Option<(PathBuf, Instant)>,
    missing_since: Option<Instant>,
    opened: bool,
}

impl Attachment {
    fn should_open(&mut self, now: Instant, present: bool, root: Option<&Path>) -> bool {
        if present {
            self.missing_since = None;
        } else {
            if now.duration_since(*self.missing_since.get_or_insert(now)) >= Duration::from_secs(1)
            {
                self.opened = false;
            }
            self.ready = None;
            return false;
        }
        if self.opened {
            return false;
        }
        let Some(root) = root else {
            self.ready = None;
            return false;
        };
        if let Some((previous, since)) = &self.ready
            && previous == root
        {
            if now.duration_since(*since) < Duration::from_millis(500) {
                return false;
            }
            // Throttle retries when Windows cannot create a console.
            self.ready = Some((root.to_owned(), now));
            return true;
        }
        self.ready = Some((root.to_owned(), now));
        false
    }
}

pub fn open_attached() -> Result<()> {
    let Some(_lock) = try_lock(&directory()?.join("menu.lock"))? else {
        return Ok(());
    };
    os::attached_console();
    let path = directory()?.join("registration.json");
    let config: Registration = serde_json::from_slice(&fs::read(path).context(tr!(
        "Автооткрытие выключено. Запусти CarryCode с флешки.",
        "Autostart is off. Start CarryCode from the USB drive."
    ))?)?;
    let resolve = || {
        locate(&os::volumes()?.0, &config).context(tr!(
            "Флешка CarryCode недоступна. Подключи её и повтори действие.",
            "The CarryCode USB drive is unavailable. Reconnect it and try again."
        ))
    };
    crate::language::load(&resolve()?);
    crate::menu(true, resolve)
}

fn locate(volumes: &[Volume], config: &Registration) -> Option<PathBuf> {
    if !config.relative.as_os_str().is_empty() && !bundle::valid(&config.relative) {
        return None;
    }
    volumes
        .iter()
        .filter(|(id, _)| id == &config.volume)
        .find_map(|(_, mount)| {
            let real_mount = mount.canonicalize().ok()?;
            let root = mount.join(&config.relative).canonicalize().ok()?;
            let valid = root.starts_with(&real_mount)
                && bundle::read_marker(&root).is_ok_and(|marker| marker.id == config.id);
            valid.then_some(root)
        })
}

#[cfg(target_os = "linux")]
mod os {
    use super::*;
    use std::{
        ffi::OsString,
        os::unix::{ffi::OsStringExt, fs::PermissionsExt},
        process::Stdio,
        time::Duration,
    };
    use zbus::{
        blocking::{Connection, fdo::ObjectManagerProxy},
        zvariant::OwnedValue,
    };

    fn desktop_entry() -> PathBuf {
        let config = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute());
        config
            .unwrap_or_else(|| platform::home().join(".config"))
            .join("autostart/flashgpt.desktop")
    }

    pub fn volumes() -> Result<VolumeScan> {
        let bus = Connection::system()?;
        let manager = ObjectManagerProxy::builder(&bus)
            .destination("org.freedesktop.UDisks2")?
            .path("/org/freedesktop/UDisks2")?
            .build()?;
        let mut volumes = Vec::new();
        for interfaces in manager.get_managed_objects()?.into_values() {
            let Some(block) = interfaces.get("org.freedesktop.UDisks2.Block") else {
                continue;
            };
            let Some(filesystem) = interfaces.get("org.freedesktop.UDisks2.Filesystem") else {
                continue;
            };
            let format = block
                .get("IdType")
                .and_then(|value| <&str>::try_from(value).ok());
            if !matches!(format, Some("exfat" | "vfat")) {
                continue;
            }
            let Some(uuid) = block
                .get("IdUUID")
                .and_then(|value| <&str>::try_from(value).ok())
            else {
                continue;
            };
            if uuid.is_empty() {
                continue;
            }
            let Some(mounts) = filesystem
                .get("MountPoints")
                .and_then(|value: &OwnedValue| value.try_clone().ok())
            else {
                continue;
            };
            for mut mount in Vec::<Vec<u8>>::try_from(mounts)? {
                if mount.last() == Some(&0) {
                    mount.pop();
                }
                let mount = PathBuf::from(OsString::from_vec(mount));
                if mount.is_absolute() {
                    volumes.push((uuid.to_owned(), mount));
                }
            }
        }
        Ok((volumes, Vec::new()))
    }

    fn terminal() -> Result<&'static str> {
        for program in ["/usr/bin/gnome-terminal", "/usr/bin/kgx", "/usr/bin/xterm"] {
            if Path::new(program).is_file() {
                return Ok(program);
            }
        }
        anyhow::bail!(
            "{}",
            tr!(
                "Установи поддерживаемый терминал: GNOME Terminal, Console или xterm",
                "Install a supported terminal: GNOME Terminal, Console, or xterm"
            )
        )
    }

    pub fn install(helper: &Path) -> Result<()> {
        terminal()?;
        ensure!(
            env::var_os("DISPLAY").is_some() || env::var_os("WAYLAND_DISPLAY").is_some(),
            "{}",
            tr!(
                "Включи автооткрытие в графической сессии",
                "Enable autostart from a graphical session"
            )
        );
        fs::set_permissions(helper, fs::Permissions::from_mode(0o700))?;
        let path = helper.to_str().context(tr!(
            "XDG Autostart требует путь UTF-8",
            "XDG Autostart requires a UTF-8 path"
        ))?;
        ensure!(
            !path.contains(['\n', '\r']),
            "{}",
            tr!(
                "Путь помощника содержит перевод строки",
                "The helper path contains a newline"
            )
        );
        let escaped = path
            .replace('\\', "\\\\\\\\")
            .replace('"', "\\\\\"")
            .replace('`', "\\\\`")
            .replace('$', "\\\\$")
            .replace('%', "%%");
        let entry = desktop_entry();
        fs::create_dir_all(entry.parent().unwrap())?;
        fs::write(
            entry,
            format!(
                "[Desktop Entry]\nType=Application\nName=CarryCode\nExec=\"{escaped}\" watch\nTerminal=false\n"
            ),
        )?;
        Ok(())
    }

    pub fn installed() -> Result<bool> {
        Ok(desktop_entry().is_file())
    }
    pub fn uninstall() -> Result<()> {
        remove(&desktop_entry())
    }

    pub fn start_watcher(helper: &Path, skip_current: bool) -> Result<()> {
        let mut command = Command::new(helper);
        command.arg("watch");
        if skip_current {
            command.arg("--skip-current");
        }
        command
            .current_dir(helper.parent().context(tr!(
                "Нет каталога помощника",
                "The helper folder is missing"
            ))?)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(())
    }

    pub fn open_menu(executable: &Path) -> Result<Option<Child>> {
        let terminal = terminal()?;
        let mut command = Command::new(terminal);
        command.arg(match terminal {
            "/usr/bin/xterm" => "-e",
            _ => "--",
        });
        Ok(Some(
            command
                .arg(executable)
                .arg("--attached")
                .current_dir(executable.parent().context(tr!(
                    "Нет каталога помощника",
                    "The helper folder is missing"
                ))?)
                .spawn()?,
        ))
    }

    pub fn attached_console() {}

    pub fn removed_volume(_: u32, _: &Path) -> bool {
        false
    }

    pub fn watch(mut update: impl FnMut(u32) -> bool) -> Result<()> {
        while update(0) {
            std::thread::sleep(Duration::from_millis(1250));
        }
        Ok(())
    }
}

#[cfg(windows)]
mod os {
    use super::*;
    use std::{
        ffi::{OsStr, OsString},
        os::windows::{ffi::OsStringExt, process::CommandExt},
        ptr,
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, ERROR_FILE_NOT_FOUND, HWND, LPARAM, LRESULT, WPARAM},
        Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW},
        System::{
            Console::{FreeConsole, SetConsoleTitleW},
            Diagnostics::Debug::{SEM_FAILCRITICALERRORS, SetThreadErrorMode},
            Registry::{
                HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW,
                RegSetKeyValueW,
            },
            Threading::{
                CREATE_NEW_CONSOLE, CREATE_NO_WINDOW, CreateProcessW, PROCESS_INFORMATION,
                STARTUPINFOW,
            },
        },
        UI::WindowsAndMessaging::*,
    };
    const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
    const APP: &str = "flashGPT";

    pub fn volumes() -> Result<VolumeScan> {
        // Treat transient media errors as scan results, without the system dialog.
        let mut old_mode = 0;
        let changed = unsafe { SetThreadErrorMode(SEM_FAILCRITICALERRORS, &mut old_mode) } != 0;
        let result = scan_volumes();
        if changed {
            unsafe { SetThreadErrorMode(old_mode, ptr::null_mut()) };
        }
        result
    }

    fn scan_volumes() -> Result<VolumeScan> {
        let mut volumes = Vec::new();
        let mut unreadable = Vec::new();
        let drives = unsafe { GetLogicalDrives() };
        ensure!(
            drives != 0,
            "{}",
            tr!("Не удалось перечислить диски", "Could not list drives")
        );
        for bit in 0..26 {
            if drives & (1 << bit) == 0 {
                continue;
            }
            let root = PathBuf::from(format!("{}:\\", char::from(b'A' + bit)));
            let wide = platform::wide(root.as_os_str());
            let drive_type = unsafe { GetDriveTypeW(wide.as_ptr()) };
            if !matches!(drive_type, 2 | 3) {
                continue;
            }
            let mut serial = 0;
            let mut format = [0u16; 32];
            let result = unsafe {
                GetVolumeInformationW(
                    wide.as_ptr(),
                    ptr::null_mut(),
                    0,
                    &mut serial,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    format.as_mut_ptr(),
                    format.len() as u32,
                )
            };
            if result == 0 {
                unreadable.push(root);
                continue;
            }
            let format = OsString::from_wide(
                &format[..format
                    .iter()
                    .position(|value| *value == 0)
                    .unwrap_or(format.len())],
            );
            if drive_type == 2 || format == "exFAT" {
                volumes.push((format!("{serial:08x}"), root));
            }
        }
        Ok((volumes, unreadable))
    }

    pub fn install(helper: &Path) -> Result<()> {
        let value = platform::wide(OsStr::new(&format!("\"{}\" watch", helper.display())));
        let key = platform::wide(OsStr::new(RUN_KEY));
        let name = platform::wide(OsStr::new(APP));
        let result = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            )
        };
        ensure!(
            result == 0,
            "{}",
            trf!(
                "Не удалось настроить автооткрытие: код {result}",
                "Could not configure autostart: code {result}"
            )
        );
        Ok(())
    }

    pub fn installed() -> Result<bool> {
        let key = platform::wide(OsStr::new(RUN_KEY));
        let name = platform::wide(OsStr::new(APP));
        let mut size = 0;
        let result = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut size,
            )
        };
        Ok(result == 0)
    }

    pub fn uninstall() -> Result<()> {
        let key = platform::wide(OsStr::new(RUN_KEY));
        let name = platform::wide(OsStr::new(APP));
        let result = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) };
        ensure!(
            result == 0 || result == ERROR_FILE_NOT_FOUND,
            "{}",
            trf!(
                "Не удалось убрать автооткрытие: код {result}",
                "Could not remove autostart: code {result}"
            )
        );
        Ok(())
    }

    pub fn start_watcher(helper: &Path, skip_current: bool) -> Result<()> {
        let mut command = Command::new(helper);
        command.arg("watch");
        if skip_current {
            command.arg("--skip-current");
        }
        command
            .current_dir(helper.parent().context(tr!(
                "Нет каталога помощника",
                "The helper folder is missing"
            ))?)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()?;
        Ok(())
    }

    pub fn open_menu(executable: &Path) -> Result<Option<Child>> {
        let executable_name = platform::wide(executable.as_os_str());
        let working_directory = platform::wide(
            executable
                .parent()
                .context(tr!(
                    "Нет каталога помощника",
                    "The helper folder is missing"
                ))?
                .as_os_str(),
        );
        let mut arguments = OsString::from("\"");
        arguments.push(executable);
        arguments.push("\" --attached");
        let mut arguments = platform::wide(&arguments);
        let startup = STARTUPINFOW {
            cb: std::mem::size_of::<STARTUPINFOW>() as u32,
            ..Default::default()
        };
        let mut child = PROCESS_INFORMATION::default();
        // A new console receives its own standard handles instead of the watcher's streams.
        let result = unsafe {
            CreateProcessW(
                executable_name.as_ptr(),
                arguments.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                0,
                CREATE_NEW_CONSOLE,
                ptr::null(),
                working_directory.as_ptr(),
                &startup,
                &mut child,
            )
        };
        ensure!(
            result != 0,
            "{}",
            trf!(
                "Не удалось открыть меню: {}",
                "Could not open the menu: {}",
                std::io::Error::last_os_error()
            )
        );
        unsafe {
            CloseHandle(child.hThread);
            CloseHandle(child.hProcess);
        }
        Ok(None)
    }

    unsafe extern "system" fn proc(window: HWND, message: u32, w: WPARAM, l: LPARAM) -> LRESULT {
        unsafe {
            if message == WM_DEVICECHANGE {
                let mut removed = 0;
                if w == DBT_DEVICEREMOVECOMPLETE as usize && l != 0 {
                    let header = &*(l as *const DEV_BROADCAST_HDR);
                    if header.dbch_devicetype == DBT_DEVTYP_VOLUME
                        && header.dbch_size as usize >= std::mem::size_of::<DEV_BROADCAST_VOLUME>()
                    {
                        removed = (*(l as *const DEV_BROADCAST_VOLUME)).dbcv_unitmask;
                    }
                }
                PostMessageW(window, WM_APP, removed as usize, 0);
                DefWindowProcW(window, message, w, l)
            } else {
                DefWindowProcW(window, message, w, l)
            }
        }
    }

    pub fn removed_volume(mask: u32, path: &Path) -> bool {
        use std::path::{Component, Prefix};
        match path.components().next() {
            Some(Component::Prefix(prefix)) => match prefix.kind() {
                Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => {
                    mask & (1 << (letter.to_ascii_uppercase() - b'A')) != 0
                }
                _ => false,
            },
            _ => false,
        }
    }

    pub fn attached_console() {
        unsafe {
            SetThreadErrorMode(SEM_FAILCRITICALERRORS, ptr::null_mut());
            SetConsoleTitleW(platform::wide(OsStr::new(APP)).as_ptr());
        }
    }

    pub fn watch(mut update: impl FnMut(u32) -> bool) -> Result<()> {
        unsafe {
            FreeConsole();
            SetThreadErrorMode(SEM_FAILCRITICALERRORS, ptr::null_mut());
            let name = platform::wide(OsStr::new("flashGPT.DeviceWatcher"));
            let instance = windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(ptr::null());
            let class = WNDCLASSW {
                lpfnWndProc: Some(proc),
                lpszClassName: name.as_ptr(),
                hInstance: instance,
                ..Default::default()
            };
            ensure!(
                RegisterClassW(&class) != 0,
                "{}",
                tr!(
                    "Не удалось зарегистрировать окно помощника",
                    "Could not register the helper window"
                )
            );
            let window = CreateWindowExW(
                0,
                name.as_ptr(),
                name.as_ptr(),
                0,
                0,
                0,
                0,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
                instance,
                ptr::null(),
            );
            ensure!(
                !window.is_null(),
                "{}",
                tr!(
                    "Не удалось создать окно помощника",
                    "Could not create the helper window"
                )
            );
            ensure!(
                SetTimer(window, 1, 500, None) != 0,
                "{}",
                tr!(
                    "Не удалось запустить таймер помощника",
                    "Could not start the helper timer"
                )
            );
            let mut message = MSG::default();
            let mut running = update(0);
            while running && GetMessageW(&mut message, ptr::null_mut(), 0, 0) > 0 {
                if matches!(message.message, WM_APP | WM_TIMER) {
                    running = update(if message.message == WM_APP {
                        message.wParam as u32
                    } else {
                        0
                    });
                }
                DispatchMessageW(&message);
            }
            DestroyWindow(window);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retries_until_mount_is_ready_and_does_not_reopen_on_transient_errors() {
        let mut attach = Attachment::default();
        let start = Instant::now();
        let moment = |ms| start + Duration::from_millis(ms);
        let path = Path::new("usb");

        assert!(!attach.should_open(moment(0), true, None));
        assert!(!attach.should_open(moment(500), true, Some(path)));
        assert!(attach.should_open(moment(1000), true, Some(path)));
        assert!(!attach.should_open(moment(1100), true, Some(path)));
        assert!(attach.should_open(moment(1500), true, Some(path)));
        attach.opened = true;
        assert!(!attach.should_open(moment(2000), true, None));
        assert!(!attach.should_open(moment(2500), false, None));
        assert!(!attach.should_open(moment(3000), true, Some(path)));
        assert!(!attach.should_open(moment(3500), true, Some(path)));
    }

    #[test]
    fn sustained_disconnect_rearms_for_the_same_mount() {
        let mut attach = Attachment {
            opened: true,
            ..Attachment::default()
        };
        let start = Instant::now();
        let moment = |ms| start + Duration::from_millis(ms);
        let path = Path::new("usb");
        assert!(!attach.should_open(moment(0), false, None));
        assert!(!attach.should_open(moment(1000), false, None));
        assert!(!attach.should_open(moment(1500), true, Some(path)));
        assert!(attach.should_open(moment(2000), true, Some(path)));
    }
}
