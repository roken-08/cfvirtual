use anyhow::{Context, Result};
use serde_json::Value;
use std::process::{Command, Output};

fn run(directory: &std::path::Path, args: &[&str]) -> Result<Output> {
    Ok(Command::new(env!("CARGO_BIN_EXE_cfvirtual"))
        .args(args)
        .current_dir(directory)
        .env("XDG_CONFIG_HOME", directory.join("config"))
        .env("XDG_DATA_HOME", directory.join("data"))
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("ALL_PROXY", "http://127.0.0.1:9")
        .env("NO_PROXY", "")
        .output()?)
}

fn parsed(output: Output) -> Result<Value> {
    anyhow::ensure!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).context("stdout must contain exactly one JSON value")
}

#[test]
fn all_commands_have_help_and_usage_failures_exit_two() -> Result<()> {
    let directory = tempfile::tempdir()?;
    for args in [
        vec!["--help"],
        vec!["start", "--help"],
        vec!["room", "create", "--help"],
        vec!["join", "--help"],
        vec!["serve", "--help"],
        vec!["history", "--help"],
        vec!["review", "--help"],
        vec!["stats", "--help"],
        vec!["config", "--help"],
        vec!["demo", "--help"],
    ] {
        assert!(run(directory.path(), &args)?.status.success());
    }
    for args in [
        vec!["start", "--problems", "0"],
        vec!["config", "set", "unknown", "value"],
        vec!["start", "--json"],
        vec!["start", "--json", "--handle", "bad/handle"],
    ] {
        assert_eq!(run(directory.path(), &args)?.status.code(), Some(2));
    }
    Ok(())
}

#[test]
fn offline_demo_cli_history_export_config_and_json_survive_restart() -> Result<()> {
    let directory = tempfile::tempdir()?;
    assert_eq!(
        parsed(run(directory.path(), &["history", "--json"])?)?,
        serde_json::json!([])
    );
    let demo = parsed(run(directory.path(), &["demo", "--seed", "42", "--json"])?)?;
    assert_eq!(demo["contest"]["mode"], "demo");
    assert_eq!(
        demo["contest"]["problems"]
            .as_array()
            .context("problems")?
            .len(),
        6
    );
    assert_eq!(demo["contest"]["bots"].as_array().context("bots")?.len(), 3);
    assert!(demo["standings"]
        .as_array()
        .context("standings")?
        .iter()
        .any(|s| s["name"] == "demo_player" && s["solved"] == 3));
    let history = parsed(run(directory.path(), &["history", "--json"])?)?;
    let id = history[0]["id"].as_str().context("contest id")?;
    let exported = parsed(run(
        directory.path(),
        &["history", "--export", id, "--json"],
    )?)?;
    let path = exported["path"].as_str().context("export path")?;
    let markdown = std::fs::read_to_string(directory.path().join(path))?;
    assert!(markdown.contains("# CFVirtual review"));
    assert!(markdown.contains("(bot)"));
    assert!(markdown.contains("https://codeforces.com/problemset/problem/"));
    assert!(
        !run(directory.path(), &["history", "--export", id])?
            .status
            .success(),
        "export must not overwrite a file"
    );
    let stats = parsed(run(directory.path(), &["stats", "--json"])?)?;
    assert_eq!(stats["total_contests"], 1);
    assert_eq!(stats["total_solved"], 3);
    assert_eq!(
        parsed(run(directory.path(), &["review", "--due", "--json"])?)?,
        serde_json::json!([])
    );
    parsed(run(
        directory.path(),
        &["config", "set", "handle", "practice_player", "--json"],
    )?)?;
    assert_eq!(
        parsed(run(
            directory.path(),
            &["config", "get", "handle", "--json"]
        )?)?,
        "practice_player"
    );
    assert_eq!(
        run(directory.path(), &["config", "set", "default_bots", "100"])?
            .status
            .code(),
        Some(2)
    );
    Ok(())
}

#[test]
fn unavailable_codeforces_returns_exit_three_without_polluting_json_stdout() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let output = run(
        directory.path(),
        &["start", "--handle", "practice_player", "--json"],
    )?;
    assert_eq!(
        output.status.code(),
        Some(3),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    Ok(())
}
