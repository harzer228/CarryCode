macro_rules! tr {
    ($ru:literal, $en:literal) => {
        if crate::language::english() { $en } else { $ru }
    };
}

macro_rules! trf {
    ($ru:literal, $en:literal $(, $arg:expr)* $(,)?) => {
        if crate::language::english() { format!($en $(, $arg)*) } else { format!($ru $(, $arg)*) }
    };
}

mod agent;
mod autostart;
mod bundle;
mod folders;
mod language;
mod platform;
mod profile;
mod recent;

use anyhow::{Context, Result, bail, ensure};
use bundle::Agent;
use std::{
    env,
    ffi::OsStr,
    io::{self, IsTerminal, Write},
    path::{Path, PathBuf},
    time::Duration,
};

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("flashGPT: {error:#}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run() -> Result<()> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    match args.first().and_then(|value| value.to_str()) {
        None if args.is_empty() => {
            let root = bundle_root()?;
            language::load(&root);
            menu(false, || Ok(root.clone()))
        }
        Some("watch") if args.len() == 1 => autostart::watch(false),
        Some("watch") if args.len() == 2 && args[1] == "--skip-current" => autostart::watch(true),
        Some("--attached") if args.len() == 1 => autostart::open_attached(),
        Some("init") if args.len() == 1 => marker(&bundle_root()?),
        Some("doctor") if args.len() == 1 => {
            let root = bundle_root()?;
            language::load(&root);
            marker(&root)?;
            agent::diagnose(&root)
        }
        Some("doctor") if args.len() == 3 && args[1] == "--connect" => {
            language::load(&bundle_root()?);
            connect(&args[2])
        }
        Some("autostart") if args.len() == 2 => {
            let root = bundle_root()?;
            language::load(&root);
            let action = args[1].to_string_lossy();
            if action == "install" {
                marker(&root)?;
            }
            autostart::run(&action, &root)
        }
        Some("--version" | "-V") if args.len() == 1 => {
            println!("flashGPT {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Some("--help" | "-h") if args.len() == 1 => {
            language::load(&bundle_root()?);
            println!(
                "flashGPT {}\n\n{}",
                env!("CARGO_PKG_VERSION"),
                tr!(
                    "flashgpt                        Открыть меню\nflashgpt doctor                 Проверить установку и песочницу\nflashgpt doctor --connect URL   Проверить DNS и HTTPS\nflashgpt autostart install|status|uninstall\n",
                    "flashgpt                        Open the menu\nflashgpt doctor                 Check the installation and sandbox\nflashgpt doctor --connect URL   Check DNS and HTTPS\nflashgpt autostart install|status|uninstall\n"
                )
            );
            Ok(())
        }
        _ => {
            language::load(&bundle_root()?);
            bail!(
                "{}",
                tr!(
                    "Неизвестная команда. Используй flashgpt --help.",
                    "Unknown command. Run flashgpt --help."
                )
            )
        }
    }
}

fn bundle_root() -> Result<std::path::PathBuf> {
    Ok(env::current_exe()?
        .parent()
        .context(tr!(
            "Не удалось определить каталог flashGPT",
            "Could not locate the flashGPT directory"
        ))?
        .to_owned())
}

fn menu(staged: bool, resolve_root: impl Fn() -> Result<PathBuf>) -> Result<()> {
    ensure!(
        io::stdin().is_terminal() && io::stdout().is_terminal(),
        "{}",
        tr!(
            "Открой flashGPT в интерактивном терминале",
            "Open flashGPT in an interactive terminal"
        )
    );
    println!("\nflashGPT {}\n", env!("CARGO_PKG_VERSION"));
    let mut profile = loop {
        let root = resolve_root()?;
        marker(&root)?;
        match profile::Profile::open(&root) {
            Ok(profile) => break profile,
            Err(error) => {
                eprintln!("flashGPT: {error:#}");
                if !prompt(tr!(
                    "Enter — попробовать снова, 0 — выйти: ",
                    "Enter to retry, 0 to exit: "
                ))?
                .is_some_and(|value| value.is_empty())
                {
                    return Ok(());
                }
            }
        }
    };
    if profile.is_new() {
        // Commit the password before the first agent launch or menu exit.
        profile.save().context(tr!(
            "Не удалось записать новый профиль на флешку",
            "Could not save the new profile on the USB drive"
        ))?;
        println!(
            "{}",
            tr!("Пароль профиля сохранён.", "Profile password saved.")
        );
    }
    let mut workspace = platform::home();
    loop {
        let startup = match autostart::enabled() {
            Ok(true) => tr!("вкл. — выключить", "on — turn off"),
            Ok(false) => tr!("выкл. — включить", "off — turn on"),
            Err(_) => tr!("статус недоступен", "status unavailable"),
        };
        println!(
            "{}",
            trf!(
                "\n1  Кодить с Codex\n2  Кодить с OpenCode\n3  Разобраться с проблемой\n4  Проверить флешку и соединение\n5  Автозапуск: {startup}\n6  Язык / Language\n7  Сменить пароль\n0  Выйти\n",
                "\n1  Code with Codex\n2  Code with OpenCode\n3  Troubleshoot a problem\n4  Check drive and connection\n5  Autostart: {startup}\n6  Language / Язык\n7  Change password\n0  Exit\n"
            )
        );
        match prompt(tr!("Выбери пункт: ", "Choose an option: "))?.as_deref() {
            Some("0") | None => {
                profile.close()?;
                println!(
                    "{}",
                    tr!(
                        "Сеанс закрыт. Флешку можно извлечь средствами ОС.",
                        "Session closed. You can now eject the USB drive."
                    )
                );
                return Ok(());
            }
            Some(choice @ ("1" | "2")) => {
                let agent = if choice == "1" {
                    Agent::Codex
                } else {
                    Agent::OpenCode
                };
                let result = run_agent(
                    &resolve_root,
                    staged,
                    &mut profile,
                    &mut workspace,
                    agent,
                    agent::Mode::Edit,
                );
                if let Err(error) = result {
                    eprintln!("flashGPT: {error:#}");
                }
            }
            Some("3") => {
                println!(
                    "{}",
                    tr!(
                        "Диагностика — агент читает файлы и объясняет причину.\nИсправление — агент может менять файлы и запускать команды с разрешениями выбранного CLI.",
                        "Diagnose — the agent reads files and explains the problem.\nRepair — the agent can edit files and run commands with the selected CLI's permissions."
                    )
                );
                let mode = match ask(tr!(
                    "1 — диагностика, 2 — исправление, 0 — назад: ",
                    "1 — Diagnose, 2 — Repair, 0 — Back: "
                ))?
                .as_str()
                {
                    "1" => agent::Mode::Diagnose,
                    "2" => agent::Mode::Edit,
                    _ => continue,
                };
                let agent = match ask(tr!(
                    "1 — Codex, 2 — OpenCode, 0 — назад: ",
                    "1 — Codex, 2 — OpenCode, 0 — Back: "
                ))?
                .as_str()
                {
                    "1" => Agent::Codex,
                    "2" => Agent::OpenCode,
                    _ => continue,
                };
                if let Err(error) = run_agent(
                    &resolve_root,
                    staged,
                    &mut profile,
                    &mut workspace,
                    agent,
                    mode,
                ) {
                    eprintln!("flashGPT: {error:#}");
                }
            }
            Some("4") => {
                let result = match ask(tr!(
                    "1 — проверить комплект, 2 — проверить HTTPS, 0 — назад: ",
                    "1 — Check the bundle, 2 — Check HTTPS, 0 — Back: "
                ))?
                .as_str()
                {
                    "1" => resolve_root().and_then(|root| agent::diagnose(&root)),
                    "2" => {
                        let url = ask(tr!(
                            "HTTPS URL провайдера [https://api.openai.com/]: ",
                            "Provider HTTPS URL [https://api.openai.com/]: "
                        ))?;
                        connect(OsStr::new(if url.is_empty() {
                            "https://api.openai.com/"
                        } else {
                            &url
                        }))
                    }
                    _ => continue,
                };
                if let Err(error) = result {
                    eprintln!("flashGPT: {error:#}");
                }
            }
            Some("5") => {
                let result = autostart::toggle(&resolve_root);
                if let Err(error) = result {
                    eprintln!("flashGPT: {error:#}");
                }
            }
            Some("6") => {
                if let Err(error) = choose_language(&resolve_root) {
                    eprintln!("flashGPT: {error:#}");
                }
            }
            Some("7") => match profile.change_password(&resolve_root) {
                Ok(profile::PasswordChange::Canceled) => {}
                Ok(profile::PasswordChange::Saved) => println!(
                    "{}",
                    tr!(
                        "Пароль изменён. При следующем запуске введи новый пароль.",
                        "Password changed. Use the new password the next time you open flashGPT."
                    )
                ),
                Ok(profile::PasswordChange::VerifyOnReconnect(error)) => {
                    eprintln!(
                        "{}",
                        trf!(
                            "Пароль заменён, но не удалось подтвердить запись: {error:#}\nСеанс закрывается. Переподключи флешку и проверь вход с новым паролем.",
                            "The password was replaced, but the write could not be verified: {error:#}\nClosing this session. Reconnect the drive and try opening the profile with the new password."
                        )
                    );
                    return profile.close();
                }
                Err(error) => eprintln!("flashGPT: {error:#}"),
            },
            _ => println!(
                "{}",
                tr!(
                    "Выбери один из пунктов меню.",
                    "Choose one of the menu options."
                )
            ),
        }
    }
}

fn choose_language(resolve_root: &impl Fn() -> Result<PathBuf>) -> Result<()> {
    let language = match ask("1 — Русский, 2 — English, 0 — Back / Назад: ")?.as_str()
    {
        "1" => language::Language::Ru,
        "2" => language::Language::En,
        _ => return Ok(()),
    };
    language::save(&resolve_root()?, language)?;
    println!(
        "{}",
        tr!(
            "Язык сохранён на флешке.",
            "Language saved on the USB drive."
        )
    );
    Ok(())
}

fn run_agent(
    resolve_root: &impl Fn() -> Result<PathBuf>,
    staged: bool,
    profile: &mut profile::Profile,
    workspace: &mut PathBuf,
    agent: Agent,
    mode: agent::Mode,
) -> Result<()> {
    let root = resolve_root()?;
    profile.set_root(&root)?;
    let Some(directory) = folders::pick(workspace, &root, &profile.recent)? else {
        return Ok(());
    };
    *workspace = directory;
    let root = resolve_root()?;
    profile.set_root(&root)?;
    println!(
        "{}",
        trf!(
            "{} · {}\nПроверяю файлы выбранного CLI…",
            "{} · {}\nChecking the selected CLI files…",
            agent.name(),
            folders::display(workspace)
        )
    );
    let result = agent::execute(&root, profile.work.path(), workspace, agent, mode, staged);
    if result.is_ok() {
        profile.recent.remember(&root, workspace);
    }
    finish_profile(profile, resolve_root)?;
    let status = result?;
    ensure!(
        status.success(),
        "{}",
        trf!(
            "{} завершился с {status}",
            "{} exited with {status}",
            agent.name()
        )
    );
    Ok(())
}

fn marker(root: &Path) -> Result<()> {
    match bundle::read_marker(root) {
        Ok(_) => Ok(()),
        Err(_) if !root.join("flashgpt.json").exists() => bundle::init(root),
        Err(error) => Err(error),
    }
}

fn finish_profile(
    profile: &mut profile::Profile,
    resolve_root: &impl Fn() -> Result<PathBuf>,
) -> Result<()> {
    loop {
        let saved = (|| -> Result<()> {
            profile.set_root(&resolve_root()?)?;
            profile.save()
        })();
        match saved {
            Ok(()) => {
                println!("{}", tr!("Профиль сохранён.", "Profile saved."));
                return Ok(());
            }
            Err(error) => eprintln!(
                "{}",
                trf!(
                    "Не удалось сохранить профиль: {error:#}",
                    "Could not save the profile: {error:#}"
                )
            ),
        }
        if prompt(tr!(
            "Подключи ту же флешку. [r] Повторить, [Enter] сохранить зашифрованную копию локально: ",
            "Reconnect the same USB drive. [r] Retry, [Enter] save an encrypted local copy: "
        ))?
            .is_some_and(|choice| choice.eq_ignore_ascii_case("r"))
        {
            continue;
        }
        let backup = profile.write_recovery()?;
        println!(
            "{}",
            trf!("Копия сохранена: {}", "Backup saved: {}", backup.display())
        );
        return Ok(());
    }
}

fn connect(value: &OsStr) -> Result<()> {
    let value = value
        .to_str()
        .context(tr!("URL должен быть в UTF-8", "The URL must use UTF-8"))?;
    ensure!(
        value.starts_with("https://") && !value.contains('@'),
        "{}",
        tr!(
            "Укажи HTTPS URL без данных входа",
            "Enter an HTTPS URL without credentials"
        )
    );
    let agent = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(8))
        .redirects(0)
        .try_proxy_from_env(true)
        .build();
    let status = match agent.head(value).call() {
        Ok(response) => response.status(),
        Err(ureq::Error::Status(code, _)) => code,
        Err(error) => bail!(
            "{}",
            trf!("HTTPS недоступен: {error}", "HTTPS unavailable: {error}")
        ),
    };
    println!(
        "{}",
        trf!(
            "HTTPS доступен. Сервер ответил HTTP {status}.",
            "HTTPS is reachable. The server returned HTTP {status}."
        )
    );
    Ok(())
}

fn prompt(message: &str) -> Result<Option<String>> {
    print!("{message}");
    io::stdout().flush()?;
    let mut input = String::new();
    if io::stdin().read_line(&mut input)? == 0 {
        return Ok(None);
    }
    Ok(Some(input.trim().to_owned()))
}

pub fn ask(message: &str) -> Result<String> {
    prompt(message)?.context(tr!("Ввод закрыт", "Input closed"))
}
