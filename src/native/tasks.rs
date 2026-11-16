use anyhow::{Result, bail};
use std::{
    io::Read,
    path::Path,
    process::{Command, Stdio},
};
fn command(task: &str) -> Result<(&'static str, Vec<&'static str>)> {
    Ok(match task {
        "cargo-test" => ("cargo", vec!["test", "--locked"]),
        "cargo-check" => ("cargo", vec!["check", "--locked"]),
        "npm-test" => ("npm", vec!["test"]),
        "npm-build" => ("npm", vec!["run", "build"]),
        "python-test" => ("python", vec!["-m", "unittest", "discover"]),
        _ => bail!("Unknown task"),
    })
}
pub(super) fn run(directory: &Path, task: &str) -> Result<String> {
    crate::platform::require_standard_user()?;
    let (program, args) = command(task)?;
    println!(
        "{}",
        trf!(
            "Запустить {} {} в {}?\nСкрипты проекта выполняются с твоими правами и могут менять файлы. Esc прерывает команду.",
            "Run {} {} in {}?\nProject scripts run with your permissions and can modify files. Esc interrupts the command.",
            program,
            args.join(" "),
            directory.display()
        )
    );
    let mut nonce = [0; 3];
    getrandom::fill(&mut nonce).map_err(|error| anyhow::anyhow!("{error}"))?;
    let nonce = format!("{:02X}{:02X}{:02X}", nonce[0], nonce[1], nonce[2]);
    if crate::ask(&trf!(
        "Введи {nonce} для запуска, Enter отмена: ",
        "Type {nonce} to run, Enter cancels: "
    ))? != nonce
    {
        return Ok("User canceled the command".into());
    }
    let output = tempfile::tempfile()?;
    let mut child = Command::new(program);
    #[cfg(windows)]
    if program == "npm" {
        child = Command::new("cmd.exe");
        child.args(["/d", "/c", "npm.cmd"]);
    }
    child
        .args(&args)
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(output.try_clone()?)
        .stderr(output.try_clone()?);
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy().to_ascii_uppercase();
        if name.contains("KEY")
            || name.contains("TOKEN")
            || name.contains("SECRET")
            || name.contains("PASSWORD")
        {
            child.env_remove(key);
        }
    }
    let status = run_process(child, &output)?;
    use std::io::{Seek, SeekFrom};
    let mut output = output;
    output.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    output.take(64 * 1024).read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let clean: String = text
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .collect();
    Ok(format!(
        "Command {program} {} exited with {status}\n{clean}",
        args.join(" ")
    ))
}
fn run_process(command: Command, output: &std::fs::File) -> Result<std::process::ExitStatus> {
    let mut process = ProcessTree::spawn(command)?;
    let child = &mut process.0;
    let _terminal = RawTerminal::open()?;
    let started = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        let stopped = if crossterm::event::poll(std::time::Duration::from_millis(100))? {
            matches!(crossterm::event::read()?, crossterm::event::Event::Key(key) if key.code == crossterm::event::KeyCode::Esc || (key.code == crossterm::event::KeyCode::Char('c') && key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)))
        } else {
            false
        };
        if stopped
            || started.elapsed().as_secs() >= 300
            || output.metadata()?.len() > 16 * 1024 * 1024
        {
            bail!("Command stopped by user, five-minute timeout, or 16 MiB output limit");
        }
    }
}
struct ProcessTree(
    std::process::Child,
    #[cfg(windows)] windows_sys::Win32::Foundation::HANDLE,
);
impl ProcessTree {
    fn spawn(mut command: Command) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let child = command.spawn()?;
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::{Foundation::CloseHandle, System::JobObjects::*};
            let mut child = child;
            let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if job.is_null()
                || unsafe {
                    SetInformationJobObject(
                        job,
                        JobObjectExtendedLimitInformation,
                        &limits as *const _ as _,
                        std::mem::size_of_val(&limits) as u32,
                    )
                } == 0
                || unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as _) } == 0
            {
                let error = std::io::Error::last_os_error();
                let _ = child.kill();
                let _ = child.wait();
                if !job.is_null() {
                    unsafe {
                        CloseHandle(job);
                    }
                }
                return Err(error.into());
            }
            Ok(Self(child, job))
        }
        #[cfg(unix)]
        Ok(Self(child))
    }
}
impl Drop for ProcessTree {
    fn drop(&mut self) {
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.1);
        }
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct RawTerminal;
impl RawTerminal {
    fn open() -> Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        Ok(Self)
    }
}
impl Drop for RawTerminal {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn task_names_cannot_inject_shell_commands() {
        assert!(command("npm-test && echo bad").is_err());
        assert_eq!(command("cargo-test").unwrap().1, vec!["test", "--locked"]);
    }
}
