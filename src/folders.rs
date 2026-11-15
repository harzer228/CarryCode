mod picker;

use crate::recent::RecentFolders;
use anyhow::{Context, Result, ensure};
use std::{
    ffi::{OsStr, OsString},
    fs, io,
    path::{Component, Path, PathBuf},
};

pub fn pick(start: &Path, root: &Path, recent: &RecentFolders) -> Result<Option<PathBuf>> {
    // A minimal path prompt keeps selection usable in terminals without screen support.
    if !cfg!(windows) && std::env::var_os("TERM").as_deref() == Some(OsStr::new("dumb")) {
        return pick_by_path(start);
    }
    let Ok(screen) = picker::Screen::open() else {
        return pick_by_path(start);
    };
    picker::run(screen, start, root, recent)
}

fn pick_by_path(start: &Path) -> Result<Option<PathBuf>> {
    println!("{}", display(start));
    let Some(input) = crate::prompt(tr!(
        "Путь к папке [Enter — текущая, 0 — назад]: ",
        "Folder path [Enter for current, 0 to go back]: "
    ))?
    else {
        return Ok(None);
    };
    if input == "0" {
        return Ok(None);
    }
    let path = accessible(&start.join(input.trim_matches('"')))?;
    selectable(&path)?;
    Ok(Some(path))
}

fn list(path: &Path, show_hidden: bool) -> io::Result<Vec<OsString>> {
    let mut folders = Vec::new();
    for entry in fs::read_dir(path)?.filter_map(Result::ok) {
        let name = entry.file_name();
        let hidden = name.to_string_lossy().starts_with('.');
        if show_hidden || !hidden {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() || (kind.is_symlink() && entry.path().is_dir()) {
                folders.push(name);
            }
        }
    }
    folders.sort_by_cached_key(|folder| folder.to_string_lossy().to_lowercase());
    Ok(folders)
}

fn accessible(path: &Path) -> Result<PathBuf> {
    let result = path
        .canonicalize()
        .with_context(|| trf!("Не удалось открыть {}", "Could not open {}", display(path)))?;
    fs::read_dir(&result).with_context(|| {
        trf!(
            "Не удалось прочитать {}",
            "Could not read {}",
            display(path)
        )
    })?;
    Ok(result)
}

fn selectable(path: &Path) -> Result<()> {
    ensure!(
        path.parent().is_some(),
        "{}",
        tr!(
            "Выбери папку внутри диска.",
            "Choose a folder inside the drive."
        )
    );
    Ok(())
}

fn create_folder(parent: &Path, name: &str) -> Result<PathBuf> {
    let mut parts = Path::new(name).components();
    ensure!(
        !name.contains(['/', '\\', ':'])
            && matches!(parts.next(), Some(Component::Normal(_)))
            && parts.next().is_none(),
        "{}",
        tr!("Введи имя одной папки", "Enter a single folder name")
    );
    let path = parent.join(name);
    fs::create_dir(&path)?;
    Ok(path)
}

fn visible(value: &OsStr) -> String {
    value
        .to_string_lossy()
        .chars()
        .map(|character| {
            if character.is_control() {
                '�'
            } else {
                character
            }
        })
        .collect()
}

pub fn display(path: &Path) -> String {
    let value = visible(path.as_os_str());
    #[cfg(windows)]
    {
        if let Some(unc) = value.strip_prefix(r"\\?\UNC\") {
            return format!(r"\\{unc}");
        }
        if let Some(disk) = value.strip_prefix(r"\\?\")
            && disk.as_bytes().get(1) == Some(&b':')
        {
            return disk.to_owned();
        }
    }
    value
}
