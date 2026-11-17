#[cfg(windows)]
mod windows {
    use anyhow::{Context, Result, bail, ensure};
    use serde_json::Value;
    use std::{
        io::Read,
        mem::size_of,
        path::PathBuf,
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };
    use windows_sys::Win32::{
        Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT},
        Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation},
        System::{
            Console::GetConsoleWindow,
            Threading::{OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION},
        },
        UI::{Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
    };

    #[derive(Clone)]
    pub struct Window {
        handle: usize,
        pid: u32,
        pub title: String,
    }

    impl Window {
        pub fn id(&self) -> String {
            format!("{:x}", self.handle)
        }
    }

    pub fn find(id: &str) -> Option<Window> {
        let foreground = unsafe { GetForegroundWindow() } as usize;
        let windows = list();
        if id.is_empty() {
            return windows
                .into_iter()
                .find(|window| window.handle == foreground);
        }
        windows.into_iter().find(|window| window.id() == id)
    }

    fn regular_process(pid: u32) -> bool {
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if process.is_null() {
            return false;
        }
        let mut token = std::ptr::null_mut();
        let opened = unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) };
        unsafe { CloseHandle(process) };
        if opened == 0 {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned = 0;
        let status = unsafe {
            GetTokenInformation(
                token,
                TokenElevation,
                std::ptr::from_mut(&mut elevation).cast(),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut returned,
            )
        };
        unsafe { CloseHandle(token) };
        status != 0 && elevation.TokenIsElevated == 0
    }

    unsafe extern "system" fn collect(hwnd: HWND, param: LPARAM) -> i32 {
        let entries = unsafe { &mut *(param as *mut Vec<Window>) };
        if entries.len() >= 80 {
            return 0;
        }
        if unsafe { IsWindowVisible(hwnd) } == 0 {
            return 1;
        }
        let mut name = [0u16; 256];
        let count = unsafe { GetWindowTextW(hwnd, name.as_mut_ptr(), name.len() as i32) };
        if count > 0 {
            let mut pid = 0;
            unsafe {
                GetWindowThreadProcessId(hwnd, &mut pid);
            }
            if pid != std::process::id()
                && hwnd != unsafe { GetConsoleWindow() }
                && regular_process(pid)
            {
                entries.push(Window {
                    handle: hwnd as usize,
                    pid,
                    title: String::from_utf16_lossy(&name[..count as usize]),
                });
            }
        }
        1
    }
    pub fn list() -> Vec<Window> {
        let mut windows = Vec::new();
        unsafe {
            EnumWindows(Some(collect), &mut windows as *mut Vec<Window> as LPARAM);
        }
        windows
    }

    pub struct Session {
        pub window: Window,
        observed: Option<(Instant, RECT, bool)>,
    }
    impl Session {
        pub fn new(window: Window) -> Self {
            Self {
                window,
                observed: None,
            }
        }
        fn hwnd(&self) -> Result<(HWND, RECT)> {
            let hwnd = self.window.handle as HWND;
            let mut current_pid = 0;
            let mut rect = RECT::default();
            unsafe {
                GetWindowThreadProcessId(hwnd, &mut current_pid);
            }
            ensure!(
                current_pid == self.window.pid
                    && regular_process(current_pid)
                    && unsafe { IsWindowVisible(hwnd) } != 0
                    && unsafe { IsIconic(hwnd) } == 0
                    && unsafe { GetWindowRect(hwnd, &mut rect) } != 0,
                "Selected window is no longer visible"
            );
            Ok((hwnd, rect))
        }
        pub fn observe(&mut self, screenshot: bool) -> Result<(String, Option<String>)> {
            let (hwnd, rect) = self.hwnd()?;
            let expression = if screenshot { CAPTURE } else { UIA };
            let script = format!(
                "$h=[IntPtr]::new([int64]{}); $ErrorActionPreference='Stop'; [Console]::OutputEncoding=[Text.Encoding]::UTF8; {expression}",
                self.window.handle
            );
            let executable = std::env::var_os("SystemRoot")
                .map(PathBuf::from)
                .context("Windows directory is unavailable")?
                .join("System32/WindowsPowerShell/v1.0/powershell.exe");
            use std::os::windows::process::CommandExt;
            let mut command = Command::new(executable);
            command
                .creation_flags(0x08000000)
                .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            let mut process = crate::native::tasks::ProcessTree::spawn(command)?;
            let output = process
                .child_mut()
                .stdout
                .take()
                .context("Missing observation output")?;
            let reader = thread::spawn(move || -> std::io::Result<Vec<u8>> {
                let mut bytes = Vec::new();
                output.take(512 * 1024 + 1).read_to_end(&mut bytes)?;
                Ok(bytes)
            });
            let deadline = Instant::now() + Duration::from_secs(10);
            let status = loop {
                if let Some(status) = process.child_mut().try_wait()? {
                    break status;
                }
                ensure!(Instant::now() < deadline, "Window observation timed out");
                thread::sleep(Duration::from_millis(35));
            };
            ensure!(status.success(), "Could not read the selected window");
            let bytes = reader
                .join()
                .map_err(|_| anyhow::anyhow!("Window observation stopped"))??;
            let text = String::from_utf8_lossy(&bytes);
            ensure!(
                text.len() <= 512 * 1024 && !text.trim().is_empty(),
                "Window observation is empty or too large"
            );
            let active = unsafe { GetForegroundWindow() } == hwnd;
            self.observed = Some((Instant::now(), rect, active));
            Ok(if screenshot {
                (
                    format!(
                        "Window: {}. Screenshot size: {}×{}.",
                        self.window.title,
                        rect.right - rect.left,
                        rect.bottom - rect.top
                    ),
                    Some(text.trim().to_owned()),
                )
            } else {
                (
                    format!(
                        "Window: {}\n{}",
                        self.window.title,
                        text.chars().take(12000).collect::<String>()
                    ),
                    None,
                )
            })
        }
        pub fn input(&mut self, args: &Value) -> Result<String> {
            let (hwnd, rect) = self.hwnd()?;
            let (time, old, previously_active) = self
                .observed
                .take()
                .context("Observe the window before requesting input")?;
            ensure!(
                time.elapsed() <= Duration::from_secs(90)
                    && [old.left, old.right, old.top, old.bottom]
                        == [rect.left, rect.right, rect.top, rect.bottom],
                "Window moved or the observation expired"
            );
            // The approval panel itself can take focus. After approval, restore
            // only the originally selected window and verify ownership before input.
            ensure!(
                previously_active
                    || unsafe { SetForegroundWindow(hwnd) } != 0
                    || unsafe { GetForegroundWindow() } == hwnd,
                "Activate the selected window to continue"
            );
            if unsafe { GetForegroundWindow() } != hwnd {
                unsafe {
                    SetForegroundWindow(hwnd);
                }
            }
            ensure!(
                unsafe { GetForegroundWindow() } == hwnd,
                "Selected window is no longer active"
            );
            let action = args["operation"].as_str().unwrap_or("");
            let events = match action {
                "click" | "scroll" => {
                    let x = args["x"].as_i64().context("Missing x coordinate")?;
                    let y = args["y"].as_i64().context("Missing y coordinate")?;
                    ensure!(
                        x >= 0
                            && y >= 0
                            && x < i64::from(rect.right - rect.left)
                            && y < i64::from(rect.bottom - rect.top),
                        "Point is outside the selected window"
                    );
                    let point = POINT {
                        x: rect.left + x as i32,
                        y: rect.top + y as i32,
                    };
                    ensure!(
                        unsafe { GetAncestor(WindowFromPoint(point), GA_ROOT) } == hwnd,
                        "Point is covered by another window"
                    );
                    ensure!(
                        unsafe { SetCursorPos(point.x, point.y) } != 0,
                        "Could not move the pointer"
                    );
                    if action == "click" {
                        vec![mouse(MOUSEEVENTF_LEFTDOWN, 0), mouse(MOUSEEVENTF_LEFTUP, 0)]
                    } else {
                        vec![mouse(
                            MOUSEEVENTF_WHEEL,
                            args["delta"].as_i64().unwrap_or(-120).clamp(-960, 960) as u32,
                        )]
                    }
                }
                "type" => {
                    let text = args["text"].as_str().context("Missing text")?;
                    ensure!(
                        !text.is_empty()
                            && text.chars().count() <= 500
                            && !text.chars().any(char::is_control),
                        "Enter up to 500 printable characters"
                    );
                    text.encode_utf16()
                        .flat_map(|code| {
                            [
                                keyboard(0, code, KEYEVENTF_UNICODE),
                                keyboard(0, code, KEYEVENTF_UNICODE | KEYEVENTF_KEYUP),
                            ]
                        })
                        .collect()
                }
                "key" => {
                    let spec = args["key"].as_str().context("Missing key")?;
                    let parts: Vec<_> = spec.split('+').collect();
                    ensure!(
                        !parts.is_empty() && parts.len() <= 3,
                        "Unsupported shortcut"
                    );
                    let mut keys = Vec::new();
                    for part in parts {
                        keys.push(match part {
                            "Ctrl" => VK_CONTROL,
                            "Shift" => VK_SHIFT,
                            "Enter" => VK_RETURN,
                            "Escape" => VK_ESCAPE,
                            "Tab" => VK_TAB,
                            "Up" => VK_UP,
                            "Down" => VK_DOWN,
                            "Left" => VK_LEFT,
                            "Right" => VK_RIGHT,
                            "Space" => VK_SPACE,
                            one if one.len() == 1 && one.as_bytes()[0].is_ascii_alphanumeric() => {
                                u16::from(one.as_bytes()[0].to_ascii_uppercase())
                            }
                            _ => bail!("Unsupported shortcut"),
                        });
                    }
                    keys.iter()
                        .map(|&key| keyboard(key, 0, 0))
                        .chain(
                            keys.iter()
                                .rev()
                                .map(|&key| keyboard(key, 0, KEYEVENTF_KEYUP)),
                        )
                        .collect()
                }
                _ => bail!("Unknown desktop input"),
            };
            ensure!(
                unsafe { GetForegroundWindow() } == hwnd,
                "Selected window is no longer active"
            );
            let sent = unsafe {
                SendInput(
                    events.len() as u32,
                    events.as_ptr(),
                    size_of::<INPUT>() as i32,
                )
            };
            ensure!(sent as usize == events.len(), "Windows rejected input");
            Ok("Input sent to the selected window".into())
        }
    }
    fn keyboard(vk: u16, scan: u16, flags: u32) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: vk,
                    wScan: scan,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }
    fn mouse(flags: u32, data: u32) -> INPUT {
        INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: data,
                    dwFlags: flags,
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }
    const UIA: &str = r#"Add-Type -AssemblyName UIAutomationClient,UIAutomationTypes;
$element=[Windows.Automation.AutomationElement]::FromHandle($h);
$count=0; $walker=[Windows.Automation.TreeWalker]::ControlViewWalker;
$queue=New-Object 'System.Collections.Generic.Queue[Windows.Automation.AutomationElement]';
$queue.Enqueue($element); while($queue.Count -gt 0 -and $count -lt 80) {
  $node=$queue.Dequeue(); $item=$node.Current;
  if (-not $item.IsPassword) {
    $bounds=$item.BoundingRectangle;
    "{0} | {1} | box={2},{3},{4},{5}" -f $item.LocalizedControlType,$item.Name,$bounds.X,$bounds.Y,$bounds.Width,$bounds.Height
  }
  $count++; $child=$walker.GetFirstChild($node);
  while($child -and $queue.Count -lt 120) { $queue.Enqueue($child); $child=$walker.GetNextSibling($child) }
}"#;
    const CAPTURE: &str = r#"Add-Type -AssemblyName System.Drawing;
Add-Type -TypeDefinition 'using System; using System.Runtime.InteropServices;
public static class Snap {
[StructLayout(LayoutKind.Sequential)] public struct R { public int Left, Top, Right, Bottom; }
[DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out R r);
[DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr dc, uint flags);
}';
$r=New-Object 'Snap+R'; if (-not [Snap]::GetWindowRect($h,[ref]$r)) { throw 'Window closed' };
$w=$r.Right-$r.Left; $height=$r.Bottom-$r.Top;
if ($w -lt 1 -or $height -lt 1 -or $w -gt 4000 -or $height -gt 2400) { throw 'Window too large' };
$bitmap=New-Object Drawing.Bitmap($w,$height); $gfx=[Drawing.Graphics]::FromImage($bitmap);
$dc=$gfx.GetHdc(); try { if (-not [Snap]::PrintWindow($h,$dc,2)) { throw 'Capture blocked' } } finally { $gfx.ReleaseHdc($dc); $gfx.Dispose() }
$stream=New-Object IO.MemoryStream; $bitmap.Save($stream,[Drawing.Imaging.ImageFormat]::Jpeg);
[Convert]::ToBase64String($stream.ToArray()); $stream.Dispose(); $bitmap.Dispose()"#;
    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        #[ignore = "requires an interactive Windows desktop"]
        fn observes_a_synthetic_window() -> Result<()> {
            let (send, receive) = std::sync::mpsc::channel();
            let fixture = thread::spawn(move || {
                let title: Vec<_> = "CarryCode window smoke test\0".encode_utf16().collect();
                let class: Vec<_> = "STATIC\0".encode_utf16().collect();
                let hwnd = unsafe {
                    CreateWindowExW(
                        0,
                        class.as_ptr(),
                        title.as_ptr(),
                        WS_OVERLAPPEDWINDOW,
                        80,
                        80,
                        400,
                        180,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                    )
                };
                if hwnd.is_null() {
                    let _ = send.send(0);
                    return;
                }
                unsafe {
                    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                }
                let _ = send.send(hwnd as usize);
                let mut message = MSG::default();
                while unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) } > 0 {
                    unsafe {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                    if unsafe { IsWindow(hwnd) } == 0 {
                        break;
                    }
                }
            });
            let handle = receive.recv_timeout(Duration::from_secs(2))?;
            ensure!(handle != 0, "Could not create the test window");
            let mut session = Session::new(Window {
                handle,
                pid: std::process::id(),
                title: "CarryCode window smoke test".into(),
            });
            let result = (|| -> Result<()> {
                let (text, _) = session.observe(false)?;
                assert!(text.contains("CarryCode window smoke test"));
                let (_, image) = session.observe(true)?;
                ensure!(
                    image.is_some_and(|image| image.starts_with("/9j/")),
                    "No captured JPEG"
                );
                Ok(())
            })();
            unsafe {
                PostMessageW(handle as HWND, WM_CLOSE, 0, 0);
            }
            fixture
                .join()
                .map_err(|_| anyhow::anyhow!("Test window stopped"))?;
            result
        }
    }
}
#[cfg(windows)]
pub use windows::*;
