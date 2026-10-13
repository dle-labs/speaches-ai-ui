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
    stream.take(1024).read_to_string(&mut reply)?;
    if !matches!(command, "status" | "bar" | "snapshot") && reply.trim() != "OK" {
        bail!("The running application did not accept {command}; try again");
    }
    if reply.trim().is_empty() {
        bail!("Empty IPC response");
    }
    Ok(reply)
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
        let status = Arc::new(Mutex::new(BarSnapshot {
            state: Status::Ready,
            animate: true,
        }));
        let state = status.clone();
        std::thread::spawn(move || {
            for mut stream in listener.incoming().flatten() {
                let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
                let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
                let mut input = String::new();
                if (&mut stream).take(64).read_to_string(&mut input).is_err() {
                    continue;
                }
                let reply = match input.trim() {
                    "status" => state
                        .lock()
                        .map(|s| s.state.text())
                        .unwrap_or_else(|_| Status::Error.text()),
                    "bar" => state
                        .lock()
                        .map(|s| s.state.bar())
                        .unwrap_or_else(|_| Status::Error.bar()),
                    "snapshot" => state
                        .lock()
                        .ok()
                        .and_then(|s| serde_json::to_string(&*s).ok())
                        .unwrap_or_else(|| "null".into()),
                    command @ ("show" | "toggle" | "cancel" | "quit") => {
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
