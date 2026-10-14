use crate::status::{BarSnapshot, Status};
use anyhow::{Context, Result, bail};
use fs2::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

pub(crate) struct Server {
    _lock: File,
    path: PathBuf,
    pub(crate) commands: async_channel::Receiver<String>,
    pub(crate) status: Arc<Mutex<BarSnapshot>>,
    /// Selected next-request preferences only; never credentials or transcripts.
    pub(crate) selection: Arc<Mutex<serde_json::Value>>,
}

fn directory() -> Result<PathBuf> {
    let dir = PathBuf::from(
        std::env::var_os("XDG_RUNTIME_DIR").context("XDG_RUNTIME_DIR is unavailable")?,
    )
    .join("speaches-ai");
    fs::create_dir_all(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    Ok(dir)
}

pub(crate) fn request(command: &str) -> Result<String> {
    let mut stream = UnixStream::connect(directory()?.join("app.sock"))?;
    stream.set_read_timeout(Some(Duration::from_secs(1)))?;
    stream.set_write_timeout(Some(Duration::from_secs(1)))?;
    stream.write_all(command.as_bytes())?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut reply = String::new();
    stream.take(8192).read_to_string(&mut reply)?;
    if !matches!(command, "status" | "bar" | "snapshot" | "selection") && reply.trim() != "OK" {
        bail!("The running application did not accept {command}; try again");
    }
    if reply.trim().is_empty() {
        bail!("Empty IPC response");
    }
    Ok(reply)
}

fn is_control_command(command: &str) -> bool {
    matches!(
        command,
        "show" | "preferences" | "language-next" | "toggle" | "cancel" | "quit"
    ) || command
        .strip_prefix("language ")
        .is_some_and(|value| crate::language::normalize(value).is_ok())
        || command.strip_prefix("model ").is_some_and(|value| {
            !value.is_empty() && value.len() <= 512 && !value.chars().any(char::is_control)
        })
}

impl Server {
    pub(crate) fn start() -> Result<Self> {
        let dir = directory()?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .open(dir.join("app.lock"))?;
        lock.try_lock_exclusive()
            .context("Speaches AI is already starting or running")?;
        let path = dir.join("app.sock");
        if path.exists() {
            fs::remove_file(&path)?;
        }
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        let (tx, commands) = async_channel::bounded(32);
        let status = Arc::new(Mutex::new(BarSnapshot::with_languages(
            Status::Ready,
            true,
            "",
            None,
        )));
        let selection = Arc::new(Mutex::new(serde_json::json!({})));
        let preferences = selection.clone();
        let state = status.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
                let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
                let mut input = String::new();
                if (&mut stream).take(1025).read_to_string(&mut input).is_err()
                    || input.len() > 1024
                {
                    let _ = writeln!(stream, "Invalid command");
                    continue;
                }
                let reply = match input.trim() {
                    "status" => state
                        .lock()
                        .map(|s| s.state.text())
                        .unwrap_or_else(|_| Status::Error.text()),
                    "bar" => state
                        .lock()
                        .map(|s| {
                            let mut snapshot = s.clone();
                            snapshot.animate = false;
                            snapshot.markup(0)
                        })
                        .unwrap_or_else(|_| Status::Error.bar()),
                    "snapshot" => state
                        .lock()
                        .ok()
                        .and_then(|s| serde_json::to_string(&*s).ok())
                        .unwrap_or_else(|| "null".into()),
                    "selection" => preferences
                        .lock()
                        .map(|value| value.to_string())
                        .unwrap_or_else(|_| "null".into()),
                    command if is_control_command(command) => {
                        if tx.try_send(command.to_owned()).is_ok() {
                            "OK".into()
                        } else {
                            "BUSY".into()
                        }
                    }
                    _ => "Unknown command".into(),
                };
                let _ = writeln!(stream, "{reply}");
            }
        });
        Ok(Self {
            _lock: lock,
            path,
            commands,
            status,
            selection,
        })
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub(crate) fn run_client(command: &str) -> Result<()> {
    match request(command) {
        Ok(reply) => print!("{reply}"),
        Err(_) if command == "status" => println!("{}", Status::Offline.text()),
        Err(_) if command == "bar" => println!("{}", Status::Offline.bar()),
        Err(err) => bail!("Speaches AI is not running: {err}"),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preference_commands_are_bounded_and_validated() {
        assert!(is_control_command("preferences"));
        assert!(is_control_command("language-next"));
        assert!(is_control_command("language de"));
        assert!(is_control_command("language auto"));
        assert!(is_control_command("model repo/model"));
        assert!(!is_control_command("language <b>en</b>"));
        assert!(!is_control_command("model model\nquit"));
        assert!(!is_control_command(&format!("model {}", "x".repeat(513))));
    }
}
