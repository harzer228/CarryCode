use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

#[cfg(windows)]
pub const PLATFORM: &str = "windows-amd64";
#[cfg(target_os = "linux")]
pub const PLATFORM: &str = "linux-amd64";

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Codex,
    OpenCode,
}

impl Agent {
    pub fn id(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::OpenCode => "opencode",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Codex => "Codex CLI",
            Self::OpenCode => "OpenCode CLI",
        }
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Marker {
    format: u32,
    pub id: String,
}

pub fn read_marker(root: &Path) -> Result<Marker> {
    let file = File::open(root.join("flashgpt.json")).context(tr!(
        "Нет flashgpt.json в каталоге комплекта",
        "flashgpt.json is missing from the bundle"
    ))?;
    let marker: Marker = serde_json::from_reader(file.take(4096))
        .context(tr!("Повреждён flashgpt.json", "Invalid flashgpt.json"))?;
    ensure!(
        marker.format == 1,
        "{}",
        tr!("Неизвестный формат комплекта", "Unknown bundle format")
    );
    ensure!(
        marker.id.len() == 32 && marker.id.bytes().all(|b| b.is_ascii_hexdigit()),
        "{}",
        tr!("Некорректный ID комплекта", "Invalid bundle ID")
    );
    Ok(marker)
}

pub fn init(root: &Path) -> Result<()> {
    let mut random = [0; 16];
    getrandom::fill(&mut random).map_err(|error| anyhow::anyhow!("{error}"))?;
    let marker = Marker {
        format: 1,
        id: format!("{:032x}", u128::from_be_bytes(random)),
    };
    let mut draft = tempfile::NamedTempFile::new_in(root)?;
    serde_json::to_writer_pretty(draft.as_file_mut(), &marker)?;
    draft.as_file().sync_all()?;
    draft.persist_noclobber(root.join("flashgpt.json"))?;
    Ok(())
}

#[derive(Deserialize)]
struct Manifest {
    runtimes: Vec<Runtime>,
}

#[derive(Deserialize)]
pub struct Runtime {
    platform: String,
    agent: String,
    pub version: String,
    entry: PathBuf,
    files: Vec<RuntimeFile>,
}

#[derive(Deserialize)]
struct RuntimeFile {
    path: PathBuf,
    sha256: String,
    executable: bool,
}

pub fn runtime(agent: Agent) -> Result<Runtime> {
    let manifest: Manifest =
        serde_json::from_str(include_str!(concat!(env!("OUT_DIR"), "/runtime.json")))?;
    manifest.runtimes.into_iter()
        .find(|runtime| runtime.platform == PLATFORM && runtime.agent == agent.id())
        .context(tr!("CLI отсутствует во встроенном манифесте. Собери комплект через xtask или распакуй архив релиза.", "The CLI is missing from the embedded manifest. Build the bundle with xtask or extract a release archive."))
}

pub fn is_full(root: &Path) -> bool {
    match File::open(root.join("manifests/edition.json")) {
        Ok(file) => serde_json::from_reader::<_, String>(file.take(128))
            .is_ok_and(|edition| edition == "full"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            [Agent::Codex, Agent::OpenCode].into_iter().any(|agent| {
                runtime(agent).is_ok_and(|runtime| {
                    valid(&runtime.entry)
                        && root
                            .join("bin")
                            .join(PLATFORM)
                            .join(agent.id())
                            .join(runtime.entry)
                            .is_file()
                })
            })
        }
        Err(_) => false,
    }
}

impl Runtime {
    pub fn prepare(&self, root: &Path, staging: Option<&Path>) -> Result<PathBuf> {
        let base = root.join("bin").join(&self.platform).join(&self.agent);
        let real_base = base
            .canonicalize()
            .context(tr!("Не найдены файлы CLI", "CLI files are missing"))?;
        ensure!(
            real_base.starts_with(root.canonicalize()?),
            "{}",
            tr!(
                "Пакет CLI указывает за пределы комплекта",
                "The CLI package points outside the bundle"
            )
        );
        ensure!(
            self.files.iter().any(|file| file.path == self.entry),
            "{}",
            tr!(
                "В манифесте нет точки входа",
                "The manifest has no entry point"
            )
        );
        let mut buffer = vec![0; 256 * 1024];
        for file in &self.files {
            ensure!(
                valid(&file.path),
                "{}",
                tr!(
                    "Недопустимый путь в манифесте",
                    "Invalid path in the manifest"
                )
            );
            let source = regular_file(&base, &file.path)?;
            let mut input = File::open(&source)?;
            let mut digest = Sha256::new();
            let destination = if let Some(staging) = staging {
                let destination = staging.join(&file.path);
                fs::create_dir_all(destination.parent().context(tr!(
                    "В пути нет родительской папки",
                    "The path has no parent folder"
                ))?)?;
                Some(destination)
            } else {
                None
            };
            let mut output = destination.as_ref().map(File::create).transpose()?;
            loop {
                let count = input.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                digest.update(&buffer[..count]);
                if let Some(output) = &mut output {
                    output.write_all(&buffer[..count])?;
                }
            }
            ensure!(
                format!("{:x}", digest.finalize()) == file.sha256,
                "{}",
                trf!(
                    "Не совпал SHA-256 файла {}",
                    "SHA-256 mismatch for {}",
                    source.display()
                )
            );
            if let Some(destination) = destination.filter(|_| file.executable) {
                make_executable(&destination)?;
            }
        }
        Ok(staging.unwrap_or(&base).join(&self.entry))
    }
}

pub fn valid(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path.components().all(|part| {
            matches!(part, Component::Normal(value)
            if value.to_str().is_some_and(|s| !s.contains(['\\', ':'])))
        })
}

fn regular_file(base: &Path, relative: &Path) -> Result<PathBuf> {
    let mut path = base.to_owned();
    for component in relative.components() {
        path.push(component);
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| trf!("Нет {}", "Missing {}", path.display()))?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "{}",
            trf!("Недопустимая ссылка {}", "Invalid link {}", path.display())
        );
    }
    if !path.is_file() || !path.canonicalize()?.starts_with(base.canonicalize()?) {
        bail!(
            "{}",
            trf!("Недопустимый файл {}", "Invalid file {}", path.display())
        );
    }
    Ok(path)
}

fn make_executable(_path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(_path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
