use anyhow::{Context, Result, ensure};
use std::{
    ffi::OsString,
    fs, io,
    path::{Component, Path, PathBuf},
};

const PAGE_SIZE: usize = 15;

pub fn pick(start: &Path) -> Result<Option<PathBuf>> {
    let home = crate::platform::home();
    let mut current = start.to_owned();
    let mut page = 0;
    let mut show_hidden = false;
    loop {
        let folders = match list(&current, show_hidden) {
            Ok(folders) => folders,
            Err(error) => {
                eprintln!(
                    "{}",
                    trf!(
                        "Не удалось открыть {}: {error}",
                        "Could not open {}: {error}",
                        display(&current)
                    )
                );
                Vec::new()
            }
        };
        let pages = folders.len().div_ceil(PAGE_SIZE).max(1);
        page = page.min(pages - 1);
        let first = page * PAGE_SIZE;
        println!(
            "{}",
            trf!(
                "\nПапка: {}  ({}/{pages})",
                "\nFolder: {}  ({}/{pages})",
                display(&current),
                page + 1
            )
        );
        for (index, folder) in folders.iter().skip(first).take(PAGE_SIZE).enumerate() {
            println!("  {:>2}  {}/", index + 1, visible(folder));
        }
        if folders.is_empty() {
            println!(
                "{}",
                tr!("  Нет видимых подпапок.", "  No visible subfolders.")
            );
        }
        println!(
            "{}",
            tr!(
                "Enter — выбрать эту папку   Номер — открыть подпапку   .. — вверх",
                "Enter — select this folder   Number — open a subfolder   .. — up"
            )
        );
        if pages > 1 {
            println!(
                "{}",
                tr!(
                    "+ / - — следующая / предыдущая страница",
                    "+ / - — next / previous page"
                )
            );
        }
        println!(
            "{}",
            tr!(
                "d — диски   h — домой   p — ввести путь   n — новая папка   . — скрытые   0 — назад",
                "d — drives   h — home   p — enter a path   n — new folder   . — hidden   0 — back"
            )
        );
        let Some(choice) = crate::prompt(tr!("Папка > ", "Folder > "))? else {
            return Ok(None);
        };
        let next = match choice.as_str() {
            "" => {
                if current.parent().is_some() && fs::read_dir(&current).is_ok() {
                    return Ok(Some(current));
                }
                println!(
                    "{}",
                    tr!(
                        "Выбери доступную рабочую папку внутри диска.",
                        "Choose an accessible folder inside a drive."
                    )
                );
                continue;
            }
            "0" => return Ok(None),
            "+" => {
                page = (page + 1).min(pages - 1);
                continue;
            }
            "-" => {
                page = page.saturating_sub(1);
                continue;
            }
            "." => {
                show_hidden = !show_hidden;
                page = 0;
                continue;
            }
            ".." => current.parent().map(Path::to_owned),
            "h" => Some(home.clone()),
            "d" => choose_root()?,
            "p" => {
                let input = crate::ask(tr!("Путь [пусто — назад]: ", "Path [blank to go back]: "))?;
                let input = input.trim_matches('"');
                (!input.is_empty()).then(|| current.join(input))
            }
            "n" => {
                let name = crate::ask(tr!(
                    "Имя новой папки [пусто — назад]: ",
                    "New folder name [blank to go back]: "
                ))?;
                if name.is_empty() {
                    continue;
                }
                match create_folder(&current, &name) {
                    Ok(path) => Some(path),
                    Err(error) => {
                        eprintln!(
                            "{}",
                            trf!(
                                "Не удалось создать папку: {error:#}",
                                "Could not create the folder: {error:#}"
                            )
                        );
                        continue;
                    }
                }
            }
            _ => choice
                .parse::<usize>()
                .ok()
                .filter(|number| (1..=PAGE_SIZE).contains(number))
                .and_then(|number| folders.get(first + number - 1))
                .map(|folder| current.join(folder)),
        };
        if let Some(next) = next {
            match accessible(&next) {
                Ok(path) => {
                    current = path;
                    page = 0;
                }
                Err(error) => eprintln!("{error:#}"),
            }
        }
    }
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

fn choose_root() -> Result<Option<PathBuf>> {
    #[cfg(windows)]
    let roots = crate::platform::folder_roots();
    #[cfg(not(windows))]
    let roots = [PathBuf::from("/")];
    println!("{}", tr!("\nКорневые папки", "\nRoot folders"));
    for (index, root) in roots.iter().enumerate() {
        println!("  {}  {}", index + 1, display(root));
    }
    let choice = crate::ask(tr!("Номер [0 — назад]: ", "Number [0 to go back]: "))?;
    Ok(choice
        .parse::<usize>()
        .ok()
        .and_then(|choice| choice.checked_sub(1))
        .and_then(|index| roots.get(index).cloned()))
}

fn visible(value: &std::ffi::OsStr) -> String {
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
