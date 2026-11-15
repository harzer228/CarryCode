use crate::{folders, platform};
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    fs::{self, File, Metadata},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

const MAX_FILE: u64 = 1024 * 1024;
const MAX_EXCERPT: usize = 8192;
const MAX_PROPOSAL: usize = 64 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    path: PathBuf,
    reason: String,
    before: String,
    after: String,
}

struct Proposal {
    root: PathBuf,
    relative: PathBuf,
    original: String,
    updated: String,
    removed: String,
    added: String,
}

impl Proposal {
    fn new(root: &Path, relative: &Path, before: &str, after: &str) -> Result<Self> {
        ensure!(
            before.len() <= MAX_EXCERPT && after.len() <= MAX_EXCERPT,
            "{}",
            tr!("Выбери фрагменты до 8 КиБ.", "Choose excerpts up to 8 KiB.")
        );
        let root = root.canonicalize()?;
        let path = target(&root, relative)?;
        let original = read_text(&path)?;
        let (removed, added) = if original.contains("\r\n") {
            (
                before.replace("\r\n", "\n").replace('\n', "\r\n"),
                after.replace("\r\n", "\n").replace('\n', "\r\n"),
            )
        } else {
            (before.to_owned(), after.to_owned())
        };
        ensure!(
            !removed.is_empty()
                && removed != added
                && original.find(&removed).is_some()
                && original.find(&removed) == original.rfind(&removed),
            "{}",
            tr!(
                "Старый фрагмент должен встречаться ровно один раз и отличаться от нового.",
                "The old excerpt must appear exactly once and differ from the replacement."
            )
        );
        let updated = original.replacen(&removed, &added, 1);
        ensure!(
            updated.len() as u64 <= MAX_FILE,
            "{}",
            tr!("Файл больше 1 МиБ", "The file exceeds 1 MiB")
        );
        Ok(Self {
            root,
            relative: relative.to_owned(),
            original,
            updated,
            removed,
            added,
        })
    }

    fn apply(self, approved: bool) -> Result<bool> {
        if !approved {
            return Ok(false);
        }
        platform::require_standard_user()?;
        let path = target(&self.root, &self.relative)?;
        File::options().write(true).open(&path)?;
        let parent = path.parent().context(tr!(
            "Нет родительской папки",
            "The parent folder is missing"
        ))?;
        let stage = tempfile::Builder::new()
            .prefix(".carrycode-")
            .tempdir_in(parent)?;
        platform::protect(stage.path())?;
        let draft = stage.path().join("replacement");
        let mut file = File::create_new(&draft)?;
        file.write_all(self.updated.as_bytes())?;
        file.sync_all()?;
        drop(file);
        ensure!(
            target(&self.root, &self.relative)? == path && read_text(&path)? == self.original,
            "{}",
            tr!(
                "Файл изменился после просмотра. Подготовь предложение заново.",
                "The file changed after review. Prepare the proposal again."
            )
        );
        replace(&path, &draft, stage)?;
        Ok(true)
    }
}

pub fn review(root: &Path) -> Result<()> {
    let Some(input) = read_proposal()? else {
        return Ok(());
    };
    let request = parse_request(&input)?;
    let proposal = Proposal::new(root, &request.path, &request.before, &request.after)?;
    println!(
        "{}",
        trf!(
            "\nИзменение файла {}",
            "\nChange to {}",
            folders::display(&proposal.root.join(&proposal.relative))
        )
    );
    println!("{}", tr!("Обоснование агента", "Agent's explanation"));
    preview(' ', &request.reason);
    preview('-', &proposal.removed);
    preview('+', &proposal.added);

    // The approval code is generated after the paste, so a proposal cannot supply it.
    let mut nonce = [0; 3];
    getrandom::fill(&mut nonce).map_err(|error| anyhow::anyhow!("{error}"))?;
    let code = format!("{:02X}{:02X}{:02X}", nonce[0], nonce[1], nonce[2]);
    let answer = crate::prompt(&trf!(
        "Введи {code}, чтобы применить замену, или Enter для отмены: ",
        "Type {code} to apply the replacement, or press Enter to cancel: "
    ))?;
    if proposal.apply(answer.as_deref() == Some(code.as_str()))? {
        println!(
            "{}",
            tr!(
                "Замена применена. Повтори действие, при котором возникала ошибка.",
                "Replacement applied. Repeat the action that triggered the issue."
            )
        );
    } else {
        println!("{}", tr!("Замена отменена.", "Replacement canceled."));
    }
    Ok(())
}

fn parse_request(input: &str) -> Result<Request> {
    let input = input.trim();
    let fence = "\x60\x60\x60";
    let json = input
        .strip_prefix(fence)
        .and_then(|body| body.strip_prefix("json"))
        .and_then(|body| body.strip_suffix(fence))
        .unwrap_or(input);
    let request: Request = serde_json::from_str(json).context(tr!(
        "Вставь один блок JSON с полями path, reason, before и after.",
        "Paste one JSON block with path, reason, before, and after fields."
    ))?;
    ensure!(
        !request.reason.trim().is_empty() && request.reason.len() <= 2000,
        "{}",
        tr!(
            "Добавь краткое обоснование правки.",
            "Include a short explanation of the change."
        )
    );
    Ok(request)
}

fn read_proposal() -> Result<Option<String>> {
    println!(
        "{}",
        tr!(
            "\nВставь предложенный JSON. Закончи строкой :end или введи :cancel для отмены.",
            "\nPaste the proposed JSON. Enter :end on its own line, or enter :cancel."
        )
    );
    let mut input = String::new();
    loop {
        let mut line = String::new();
        if io::stdin().read_line(&mut line)? == 0 {
            return Ok(None);
        }
        match line.trim_end_matches(['\r', '\n']) {
            ":end" => return Ok((!input.trim().is_empty()).then_some(input)),
            ":cancel" => return Ok(None),
            _ => {
                ensure!(
                    input.len() + line.len() <= MAX_PROPOSAL,
                    "{}",
                    tr!("Слишком большой блок JSON.", "The JSON block is too large.")
                );
                input.push_str(&line);
            }
        }
    }
}

fn preview(prefix: char, excerpt: &str) {
    for line in excerpt.split('\n').filter(|_| !excerpt.is_empty()) {
        let mut safe = String::new();
        for ch in line.trim_end_matches('\r').chars() {
            if ch.is_control() || matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}') {
                safe.extend(ch.escape_default());
            } else {
                safe.push(ch);
            }
        }
        println!("{prefix} {safe}");
    }
}

fn target(root: &Path, relative: &Path) -> Result<PathBuf> {
    ensure!(
        crate::bundle::valid(relative),
        "{}",
        tr!(
            "Укажи путь внутри проекта.",
            "Enter a path inside the project."
        )
    );
    let mut path = root.to_owned();
    for component in relative.components() {
        path.push(component);
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(
            !is_link(&metadata),
            "{}",
            tr!(
                "Исправление через ссылку недоступно.",
                "Repair through a link is unavailable."
            )
        );
    }
    ensure!(
        path.canonicalize()?.starts_with(root) && path.is_file(),
        "{}",
        tr!(
            "Выбери существующий файл внутри проекта.",
            "Choose an existing file inside the project."
        )
    );
    Ok(path)
}

fn is_link(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes()
            & windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT
            != 0
    }
    #[cfg(not(windows))]
    metadata.is_symlink()
}

fn read_text(path: &Path) -> Result<String> {
    let mut data = String::new();
    File::open(path)?
        .take(MAX_FILE + 1)
        .read_to_string(&mut data)
        .context(tr!(
            "Файл должен быть текстом UTF-8.",
            "The file must be UTF-8 text."
        ))?;
    ensure!(
        data.len() as u64 <= MAX_FILE && !data.contains('\0'),
        "{}",
        tr!(
            "Выбери текстовый файл до 1 МиБ.",
            "Choose a text file up to 1 MiB."
        )
    );
    Ok(data)
}

#[cfg(target_os = "linux")]
fn replace(path: &Path, draft: &Path, _stage: tempfile::TempDir) -> Result<()> {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let metadata = fs::metadata(path)?;
    ensure!(
        metadata.uid() == unsafe { libc::geteuid() } && metadata.mode() & 0o7000 == 0,
        "{}",
        tr!(
            "Выбери обычный файл, которым владеет твой пользователь.",
            "Choose an ordinary file owned by your user."
        )
    );
    let draft_metadata = fs::metadata(draft)?;
    if metadata.gid() != draft_metadata.gid() {
        std::os::unix::fs::chown(draft, None, Some(metadata.gid()))?;
    }
    if metadata.permissions() != draft_metadata.permissions() {
        fs::set_permissions(draft, fs::Permissions::from_mode(metadata.mode() & 0o777))?;
    }
    match xattr::list(path) {
        Ok(keys) => {
            for key in keys {
                let value = xattr::get(path, &key)?
                    .context(tr!("Атрибут файла изменился", "A file attribute changed"))?;
                xattr::set(draft, key, &value)?;
            }
        }
        Err(error) if error.raw_os_error() == Some(libc::ENOTSUP) => {}
        Err(error) => return Err(error.into()),
    }
    File::open(draft)?.sync_all()?;
    fs::rename(draft, path)?;
    platform::sync_directory(path.parent().unwrap()).context(tr!(
        "Замена записана, но не удалось подтвердить её сохранение на диске.",
        "The replacement was written, but its persistence could not be verified."
    ))
}

#[cfg(windows)]
fn replace(path: &Path, draft: &Path, stage: tempfile::TempDir) -> Result<()> {
    use std::ptr::null_mut;
    use windows_sys::Win32::Storage::FileSystem::ReplaceFileW;
    let backup = stage.path().join("original");
    let success = unsafe {
        ReplaceFileW(
            platform::wide(path.as_os_str()).as_ptr(),
            platform::wide(draft.as_os_str()).as_ptr(),
            platform::wide(backup.as_os_str()).as_ptr(),
            0,
            null_mut(),
            null_mut(),
        )
    };
    if success == 0 {
        let error = io::Error::last_os_error();
        if backup.is_file() && (path.exists() || fs::rename(&backup, path).is_err()) {
            let recovery = stage.keep().join("original");
            return Err(error).with_context(|| {
                trf!(
                    "Исходный файл сохранён в {}",
                    "The original file is available at {}",
                    folders::display(&recovery)
                )
            });
        }
        return Err(error).context(tr!(
            "Не удалось заменить файл с сохранением его разрешений",
            "Could not replace the file while preserving its permissions"
        ));
    }
    File::options()
        .write(true)
        .open(path)?
        .sync_all()
        .context(tr!(
            "Замена записана, но не удалось подтвердить её сохранение на диске.",
            "The replacement was written, but its persistence could not be verified."
        ))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preview_accepts_one_json_proposal_with_an_explanation() -> Result<()> {
        let input = r#"{"path":"src/config.txt","reason":"Correct the option","before":"off","after":"on"}"#;
        let request = parse_request(input)?;
        assert_eq!(request.path, Path::new("src/config.txt"));
        assert_eq!(request.before, "off");
        assert!(parse_request(&input.replace("Correct the option", " ")).is_err());
        assert!(parse_request(&input.replace("\"after\"", "\"command\"")).is_err());
        assert!(parse_request(&format!("{input}\n1")).is_err());
        Ok(())
    }

    #[test]
    fn repair_requires_explicit_confirmation_and_preserves_crlf() -> Result<()> {
        let root = tempfile::tempdir()?;
        let relative = Path::new("test.txt");
        let path = root.path().join(relative);
        fs::write(&path, "before\r\nkeep\r\n")?;
        let proposal = Proposal::new(root.path(), relative, "before\nkeep", "after\nkeep")?;
        assert_eq!(proposal.updated, "after\r\nkeep\r\n");
        assert!(!proposal.apply(false)?);
        assert_eq!(fs::read_to_string(&path)?, "before\r\nkeep\r\n");
        if !platform::is_elevated()? {
            assert!(Proposal::new(root.path(), relative, "before", "after")?.apply(true)?);
            assert_eq!(fs::read_to_string(path)?, "after\r\nkeep\r\n");
        }
        Ok(())
    }

    #[test]
    fn repair_rejects_escape_ambiguous_matches_and_changes_after_review() -> Result<()> {
        let root = tempfile::tempdir()?;
        let relative = Path::new("test.txt");
        let path = root.path().join(relative);
        fs::write(&path, "same same")?;
        assert!(Proposal::new(root.path(), relative, "same", "new").is_err());
        assert!(Proposal::new(root.path(), Path::new("../outside.txt"), "a", "b").is_err());
        let proposal = Proposal::new(root.path(), relative, "same same", "new")?;
        fs::write(&path, "concurrent edit")?;
        assert!(proposal.apply(true).is_err());
        assert_eq!(fs::read_to_string(&path)?, "concurrent edit");
        Ok(())
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn repair_preserves_unix_mode_and_rejects_linked_files() -> Result<()> {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = tempfile::tempdir()?;
        let file = root.path().join("settings.txt");
        fs::write(&file, "before")?;
        fs::set_permissions(&file, fs::Permissions::from_mode(0o640))?;
        symlink(&file, root.path().join("linked.txt"))?;
        assert!(Proposal::new(root.path(), Path::new("linked.txt"), "before", "after").is_err());
        if !platform::is_elevated()? {
            assert!(
                Proposal::new(root.path(), Path::new("settings.txt"), "before", "after")?
                    .apply(true)?
            );
            assert_eq!(fs::metadata(file)?.permissions().mode() & 0o777, 0o640);
        }
        Ok(())
    }
}
