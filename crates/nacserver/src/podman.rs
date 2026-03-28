use std::path::Path;
use anyhow::{anyhow, Result};
use tokio::process::Command;

pub async fn check_available() -> Result<()> {
    let output = Command::new("podman").arg("--version").output().await
        .map_err(|_| anyhow!("podman not found — install podman to use nacserver"))?;
    if !output.status.success() {
        return Err(anyhow!("podman not working: {}", String::from_utf8_lossy(&output.stderr)));
    }
    Ok(())
}

pub async fn run_container(name: &str, image: &str, workspace: &Path, env_vars: &[(&str, &str)]) -> Result<()> {
    let mut args = vec![
        "run".to_string(), "-d".to_string(),
        "--name".to_string(), name.to_string(),
        "-v".to_string(), format!("{}:/workspace", workspace.display()),
        "-w".to_string(), "/workspace".to_string(),
    ];
    for (k, v) in env_vars {
        args.push("-e".to_string());
        args.push(format!("{}={}", k, v));
    }
    args.push(image.to_string());
    args.push("sleep".to_string());
    args.push("infinity".to_string());

    let output = Command::new("podman")
        .args(&args)
        .output()
        .await?;

    if !output.status.success() {
        return Err(anyhow!("podman run failed: {}", String::from_utf8_lossy(&output.stderr)));
    }
    Ok(())
}

pub struct ExecResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

pub async fn exec_in_container(name: &str, prompt: &str) -> Result<ExecResult> {
    let output = Command::new("podman")
        .args(["exec", name, "nac", "--single", prompt])
        .output()
        .await?;

    Ok(ExecResult {
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        exit_code: output.status.code().unwrap_or(-1),
    })
}

pub async fn remove_container(name: &str) -> Result<()> {
    let _ = Command::new("podman").args(["rm", "-f", name]).output().await?;
    Ok(())
}
