use anyhow::{bail, Context, Result};
use std::io::Write;
use std::process::{Command, Stdio};

/// Thin wrapper over the system `ssh` binary with connection multiplexing.
///
/// We deliberately shell out instead of linking an SSH library: the user's
/// ~/.ssh/config (aliases, keys, jump hosts, agent) works unchanged, and
/// ControlMaster gives us "maintain the connection" for free — the first
/// command pays the handshake, everything after multiplexes over the socket.
pub struct Ssh {
    target: String,
    control_path: String,
}

impl Ssh {
    pub fn new(target: String) -> Result<Self> {
        let dir = crate::config::config_home().join("sockets");
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("creating {}", dir.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        // %C = hash of local host, remote host, port, and user — short enough
        // to stay under the unix socket path limit.
        let control_path = dir.join("%C").to_string_lossy().into_owned();
        Ok(Self {
            target,
            control_path,
        })
    }

    fn option_args(&self) -> Vec<String> {
        vec![
            "-o".into(),
            "ControlMaster=auto".into(),
            "-o".into(),
            format!("ControlPath={}", self.control_path),
            "-o".into(),
            "ControlPersist=10m".into(),
            "-o".into(),
            "ServerAliveInterval=30".into(),
            // Firewalls that drop (not reject) port 22 otherwise leave ssh
            // waiting on the OS TCP timeout — the CLI looks frozen.
            "-o".into(),
            "ConnectTimeout=10".into(),
        ]
    }

    /// Turn a finished ssh invocation into a Result. ssh reserves exit 255
    /// for its own failures (unreachable, timed out, auth) — everything else
    /// is the remote command's exit code.
    fn ensure_success(&self, status: std::process::ExitStatus, stderr: Option<&str>) -> Result<()> {
        if status.success() {
            return Ok(());
        }
        let detail = stderr.map(str::trim).filter(|s| !s.is_empty());
        if status.code() == Some(255) {
            bail!(
                "could not reach {target} over ssh{sep}{detail} — the boxes gate ssh by source IP; \
                 check you're on an allowed network/VPN, then verify with: ssh {target}",
                target = self.target,
                sep = if detail.is_some() { ": " } else { "" },
                detail = detail.unwrap_or(""),
            );
        }
        match detail {
            Some(d) => bail!("remote command failed (exit {:?}): {}", status.code(), d),
            None => bail!("remote command failed (exit {:?})", status.code()),
        }
    }

    fn command(&self, tty: bool) -> Command {
        let mut cmd = Command::new("ssh");
        cmd.args(self.option_args());
        if tty {
            cmd.arg("-t");
        }
        cmd.arg(&self.target);
        cmd
    }

    /// Run a remote command, streaming its output to the terminal.
    pub fn run(&self, remote: &str) -> Result<()> {
        let status = self
            .command(false)
            .arg(remote)
            .status()
            .context("failed to spawn ssh — is it on PATH?")?;
        self.ensure_success(status, None)
    }

    /// Run with a TTY for interactive/follow commands. Ctrl-C is forwarded to
    /// the remote process, so a non-zero exit here is normal — not an error.
    pub fn run_interactive(&self, remote: &str) -> Result<()> {
        self.command(true)
            .arg(remote)
            .status()
            .context("failed to spawn ssh — is it on PATH?")?;
        Ok(())
    }

    /// Run a remote command and capture trimmed stdout.
    pub fn capture(&self, remote: &str) -> Result<String> {
        let out = self
            .command(false)
            .arg(remote)
            .output()
            .context("failed to spawn ssh — is it on PATH?")?;
        self.ensure_success(out.status, Some(&String::from_utf8_lossy(&out.stderr)))?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    /// Run a remote command with `input` piped to its stdin (used to write files).
    pub fn run_with_stdin(&self, remote: &str, input: &str) -> Result<()> {
        let mut child = self
            .command(false)
            .arg(remote)
            .stdin(Stdio::piped())
            .spawn()
            .context("failed to spawn ssh — is it on PATH?")?;
        child
            .stdin
            .as_mut()
            .expect("stdin was piped")
            .write_all(input.as_bytes())?;
        drop(child.stdin.take());
        let status = child.wait()?;
        self.ensure_success(status, None)
    }

    /// Value for GIT_SSH_COMMAND so `git push` shares our control socket.
    pub fn git_ssh_command(&self) -> String {
        format!(
            "ssh -o ControlMaster=auto -o ControlPath={} -o ControlPersist=10m -o ServerAliveInterval=30 -o ConnectTimeout=10",
            self.control_path
        )
    }
}
