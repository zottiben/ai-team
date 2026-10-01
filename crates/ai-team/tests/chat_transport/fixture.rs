use ai_team_core::{
    Chat, NewAgent, NewChat, NewProject, NewRepo, Provider, Reasoning, Store,
    DEFAULT_MACHINE_PROFILE,
};
use anyhow::Context;
use serde_json::json;
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

pub(super) struct Fixture {
    pub(super) root: PathBuf,
    pub(super) pi: PathBuf,
    pub(super) repo: PathBuf,
    pub(super) db: PathBuf,
    directory: Option<tempfile::TempDir>,
    pub(super) passed: bool,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if !self.passed {
            if let Some(directory) = self.directory.take() {
                eprintln!("Failure evidence retained at {}", self.root.display());
                let _path = directory.keep();
            }
        }
    }
}

impl Fixture {
    pub(super) fn new(pi: &Path, adapter: &Path) -> anyhow::Result<Self> {
        let pi = pi
            .canonicalize()
            .context("existing Pi executable required")?;
        let adapter = adapter
            .canonicalize()
            .context("existing MCP adapter required")?;
        anyhow::ensure!(
            pi.is_file() && adapter.is_file(),
            "give executable/extension files"
        );
        let directory = tempfile::Builder::new()
            .prefix("ai-team-transport-")
            .tempdir()?;
        let root = directory.path().canonicalize()?;
        let fixture = Self {
            repo: root.join("repo"),
            db: root.join("team.sqlite"),
            root,
            pi: pi.clone(),
            directory: Some(directory),
            passed: false,
        };
        for dir in ["repo", "bin", "agent", "config/ai-team"] {
            std::fs::create_dir_all(fixture.root.join(dir))?;
        }
        isolate(&fixture.root)?;
        runtime(&fixture.root, &pi, &adapter)?;
        repository(&fixture.repo)?;
        std::fs::write(fixture.root.join("outside.txt"), "KEEP\n")?;
        Ok(fixture)
    }

    pub(super) fn store(&self) -> anyhow::Result<(Store, Chat)> {
        let mut store = Store::init(&self.db)?;
        let project = store.create_project(NewProject {
            name: "Transport".into(),
            ..Default::default()
        })?;
        store.attach_repo(
            project.id,
            NewRepo {
                main_path: Some(self.repo.to_string_lossy().into()),
                ..Default::default()
            },
        )?;
        let team =
            store.seed_default_team(project.id, &ai_team_core::RoleModelDefault::local_floor())?;
        // Catalogue isolation remains an explicit transport check, not a seeding side effect.
        assert!(!ai_team_core::ModelRegistry::load()?.models()?.is_empty());
        for mut agent in store.agents(team.id)? {
            agent.provider = Provider::Local;
            agent.model = format!("fixture-{}", agent.role);
            store.update_agent(agent.id, NewAgent::from(&agent))?;
        }
        let chat = store.create_chat(NewChat {
            project_id: project.id,
            workspace: self.repo.clone(),
            provider: Provider::Local,
            model: "fixture-solo".into(),
            reasoning: Reasoning::High,
        })?;
        Ok((store, chat))
    }
}

fn isolate(root: &Path) -> anyhow::Result<()> {
    // Before worker threads: no inherited agent overrides, cloud auth, proxy, or shell hooks.
    for (key, _) in std::env::vars_os() {
        if !["PATH", "TMPDIR", "LANG", "TERM"].contains(&key.to_string_lossy().as_ref()) {
            std::env::remove_var(key);
        }
    }
    for (key, path) in [
        ("HOME", root.to_path_buf()),
        ("BUILD_TEST_ROOT", root.to_path_buf()),
        ("XDG_CONFIG_HOME", root.join("config")),
        ("AI_TEAM_HOME", root.join("state")),
        ("PI_CODING_AGENT_DIR", root.join("agent")),
        ("PI_CODING_AGENT_SESSION_DIR", root.join("sessions")),
        ("AI_PLANNER_DB", root.join("standalone.sqlite")),
    ] {
        std::env::set_var(key, path);
    }
    for (key, value) in [
        ("PI_OFFLINE", "1"),
        ("PI_TELEMETRY", "0"),
        ("JITI_FS_CACHE", "0"),
        ("GIT_CONFIG_NOSYSTEM", "1"),
        ("GIT_CONFIG_GLOBAL", "/dev/null"),
        ("NO_UPDATE_NOTIFIER", "1"),
        ("npm_config_update_notifier", "false"),
        ("npm_config_offline", "true"),
        ("npm_config_audit", "false"),
        ("npm_config_fund", "false"),
        ("ANTHROPIC_API_KEY", "fixture-decoy"),
        ("OPENAI_API_KEY", "fixture-decoy"),
    ] {
        std::env::set_var(key, value);
    }
    std::env::set_var(
        "NODE_OPTIONS",
        format!("--require={}", root.join("offline.cjs").display()),
    );
    std::env::set_var(
        "PATH",
        format!("{}:{}", root.join("bin").display(), std::env::var("PATH")?),
    );
    std::fs::write(root.join("standalone.sqlite"), "STANDALONE_KEEP")?;
    std::fs::write(
        root.join("config/ai-team/machine.toml"),
        DEFAULT_MACHINE_PROFILE.replace("local = false", "local = true"),
    )?;
    Ok(())
}

fn runtime(root: &Path, pi: &Path, adapter: &Path) -> anyhow::Result<()> {
    let agent = root.join("agent");
    let bin = root.join("bin");
    std::fs::write(
        agent.join("mcp-adapter.json"),
        json!({"mcpServers":{"unscoped-sentinel":{
            "command":bin.join("aip"),"lifecycle":"eager","directTools":true
        }}})
        .to_string(),
    )?;
    std::fs::write(
        agent.join("settings.json"),
        json!({"extensions":[adapter],"cacheWarming":"off","compaction":{"enabled":false},"retry":{"enabled":false}})
            .to_string(),
    )?;
    // Static selection + session_start native override: never ask a live llama.cpp router.
    let models = ["solo", "orchestrator", "planner", "backend", "verifier", "probe"]
        .map(|role| json!({"id":format!("fixture-{role}"),"reasoning":true,"contextWindow":200_000,"maxTokens":4096}));
    std::fs::write(agent.join("models.json"), json!({"providers":{"llama.cpp":{
        "api":"openai-completions", "apiKey":"not-a-credential", "baseUrl":"http://127.0.0.1:9", "models":models
    }}}).to_string())?;
    std::fs::write(root.join("provider.ts"), include_str!("provider.mjs"))?;
    std::fs::write(root.join("offline.cjs"), include_str!("offline.cjs"))?;
    std::fs::write(
        root.join("awt.mjs"),
        include_str!("../../../ai-team-core/tests/fixtures/chat-worker-awt.mjs"),
    )?;
    std::fs::write(root.join("worker-mode"), "success")?;
    executable(&bin.join("pi"), &format!(
        "#!/bin/sh\nexec {} --offline --no-extensions --no-skills --no-prompt-templates --no-themes --no-approve --extension {} --extension {} \"$@\"\n",
        quote(&pi.to_string_lossy()), quote(&root.join("provider.ts").to_string_lossy()), quote(&adapter.to_string_lossy())
    ))?;
    executable(
        &bin.join("awt"),
        &format!(
            "#!/bin/sh\nexec node {} \"$@\"\n",
            quote(&root.join("awt.mjs").to_string_lossy())
        ),
    )?;
    for command in ["aip", "claude", "codex", "security", "gh"] {
        // Bind the evidence path, rather than relying on a child preserving HOME.
        executable(
            &bin.join(command),
            &format!(
                "#!/bin/sh\necho {command} >> {}\nexit 99\n",
                quote(&root.join("unexpected-tool").to_string_lossy())
            ),
        )?;
    }
    Ok(())
}

fn repository(repo: &Path) -> anyhow::Result<()> {
    git(repo, &["init", "-q"]);
    std::fs::create_dir(repo.join("crates"))?;
    std::fs::write(repo.join("crates/answer.txt"), "BASE\n")?;
    std::fs::write(
        repo.join("package.json"),
        r#"{"scripts":{"test":"node check.mjs"}}"#,
    )?;
    std::fs::write(repo.join("check.mjs"), "import {readFileSync} from 'node:fs'; if(process.env.ANTHROPIC_API_KEY || process.env.OPENAI_API_KEY) throw Error('inherited credentials'); if(readFileSync('crates/answer.txt','utf8')!=='GOOD\\n') process.exit(1);\n")?;
    git(repo, &["add", "."]);
    git(
        repo,
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@invalid",
            "commit",
            "-qm",
            "base",
        ],
    );
    Ok(())
}
fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}
fn executable(path: &Path, source: &str) -> anyhow::Result<()> {
    std::fs::write(path, source)?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}
pub(super) fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().into()
}
