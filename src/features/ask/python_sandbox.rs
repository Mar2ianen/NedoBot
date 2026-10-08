use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use anyhow::Context;
use genai::chat::Tool;
use serde_json::{Value, json};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, Command};
use tokio::time::timeout;

use crate::config::Config;
use crate::features::ask::mcp_client::wire_tool_name;

const TOOL_NAME: &str = "sandbox.python";
const MAX_CODE_BYTES: usize = 16 * 1024;
const MAX_OUTPUT_BYTES_PER_STREAM: usize = 8 * 1024;
const MAX_EXECUTIONS_PER_ASK: usize = 4;
const VERIFY_TIMEOUT: Duration = Duration::from_secs(5);
const EXECUTION_TIMEOUT: Duration = Duration::from_secs(15);

pub struct AskPythonSandbox {
    image_id: String,
    podman: PathBuf,
    executions: usize,
}

impl AskPythonSandbox {
    pub async fn validate_runtime(config: &Config) -> anyhow::Result<()> {
        if !config.ask_python_sandbox_enabled {
            return Ok(());
        }
        let image_id = config
            .ask_python_sandbox_image
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Python sandbox image is not configured"))?;
        Self::new(image_id)?.verify_runtime().await
    }

    pub fn new(image_id: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(
            is_pinned_image_id(image_id),
            "invalid pinned sandbox image id"
        );
        Ok(Self {
            image_id: image_id.to_owned(),
            podman: find_podman_binary()?,
            executions: 0,
        })
    }

    pub fn tool_definition(&self) -> Tool {
        Tool::new(wire_tool_name(TOOL_NAME))
            .with_description(
                "Вычисления и временные файлы в одноразовой Python-песочнице. Каждый запуск изолирован и имеет пустой /workspace объёмом до 32 MiB; файлы удаляются после вызова. Сети, секретов и доступа к файлам хоста нет. Между вызовами состояние не сохраняется. Используй для расчётов по данным запроса; печатай только нужный результат.",
            )
            .with_schema(json!({
                "type": "object",
                "additionalProperties": false,
                "required": ["code"],
                "properties": {
                    "code": {"type": "string", "maxLength": MAX_CODE_BYTES}
                }
            }))
            .with_strict(true)
    }

    pub async fn verify_runtime(&self) -> anyhow::Result<()> {
        let rootless = self
            .podman_output(["info", "--format", "{{.Host.Security.Rootless}}"])
            .await?;
        anyhow::ensure!(
            rootless.status.success() && String::from_utf8_lossy(&rootless.stdout).trim() == "true",
            "Python sandbox requires rootless Podman"
        );
        let image = self
            .podman_output(["image", "exists", self.image_id.as_str()])
            .await?;
        anyhow::ensure!(
            image.status.success(),
            "pinned Python sandbox image is not present locally"
        );
        let mut probe = Self::new(&self.image_id)?;
        let result = probe.execute("pass").await?;
        anyhow::ensure!(
            result["exit_code"] == 0,
            "pinned sandbox image does not run Python successfully"
        );
        Ok(())
    }

    pub async fn execute(&mut self, code: &str) -> anyhow::Result<Value> {
        anyhow::ensure!(
            !code.trim().is_empty() && code.len() <= MAX_CODE_BYTES,
            "Python code must be non-empty and at most {MAX_CODE_BYTES} bytes"
        );
        anyhow::ensure!(
            self.executions < MAX_EXECUTIONS_PER_ASK,
            "Python sandbox execution limit reached for this /ask"
        );
        self.executions += 1;

        let mut command = self.podman_command();
        command
            .args(container_args(&self.image_id))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().context("start isolated Python sandbox")?;
        let output = timeout(EXECUTION_TIMEOUT, run_child(&mut child, code))
            .await
            .map_err(|_| anyhow::anyhow!("Python sandbox execution timed out"))??;

        Ok(json!({
            "exit_code": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
            "output_truncated": output.stdout_truncated || output.stderr_truncated,
        }))
    }

    async fn podman_output<const N: usize>(
        &self,
        args: [&str; N],
    ) -> anyhow::Result<std::process::Output> {
        let output = timeout(VERIFY_TIMEOUT, self.podman_command().args(args).output())
            .await
            .map_err(|_| anyhow::anyhow!("Podman sandbox preflight timed out"))??;
        Ok(output)
    }

    fn podman_command(&self) -> Command {
        let mut command = Command::new(&self.podman);
        command.env_clear().env("PATH", "/usr/bin:/bin");
        for name in ["HOME", "XDG_RUNTIME_DIR"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
    }
}

fn find_podman_binary() -> anyhow::Result<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .map(|directory| directory.join("podman"))
        .find(|candidate| candidate.is_file())
        .ok_or_else(|| anyhow::anyhow!("Podman executable is not available"))
}

fn is_pinned_image_id(value: &str) -> bool {
    let Some(digest) = value.strip_prefix("sha256:") else {
        return false;
    };
    digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn container_args(image_id: &str) -> Vec<OsString> {
    [
        "run",
        "--rm",
        "--pull=never",
        "--interactive",
        "--network=none",
        "--read-only",
        "--cap-drop=ALL",
        "--security-opt=no-new-privileges",
        "--userns=keep-id:uid=1000,gid=1000",
        "--user=1000:1000",
        "--pids-limit=32",
        "--memory=256m",
        "--cpus=1",
        "--ulimit=nofile=64:64",
        "--ulimit=fsize=4194304:4194304",
        "--shm-size=8m",
        "--tmpfs=/tmp:rw,noexec,nosuid,nodev,size=16m",
        "--tmpfs=/workspace:rw,noexec,nosuid,nodev,size=32m,mode=0777",
        "--workdir=/workspace",
        "--http-proxy=false",
    ]
    .into_iter()
    .map(OsString::from)
    .chain([
        OsString::from(image_id),
        OsString::from("python3"),
        OsString::from("-I"),
        OsString::from("-B"),
        OsString::from("-S"),
        OsString::from("-"),
    ])
    .collect()
}

struct CapturedOutput {
    status: std::process::ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    stdout_truncated: bool,
    stderr_truncated: bool,
}

async fn run_child(child: &mut Child, code: &str) -> anyhow::Result<CapturedOutput> {
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("sandbox stdin was not piped"))?;
    stdin.write_all(code.as_bytes()).await?;
    drop(stdin);
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow::anyhow!("sandbox stdout was not piped"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("sandbox stderr was not piped"))?;

    let (stdout, stderr, status) = tokio::join!(
        read_capped(&mut stdout, MAX_OUTPUT_BYTES_PER_STREAM),
        read_capped(&mut stderr, MAX_OUTPUT_BYTES_PER_STREAM),
        child.wait(),
    );
    let (stdout, stdout_truncated) = stdout?;
    let (stderr, stderr_truncated) = stderr?;
    Ok(CapturedOutput {
        status: status?,
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
    })
}

async fn read_capped(
    reader: &mut (impl AsyncRead + Unpin),
    limit: usize,
) -> anyhow::Result<(Vec<u8>, bool)> {
    let mut captured = Vec::with_capacity(limit);
    let mut chunk = [0u8; 4096];
    let mut truncated = false;
    loop {
        let bytes_read = reader.read(&mut chunk).await?;
        if bytes_read == 0 {
            break;
        }
        let remaining = limit.saturating_sub(captured.len());
        let captured_bytes = remaining.min(bytes_read);
        captured.extend_from_slice(&chunk[..captured_bytes]);
        truncated |= captured_bytes < bytes_read;
    }
    Ok((captured, truncated))
}

#[cfg(test)]
mod tests {
    use super::{container_args, is_pinned_image_id, read_capped};

    #[test]
    fn sandbox_requires_an_immutable_local_image_id() {
        assert!(is_pinned_image_id(&format!("sha256:{}", "a".repeat(64))));
        assert!(!is_pinned_image_id("python:3.13-slim"));
        assert!(!is_pinned_image_id(&format!("sha256:{}", "g".repeat(64))));
    }

    #[test]
    fn container_is_rootless_networkless_and_uses_bounded_tmpfs_only() {
        let args = container_args(&format!("sha256:{}", "a".repeat(64)))
            .into_iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        for required in [
            "--pull=never",
            "--network=none",
            "--read-only",
            "--cap-drop=ALL",
            "--security-opt=no-new-privileges",
            "--userns=keep-id:uid=1000,gid=1000",
            "--user=1000:1000",
            "--tmpfs=/workspace:rw,noexec,nosuid,nodev,size=32m,mode=0777",
            "--pids-limit=32",
            "--memory=256m",
            "--cpus=1",
            "--http-proxy=false",
        ] {
            assert!(args.iter().any(|argument| argument == required));
        }
        assert!(
            !args
                .iter()
                .any(|argument| argument.starts_with("--volume="))
        );
    }

    #[tokio::test]
    async fn output_capture_caps_memory_and_drains_the_remaining_pipe() {
        let (mut writer, mut reader) = tokio::io::duplex(8);
        let writer_task = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            writer.write_all(b"0123456789abcdef").await.unwrap();
        });

        let (captured, truncated) = read_capped(&mut reader, 6).await.unwrap();
        writer_task.await.unwrap();

        assert_eq!(captured, b"012345");
        assert!(truncated);
    }
}
