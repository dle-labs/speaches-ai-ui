//! Persistent i3xrocks worker: one process, bounded stdin clicks, busy-only animation.

use crate::{ipc, status::BarSnapshot};
use anyhow::Result;
use serde::Deserialize;
use std::{
    fs::File,
    io::{self, BufRead, Read, Write},
    os::{fd::AsFd, unix::process::CommandExt},
    process::{Command, Stdio},
    sync::mpsc::{self, RecvTimeoutError},
    time::{Duration, Instant},
};

const FRAME_INTERVAL: Duration = Duration::from_millis(125);
const IDLE_INTERVAL: Duration = Duration::from_secs(1);
const CLICK_LIMIT: u64 = 4096;

enum Event {
    Show,
    Closed,
}
#[derive(Deserialize)]
struct Click {
    button: u8,
}

fn is_left_click(line: &str) -> bool {
    serde_json::from_str::<Click>(line).is_ok_and(|click| click.button == 1)
}

fn show_app() {
    if ipc::request("show").is_ok() {
        return;
    }
    // Only an explicit click can launch the app. Normal status polling never does.
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    // The GUI owns its lifetime, not the bar's: a later i3bar process-group
    // restart must not terminate a dictation started from this click.
    if let Ok(mut child) = Command::new(executable)
        .process_group(0)
        .arg("show")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

pub(crate) fn watch() -> Result<()> {
    let (sender, receiver) = mpsc::sync_channel(8);
    std::thread::spawn(move || {
        let input = io::stdin();
        let mut input = input.lock();
        loop {
            let mut line = String::new();
            match (&mut input).take(CLICK_LIMIT).read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) if !line.ends_with('\n') => break,
                Ok(_) => {
                    if is_left_click(&line) && sender.try_send(Event::Show).is_err() {
                        // Click bursts can coalesce; they must not allocate unbounded work.
                        continue;
                    }
                }
            }
        }
        let _ = sender.send(Event::Closed);
    });
    // File is unbuffered. Each complete JSON line (well below PIPE_BUF) is sent
    // together: installed i3xrocks can flicker if text and newline are separate writes.
    let mut output = File::from(io::stdout().as_fd().try_clone_to_owned()?);
    let mut previous = String::new();
    let mut frame: usize = 0;
    loop {
        let snapshot = ipc::request("snapshot")
            .ok()
            .and_then(|text| serde_json::from_str::<BarSnapshot>(&text).ok())
            .unwrap_or_else(BarSnapshot::offline);
        let markup = snapshot.markup(frame);
        if markup != previous {
            let mut line =
                serde_json::to_vec(&serde_json::json!({"full_text": markup, "markup": "pango"}))?;
            line.push(b'\n');
            if let Err(error) = output.write_all(&line) {
                if error.kind() == io::ErrorKind::BrokenPipe {
                    return Ok(());
                }
                return Err(error.into());
            }
            previous = markup;
        }
        frame = frame.wrapping_add(1);
        let interval = if snapshot.is_animating() {
            FRAME_INTERVAL
        } else {
            IDLE_INTERVAL
        };
        let deadline = Instant::now() + interval;
        loop {
            match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(Event::Show) => show_app(),
                Ok(Event::Closed) | Err(RecvTimeoutError::Disconnected) => return Ok(()),
                Err(RecvTimeoutError::Timeout) => break,
            }
            if Instant::now() >= deadline {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn click_protocol_accepts_left_click_and_ignores_extra_properties() {
        assert!(is_left_click(
            r#"{"button":1,"x":123,"":"", "full_text":"ignored"}"#
        ));
        assert!(!is_left_click(r#"{"button":3}"#));
        assert!(!is_left_click("not json"));
        assert!(!is_left_click(r#"{"button":"1"}"#));
    }
}
