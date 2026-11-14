use crate::bundle::{self, Agent};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    env, fs,
    path::Path,
    process::{Command, ExitStatus},
};

#[derive(Clone, Copy)]
pub enum Mode {
    Diagnose,
    Edit,
}

pub fn execute(
    root: &Path,
    work: &Path,
    directory: &Path,
    agent: Agent,
    mode: Mode,
    staged: bool,
) -> Result<ExitStatus> {
    let stage = staged.then(crate::platform::private_dir).transpose()?;
    let program =
        bundle::runtime(agent)?.prepare(root, stage.as_ref().map(tempfile::TempDir::path))?;
    crate::platform::check_execution(&program)?;
    let mut cmd = command(&program, directory, work, agent)?;
    println!("{}", trf!("Запускаю {}…", "Starting {}…", agent.name()));
    match agent {
        Agent::Codex => {
            #[cfg(windows)]
            crate::platform::configure_terminal(&mut cmd);
            let (sandbox, approval) = match mode {
                Mode::Diagnose => ("read-only", "never"),
                Mode::Edit => ("workspace-write", "on-request"),
            };
            cmd.args(["--no-daemon", "-s", sandbox, "-a", approval]);
            cmd.args([
                "-c",
                "cli_auth_credentials_store=\"file\"",
                "-c",
                "check_for_update_on_startup=false",
                "--disable",
                "hooks",
            ]);
        }
        Agent::OpenCode => {
            configure_opencode(work, mode)?;
            cmd.arg("--standalone").arg(directory);
            // Private mode keeps project configuration from overriding the launcher's settings.
            cmd.env("OPENCODE_DISABLE_PROJECT_CONFIG", "true");
        }
    }
    crate::platform::run_child(cmd).with_context(|| {
        trf!(
            "Не удалось запустить или завершить {}",
            "Could not start or stop {}",
            agent.name()
        )
    })
}

fn command(program: &Path, directory: &Path, work: &Path, agent: Agent) -> Result<Command> {
    let mut cmd = Command::new(program);
    cmd.current_dir(directory);
    for (key, _) in env::vars_os() {
        let name = key.to_string_lossy().to_ascii_uppercase();
        if inherited_setting(&name) {
            cmd.env_remove(key);
        }
    }
    let temp = work.join("tmp");
    fs::create_dir_all(&temp)?;
    for name in ["TEMP", "TMP", "TMPDIR"] {
        cmd.env(name, &temp);
    }
    match agent {
        Agent::Codex => {
            let home = work.join("codex");
            fs::create_dir_all(&home)?;
            cmd.env("CODEX_HOME", home);
        }
        Agent::OpenCode => {
            for (key, name) in [
                ("XDG_CONFIG_HOME", "oc-config"),
                ("XDG_DATA_HOME", "oc-data"),
                ("XDG_STATE_HOME", "oc-state"),
                ("XDG_CACHE_HOME", "oc-cache"),
            ] {
                let path = work.join(name);
                fs::create_dir_all(&path)?;
                cmd.env(key, path);
            }
            cmd.env("OPENCODE_DISABLE_AUTOUPDATE", "1");
        }
    }
    Ok(cmd)
}

fn inherited_setting(name: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "CODEX_",
        "OPENCODE_",
        "OPENAI_",
        "ANTHROPIC_",
        "AZURE_OPENAI_",
        "GOOGLE_APPLICATION_CREDENTIALS",
        "AWS_",
        "BUN_",
        "NODE_",
    ];
    PREFIXES.iter().any(|prefix| name.starts_with(prefix))
        || name.ends_with("_API_KEY")
        || name.ends_with("_ACCESS_TOKEN")
        || matches!(
            name,
            "GEMINI_API_TOKEN" | "GITHUB_TOKEN" | "GH_TOKEN" | "GOOGLE_CLOUD_PROJECT"
        )
}

fn configure_opencode(work: &Path, mode: Mode) -> Result<()> {
    let config_dir = work.join("oc-config/opencode");
    fs::create_dir_all(&config_dir)?;
    let config = config_dir.join("opencode.json");
    let mut value = if config.exists() {
        serde_json::from_slice(&fs::read(&config)?)?
    } else {
        json!({})
    };
    let object = value.as_object_mut().context(tr!(
        "Настройки OpenCode должны быть JSON-объектом",
        "OpenCode settings must be a JSON object"
    ))?;
    object.insert("update".into(), json!("disable"));
    object.insert(
        "permissions".into(),
        json!([
            { "action": "edit", "resource": "*", "effect": "ask" },
            { "action": "shell", "resource": "*", "effect": "ask" },
            { "action": "external_directory", "resource": "*", "effect": "ask" }
        ]),
    );
    let mut policies = vec![
        json!({ "action": "plugin.use", "resource": "*", "effect": "deny" }),
        json!({ "action": "mcp.use", "resource": "*", "effect": "deny" }),
        json!({ "action": "skill.use", "resource": "*", "effect": "deny" }),
    ];
    if matches!(mode, Mode::Diagnose) {
        for rule in ["edit:*", "shell:*", "external_directory:*", "subagent:*"] {
            policies.push(json!({ "action": "tool.use", "resource": rule, "effect": "deny" }));
        }
    }
    object.insert("policies".into(), Value::Array(policies));
    fs::write(config, serde_json::to_vec_pretty(&value)?)?;
    Ok(())
}

pub fn diagnose(root: &Path) -> Result<()> {
    crate::bundle::read_marker(root)?;
    println!("{}", trf!("Носитель: {}", "Drive: {}", root.display()));
    println!(
        "{}",
        trf!(
            "Свободно на носителе: {} МиБ",
            "Free space on the drive: {} MiB",
            crate::platform::available(root)? / (1024 * 1024)
        )
    );
    tempfile::NamedTempFile::new_in(root).context(tr!(
        "Каталог комплекта доступен только для чтения",
        "The bundle folder is read-only"
    ))?;
    let work = crate::platform::private_dir()?;
    println!("{}", tr!("Временный каталог: OK", "Temporary folder: OK"));
    let mut healthy = true;
    for agent in [Agent::Codex, Agent::OpenCode] {
        let result = diagnose_agent(root, work.path(), agent);
        match result {
            Ok(version) => println!("{} {version}: OK", agent.name()),
            Err(error) => {
                healthy = false;
                println!("{}: {error:#}", agent.name());
            }
        }
    }
    ensure!(
        healthy,
        "{}",
        tr!(
            "Одна или несколько проверок завершились ошибкой",
            "One or more checks failed"
        )
    );
    Ok(())
}

fn diagnose_agent(root: &Path, work: &Path, agent: Agent) -> Result<String> {
    let runtime = bundle::runtime(agent)?;
    let program = runtime.prepare(root, None)?;
    crate::platform::check_execution(&program)?;
    let output = command(&program, work, work, agent)?
        .arg("--version")
        .output()?;
    ensure!(
        output.status.success(),
        "{}",
        tr!(
            "Не удалось получить версию CLI",
            "Could not read the CLI version"
        )
    );
    let actual = String::from_utf8_lossy(&output.stdout);
    ensure!(
        actual.contains(&runtime.version),
        "{}",
        trf!(
            "Неожиданная версия CLI: {}",
            "Unexpected CLI version: {}",
            actual.trim()
        )
    );
    if agent == Agent::Codex {
        let mut probe = command(&program, work, work, agent)?;
        probe.args([
            "sandbox",
            "--include-managed-config",
            "--permission-profile",
            ":read-only",
            "--",
        ]);
        if cfg!(windows) {
            probe.args(["cmd.exe", "/d", "/c", "exit", "0"]);
        } else {
            probe.arg("/bin/true");
        }
        let output = probe.output()?;
        ensure!(
            output.status.success(),
            "{}",
            trf!(
                "Песочница: {}",
                "Sandbox: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )
        );
    }
    Ok(runtime.version)
}
