use crate::{agent, bundle, folders, platform};
use anyhow::{Context, Result, bail, ensure};
use std::{ffi::OsStr, io::Write, path::Path, time::Duration};

#[derive(Default)]
struct Report {
    passed: usize,
    failed: usize,
    skipped: usize,
}

impl Report {
    fn check<T>(&mut self, label: &str, result: Result<T>, next: &str) -> Option<T> {
        match result {
            Ok(value) => {
                self.passed += 1;
                println!("  [OK] {label}");
                Some(value)
            }
            Err(error) => {
                self.failed += 1;
                println!("  [{}] {label}: {error:#}", tr!("Сбой", "Failed"));
                println!("       {} {next}", tr!("Дальше.", "Next."));
                None
            }
        }
    }

    fn skip(&mut self, label: &str) {
        self.skipped += 1;
        println!("  [{}] {label}", tr!("Пропущено", "Skipped"));
    }

    fn finish(self) -> Result<()> {
        println!(
            "{}",
            trf!(
                "\nИтого: прошли {}, сбой {}, пропущены {}. Эти проверки не определяют причину сбоя и не проверяют вход в аккаунт.",
                "\nSummary: {} passed, {} failed, {} skipped. These checks do not determine the cause or verify account sign-in.",
                self.passed,
                self.failed,
                self.skipped
            )
        );
        ensure!(
            self.failed == 0,
            "{}",
            tr!(
                "Есть проверки со сбоем. См. шаги выше.",
                "Some checks failed. See the next steps above."
            )
        );
        Ok(())
    }
}

pub fn bundle(root: &Path) -> Result<()> {
    println!(
        "{}",
        trf!("\nКомплект: {}", "\nBundle: {}", folders::display(root))
    );
    let mut report = Report::default();
    report.check(
        tr!("ID комплекта", "Bundle ID"), bundle::read_marker(root),
        tr!("Запусти CarryCode в папке комплекта. Если там есть профиль, восстанови исходный flashgpt.json вместе с ним.",
            "Open CarryCode in the bundle folder. For an existing profile, restore its original flashgpt.json alongside it."),
    );
    if let Some(bytes) = report.check(
        tr!("Место на диске", "Disk space"),
        platform::available(root),
        tr!(
            "Проверь, что носитель подключён и доступен.",
            "Check that the drive is connected and accessible."
        ),
    ) {
        println!(
            "{}",
            trf!(
                "       Свободно: {} МиБ",
                "       Available: {} MiB",
                bytes >> 20
            )
        );
    }
    report.check(
        tr!("Запись на носитель", "Write to the drive"),
        probe_write(root),
        tr!(
            "Проверь свободное место и разрешение на запись в папку комплекта.",
            "Check available space and write permissions for the bundle folder."
        ),
    );
    let work = report.check(
        tr!("Приватный временный каталог", "Private temporary folder"),
        platform::private_dir(),
        tr!(
            "Проверь место и права записи для временных файлов своего пользователя.",
            "Check available space and write permissions for your user's temporary files."
        ),
    );
    let standard = report.check(
        tr!("Обычные права пользователя", "Standard user privileges"),
        platform::require_standard_user(),
        tr!(
            "Повтори проверку из терминала без повышенных прав.",
            "Repeat the check in an unelevated terminal."
        ),
    );
    let full = bundle::is_full(root);
    for cli in [bundle::Agent::Codex, bundle::Agent::OpenCode] {
        if !full {
            report.skip(cli.name());
            continue;
        }
        println!("\n{}", cli.name());
        let files = report.check(
            tr!("Манифест и целостность файлов", "Manifest and file integrity"),
            bundle::runtime(cli).and_then(|runtime| Ok((runtime.prepare(root, None)?, runtime.version))),
            tr!("Распакуй проверенный ZIP или переустанови комплект в ту же папку. Сохрани профиль и flashgpt.json.",
                "Extract a verified ZIP or reinstall into this folder. Keep the profile and flashgpt.json."),
        );
        let version_label = tr!("Запуск и версия CLI", "CLI startup and version");
        let sandbox_label = tr!("Пробный запуск песочницы Codex", "Codex sandbox probe");
        if let (Some((program, expected)), Some(work), Some(())) = (files, work.as_ref(), standard)
        {
            let version = report.check(
                version_label,
                platform::check_execution(&program).and_then(|()| agent::probe_version(&program, work.path(), cli, &expected)),
                tr!("Проверь, что ОС разрешает запуск. В Linux проверь флаг выполнения и параметр noexec.",
                    "Check that the OS permits execution. On Linux, check executable permissions and the noexec mount option."),
            );
            if let Some(version) = &version {
                println!("       {version}");
            }
            if cli == bundle::Agent::Codex {
                if version.is_some() {
                    report.check(
                        sandbox_label, agent::probe_codex_sandbox(&program, work.path()),
                        tr!("Проверь поддержку песочницы на этом ПК и повтори пробу из обычного терминала.",
                            "Check sandbox support on this computer and retry in a regular terminal."),
                    );
                } else {
                    report.skip(sandbox_label);
                }
            }
        } else {
            report.skip(version_label);
            if cli == bundle::Agent::Codex {
                report.skip(sandbox_label);
            }
        }
    }
    report.finish()
}

fn probe_write(root: &Path) -> Result<()> {
    let mut probe = tempfile::NamedTempFile::new_in(root)?;
    probe.write_all(b"CarryCode")?;
    probe.as_file().sync_all()?;
    probe.close()?;
    Ok(())
}

pub fn connect(value: &OsStr) -> Result<()> {
    let value = value
        .to_str()
        .context(tr!("URL должен быть в UTF-8", "The URL must use UTF-8"))?;
    ensure!(
        value.starts_with("https://")
            && !value.contains(['@', '?', '#'])
            && !value.chars().any(char::is_whitespace),
        "{}",
        tr!(
            "Укажи HTTPS-адрес без логина, параметров и фрагмента.",
            "Enter an HTTPS URL without credentials, query parameters, or a fragment."
        )
    );
    println!(
        "{}",
        tr!(
            "Проверка HTTPS: запрос HEAD без авторизации.",
            "HTTPS check: sending an unauthenticated HEAD request."
        )
    );
    let client = ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(8))
        .redirects(0)
        .try_proxy_from_env(true)
        .build();
    let status = match client.head(value).call() {
        Ok(response) => response.status(),
        Err(ureq::Error::Status(code, _)) => code,
        Err(ureq::Error::Transport(error)) => {
            use ureq::ErrorKind::*;
            let (observation, next) = match error.kind() {
                InvalidUrl | UnknownScheme => (
                    tr!("адрес не разобран", "the URL could not be parsed"),
                    tr!("Проверь адрес провайдера.", "Check the provider URL."),
                ),
                Dns => (
                    tr!("не удалось разрешить имя", "the hostname did not resolve"),
                    tr!(
                        "Проверь имя хоста и DNS, затем повтори.",
                        "Check the hostname and DNS, then retry."
                    ),
                ),
                InvalidProxyUrl | ProxyConnect | ProxyUnauthorized => (
                    tr!("запрос через прокси не прошёл", "the proxy request failed"),
                    tr!(
                        "Проверь адрес прокси и доступ к нему.",
                        "Check the proxy address and your access to it."
                    ),
                ),
                _ => (
                    tr!("ответ HTTP не получен", "no HTTP response was received"),
                    tr!(
                        "Проверь подключение, прокси и сертификаты, затем повтори.",
                        "Check your connection, proxy and certificates, then retry."
                    ),
                ),
            };
            bail!(
                "{}",
                trf!(
                    "Проверка HTTPS: {observation}. {next}",
                    "HTTPS check: {observation}. {next}"
                )
            );
        }
    };
    println!(
        "{}",
        trf!(
            "Получен ответ HTTP {status}. Это подтверждает доступность HTTPS-адреса. Для проверки входа и модели отправь запрос из выбранного CLI.",
            "Received HTTP {status}. The HTTPS endpoint responded. To check your sign-in and model, send a request from the CLI."
        )
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_probe_leaves_no_artifacts() -> Result<()> {
        let drive = tempfile::tempdir()?;
        probe_write(drive.path())?;
        assert!(drive.path().read_dir()?.next().is_none());
        Ok(())
    }

    #[test]
    fn report_counts_failures_and_skips_separately() {
        let mut report = Report::default();
        assert_eq!(report.check("success", Ok(42), ""), Some(42));
        assert_eq!(
            report.check::<()>("failure", Err(anyhow::anyhow!("test")), "retry"),
            None
        );
        report.skip("dependent check");
        assert_eq!((report.passed, report.failed, report.skipped), (1, 1, 1));
        assert!(report.finish().is_err());
    }

    #[test]
    fn rejects_connect_requests_with_credentials_before_network_access() {
        for value in [
            "http://example.test",
            "https://user@example.test",
            "https://example.test/?secret=x",
        ] {
            assert!(connect(OsStr::new(value)).is_err());
        }
    }
}
