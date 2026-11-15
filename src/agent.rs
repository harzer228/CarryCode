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
    PlanRepair,
    Edit,
}

impl Mode {
    fn read_only(self) -> bool {
        matches!(self, Self::Diagnose | Self::PlanRepair)
    }
}

const REPAIR_PROMPT: &str = r#"Ask me what is broken if I have not explained it. Investigate with read-only tools. Explain your observations and uncertainty. If there is a useful change to one existing project text file, give me one JSON code block with string fields "path" (relative to the selected folder), "reason" (why the change may help), "before" (an exact unique excerpt), and "after" (its replacement). Keep the excerpts small. Do not apply changes or request administrator privileges. Reply in the language I use."#;

pub fn execute(
    root: &Path,
    work: &Path,
    directory: &Path,
    agent: Agent,
    mode: Mode,
    staged: bool,
) -> Result<ExitStatus> {
    crate::platform::require_standard_user()?;
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
                Mode::Diagnose | Mode::PlanRepair => ("read-only", "never"),
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
            if matches!(mode, Mode::PlanRepair) {
                cmd.args(["--no-alt-screen", "--", REPAIR_PROMPT]);
            }
        }
        Agent::OpenCode => {
            configure_opencode(work, mode)?;
            cmd.arg("--standalone").arg(directory);
            // Private mode keeps project configuration from overriding the launcher's settings.
            cmd.env("OPENCODE_DISABLE_PROJECT_CONFIG", "true");
            if matches!(mode, Mode::PlanRepair) {
                cmd.args(["--prompt", REPAIR_PROMPT]);
            }
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
    if mode.read_only() {
        for rule in ["edit:*", "shell:*", "external_directory:*", "subagent:*"] {
            policies.push(json!({ "action": "tool.use", "resource": rule, "effect": "deny" }));
        }
    }
    object.insert("policies".into(), Value::Array(policies));
    fs::write(config, serde_json::to_vec_pretty(&value)?)?;
    Ok(())
}

pub fn probe_version(program: &Path, work: &Path, agent: Agent, expected: &str) -> Result<String> {
    let output = command(program, work, work, agent)?
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
        actual
            .split_whitespace()
            .any(|part| part.trim_start_matches('v') == expected),
        "{}",
        trf!(
            "Неожиданная версия CLI: {}",
            "Unexpected CLI version: {}",
            actual.trim()
        )
    );
    Ok(expected.to_owned())
}

pub fn probe_codex_sandbox(program: &Path, work: &Path) -> Result<()> {
    let mut probe = command(program, work, work, Agent::Codex)?;
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repair_planning_denies_mutating_opencode_tools() -> Result<()> {
        let work = tempfile::tempdir()?;
        configure_opencode(work.path(), Mode::PlanRepair)?;
        let config: Value = serde_json::from_slice(&fs::read(
            work.path().join("oc-config/opencode/opencode.json"),
        )?)?;
        let policies = config["policies"].as_array().unwrap();
        for resource in ["edit:*", "shell:*", "external_directory:*", "subagent:*"] {
            assert!(
                policies
                    .contains(&json!({"action":"tool.use", "resource":resource, "effect":"deny"}))
            );
        }
        Ok(())
    }
}
