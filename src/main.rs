mod audio;
mod backend;
mod bar;
mod controller;
mod ipc;
mod settings;
mod status;
mod ui;
mod x11;

use anyhow::Result;
use gpui_kit::*;
use std::{sync::Arc, time::Duration};

fn main() -> Result<()> {
    let command = std::env::args().nth(1).unwrap_or_else(|| "show".into());
    if command == "bar-watch" {
        return bar::watch();
    }
    if matches!(
        command.as_str(),
        "status" | "bar" | "toggle" | "cancel" | "quit"
    ) {
        return ipc::run_client(&command);
    }
    if matches!(command.as_str(), "--help" | "-h") {
        println!(
            "Speaches AI\n\n  speaches-ai [show]   Open or reveal the window\n  speaches-ai status   Print plain-text status (does not launch app)\n  speaches-ai bar      Print static colored icon (Pango)\n  speaches-ai bar-watch Stream animated icons and handle i3xrocks clicks\n  speaches-ai toggle   Start/stop recording in the running app\n  speaches-ai cancel   Cancel and discard temporary recording\n  speaches-ai quit     Exit the running app\n\nGlobal shortcuts: configurable in the window. Ctrl+Alt+Space toggles recording; Ctrl+Shift+X cancels/discards by default. Ctrl+Q hides the window by default."
        );
        return Ok(());
    }
    anyhow::ensure!(command == "show", "Unknown command; use --help");
    match ipc::request("show") {
        Ok(_) => return Ok(()),
        Err(error)
            if error.downcast_ref::<std::io::Error>().is_some_and(|e| {
                matches!(
                    e.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
                )
            }) => {}
        Err(error) => return Err(error),
    }
    let server = ipc::Server::start()?;
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?,
    );
    let runtime_owner = runtime.clone();
    gpui_kit::application()
        .with_quit_mode(QuitMode::Explicit)
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| {
            gpui_kit::init(cx);
            cx.set_app_identity("speaches-ai", "Speaches AI");
            cx.bind_keys([KeyBinding::new("ctrl-q", ui::CloseOrQuit, Some("Speaches"))]);
            let controller = cx.new(|_| controller::Controller::new(runtime));
            ui::show(controller.clone(), cx);
            cx.spawn(async move |cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(50))
                        .await;
                    cx.update(|cx| {
                        while let Ok(command) = server.commands.try_recv() {
                            match command.as_str() {
                                "show" => ui::show(controller.clone(), cx),
                                "toggle" => {
                                    controller.update(cx, |c, cx| c.toggle(true, cx));
                                }
                                "cancel" => {
                                    controller.update(cx, |c, cx| c.cancel(cx));
                                }
                                "quit" => {
                                    controller.update(cx, |c, cx| c.cancel(cx));
                                    cx.quit();
                                }
                                _ => {}
                            }
                        }
                        controller.update(cx, |c, cx| {
                            c.tick(cx);
                            if let Ok(mut status) = server.status.lock() {
                                *status = status::BarSnapshot {
                                    state: c.state(),
                                    animate: c.settings.animate_bar,
                                };
                            }
                        });
                    });
                }
            })
            .detach();
        });
    if let Ok(runtime) = Arc::try_unwrap(runtime_owner) {
        runtime.shutdown_timeout(Duration::from_secs(1));
    }
    Ok(())
}
