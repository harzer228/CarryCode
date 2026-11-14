use anyhow::{Context, Result, bail, ensure};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, BufReader, BufWriter, Read},
    path::{Component, Path, PathBuf},
    process::Command,
    time::Duration,
};
use zip::{ZipArchive, ZipWriter, write::SimpleFileOptions};

const WINDOWS: &str = "windows-amd64";
const LINUX: &str = "linux-amd64";
const LAUNCHERS: [(&str, &str); 2] = [(WINDOWS, "flashGPT.exe"), (LINUX, "flashgpt")];

#[derive(Deserialize)]
struct Lock {
    assets: Vec<Asset>,
}

#[derive(Deserialize)]
struct Asset {
    platform: String,
    agent: String,
    version: String,
    url: String,
    sha256: String,
}

#[derive(Serialize)]
struct Runtime {
    platform: String,
    agent: String,
    version: String,
    entry: String,
    files: Vec<RuntimeFile>,
}

#[derive(Serialize)]
struct RuntimeFile {
    path: String,
    sha256: String,
    executable: bool,
}

fn main() -> Result<()> {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let dist = repo.join("dist");
    let arguments: Vec<_> = env::args().skip(1).collect();
    match arguments.first().map(String::as_str) {
        Some("prepare") if arguments.len() == 1 => prepare(repo, &dist),
        Some("build") if arguments.len() <= 2 => {
            build(repo, &dist, arguments.get(1).map(String::as_str))
        }
        Some("package") if arguments.len() == 1 => package(repo, &dist),
        _ => bail!("Usage: cargo run -p xtask -- prepare | build [target] | package"),
    }
}

fn prepare(repo: &Path, dist: &Path) -> Result<()> {
    let spec: Lock = serde_json::from_reader(File::open(repo.join("runtimes.lock.json"))?)?;
    ensure!(spec.assets.len() == 4, "expected four locked assets");
    let downloads = dist.join("downloads");
    fs::create_dir_all(&downloads)?;
    let stage = tempfile::Builder::new()
        .prefix("runtimes-")
        .tempdir_in(dist)?;
    let mut runtimes = Vec::new();
    for asset in spec.assets {
        ensure!(
            matches!(asset.platform.as_str(), WINDOWS | LINUX),
            "unexpected platform"
        );
        ensure!(
            matches!(asset.agent.as_str(), "codex" | "opencode"),
            "unexpected agent"
        );
        let extension = if asset.url.ends_with(".zip") {
            "zip"
        } else {
            "tar.gz"
        };
        let os = if asset.platform == WINDOWS {
            "windows"
        } else {
            "linux"
        };
        let archive = downloads.join(format!("{}-{os}.{extension}", asset.agent));
        fetch(&asset, &archive)?;
        let base = stage
            .path()
            .join("bin")
            .join(&asset.platform)
            .join(&asset.agent);
        fs::create_dir_all(&base)?;
        let mut files = if extension == "zip" {
            extract_zip(&archive, &base)?
        } else {
            extract_tar(&archive, &base)?
        };
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let entry = if asset.agent == "codex" {
            "bin/codex"
        } else {
            "opencode"
        };
        let entry = if asset.platform == WINDOWS {
            format!("{entry}.exe")
        } else {
            entry.to_owned()
        };
        ensure!(
            files.iter().any(|file| file.path == entry),
            "missing {entry}"
        );
        runtimes.push(Runtime {
            platform: asset.platform,
            agent: asset.agent,
            version: asset.version,
            entry,
            files,
        });
    }
    let manifest = serde_json::to_vec_pretty(&serde_json::json!({ "runtimes": runtimes }))?;
    fs::write(stage.path().join("runtime.json"), manifest)?;
    let stage = stage.keep();
    let runtime = dist.join("runtime");
    let previous = dist.join("runtime.previous");
    // Both paths are constant children of the repository's dist directory.
    if previous.exists() {
        fs::remove_dir_all(&previous)?;
    }
    if runtime.exists() {
        fs::rename(&runtime, &previous)?;
    }
    if let Err(error) = fs::rename(&stage, &runtime) {
        if previous.exists() {
            fs::rename(&previous, &runtime)?;
        }
        return Err(error.into());
    }
    if previous.exists() {
        fs::remove_dir_all(previous)?;
    }
    println!("Prepared four verified runtimes in {}", runtime.display());
    Ok(())
}

fn fetch(asset: &Asset, path: &Path) -> Result<()> {
    if path.is_file() && hash(path)? == asset.sha256 {
        return Ok(());
    }
    ensure!(
        asset.url.starts_with("https://github.com/"),
        "unexpected release host"
    );
    println!(
        "Downloading {} {} ({})",
        asset.agent, asset.version, asset.platform
    );
    let agent = ureq::AgentBuilder::new()
        .try_proxy_from_env(true)
        .timeout(Duration::from_secs(300))
        .build();
    let response = agent.get(&asset.url).call()?;
    let mut draft = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    let size = io::copy(
        &mut response.into_reader().take(512 * 1024 * 1024 + 1),
        draft.as_file_mut(),
    )?;
    ensure!(
        size <= 512 * 1024 * 1024,
        "release archive exceeds maximum size"
    );
    ensure!(
        hash(draft.path())? == asset.sha256,
        "SHA-256 mismatch for {}",
        asset.url
    );
    draft.persist(path)?;
    Ok(())
}

fn extract_tar(archive: &Path, base: &Path) -> Result<Vec<RuntimeFile>> {
    let mut archive = tar::Archive::new(GzDecoder::new(BufReader::new(File::open(archive)?)));
    let mut files = Vec::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        let kind = entry.header().entry_type();
        if kind.is_dir() {
            continue;
        }
        ensure!(
            kind.is_file(),
            "unsupported archive entry {}",
            path.display()
        );
        let executable = entry.header().mode()? & 0o111 != 0;
        files.push(extract_file(base, &path, &mut entry, executable)?);
    }
    Ok(files)
}

fn extract_zip(archive: &Path, base: &Path) -> Result<Vec<RuntimeFile>> {
    let mut archive = ZipArchive::new(File::open(archive)?)?;
    let mut files = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        if entry.is_dir() {
            continue;
        }
        ensure!(!entry.is_symlink(), "archive contains a link");
        let path = entry.enclosed_name().context("unsafe zip path")?;
        files.push(extract_file(base, &path, &mut entry, false)?);
    }
    Ok(files)
}

fn extract_file(
    base: &Path,
    path: &Path,
    input: &mut dyn Read,
    executable: bool,
) -> Result<RuntimeFile> {
    let path_string = portable_path(path)?;
    let destination = base.join(path);
    fs::create_dir_all(destination.parent().unwrap())?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)?;
    io::copy(input, &mut output)?;
    drop(output);
    #[cfg(unix)]
    if executable {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o755))?;
    }
    Ok(RuntimeFile {
        path: path_string,
        sha256: hash(&destination)?,
        executable,
    })
}

fn portable_path(path: &Path) -> Result<String> {
    let pieces = path
        .components()
        .map(|part| match part {
            Component::Normal(part) => part
                .to_str()
                .filter(|part| !part.contains(['\\', ':']))
                .context("nonportable path"),
            _ => bail!("unsafe path"),
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(!pieces.is_empty(), "empty path");
    Ok(pieces.join("/"))
}

fn hash(path: &Path) -> Result<String> {
    let mut hash = Sha256::new();
    io::copy(
        &mut BufReader::with_capacity(256 * 1024, File::open(path)?),
        &mut hash,
    )?;
    Ok(format!("{:x}", hash.finalize()))
}

fn build(repo: &Path, dist: &Path, target: Option<&str>) -> Result<()> {
    let platform = match target {
        Some("x86_64-unknown-linux-musl" | "x86_64-unknown-linux-gnu") => LINUX,
        Some("x86_64-pc-windows-msvc") => WINDOWS,
        None if cfg!(target_os = "linux") => LINUX,
        None if cfg!(windows) => WINDOWS,
        _ => bail!("unsupported build target"),
    };
    let manifest = dist
        .join("runtime/runtime.json")
        .canonicalize()
        .context("run xtask prepare first")?;
    let cargo = env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let mut command = Command::new(cargo);
    command
        .current_dir(repo)
        .env("FASTGPT_RUNTIME_MANIFEST", manifest)
        .args([
            "build",
            "--release",
            "--locked",
            "-p",
            "flashgpt",
            "--bin",
            "flashgpt",
        ]);
    if let Some(target) = target {
        command.args(["--target", target]);
    }
    ensure!(command.status()?.success(), "Cargo build failed");
    let mut target_dir = env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repo.join("target"));
    if let Some(target) = target {
        target_dir.push(target);
    }
    let binary = if platform == WINDOWS {
        "flashgpt.exe"
    } else {
        "flashgpt"
    };
    let launcher_dir = dist.join("launchers").join(platform);
    fs::create_dir_all(&launcher_dir)?;
    let name = LAUNCHERS
        .iter()
        .find(|(entry, _)| *entry == platform)
        .unwrap()
        .1;
    fs::copy(
        target_dir.join("release").join(binary),
        launcher_dir.join(name),
    )?;
    println!("Built {platform} launcher");
    Ok(())
}

fn package(repo: &Path, dist: &Path) -> Result<()> {
    let mut files = Vec::new();
    for (platform, name) in LAUNCHERS {
        files.push((
            name.to_owned(),
            dist.join("launchers").join(platform).join(name),
            true,
        ));
    }
    let runtime = dist.join("runtime");
    collect(&runtime, Path::new("bin"), &mut files)?;
    files.push((
        "manifests/runtime.json".into(),
        runtime.join("runtime.json"),
        false,
    ));
    for file in [
        "README.md",
        "PLAN.md",
        "LICENSE",
        "SECURITY.md",
        "runtimes.lock.json",
    ] {
        files.push((file.into(), repo.join(file), false));
    }
    collect(repo, Path::new("licenses"), &mut files)?;
    files.push((
        "assets/flashgpt.png".into(),
        repo.join("assets/flashgpt.png"),
        false,
    ));
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let name = "flashgpt-usb-amd64.zip";
    let mut draft = tempfile::NamedTempFile::new_in(dist)?;
    let mut zip = ZipWriter::new(BufWriter::new(draft.as_file_mut()));
    for (name, path, executable) in files {
        let options = SimpleFileOptions::default()
            .unix_permissions(if executable { 0o755 } else { 0o644 })
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(1));
        zip.start_file(name, options)?;
        let mut source =
            File::open(&path).with_context(|| format!("missing {}", path.display()))?;
        io::copy(&mut source, &mut zip)?;
    }
    drop(zip.finish()?);
    draft.as_file().sync_all()?;
    draft.persist(dist.join(name))?;
    let sha = hash(&dist.join(name))?;
    fs::write(dist.join("SHA256SUMS"), format!("{sha}  {name}\n"))?;
    println!("{}", dist.join(name).display());
    Ok(())
}

fn collect(base: &Path, relative: &Path, files: &mut Vec<(String, PathBuf, bool)>) -> Result<()> {
    for entry in fs::read_dir(base.join(relative))? {
        let entry = entry?;
        let path = relative.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            collect(base, &path, files)?;
        } else {
            ensure!(entry.file_type()?.is_file(), "unsupported file in bundle");
            let path_string = portable_path(&path)?;
            let executable =
                path_string.starts_with("bin/linux-amd64/") && !path_string.ends_with(".json");
            files.push((path_string, entry.path(), executable));
        }
    }
    Ok(())
}
