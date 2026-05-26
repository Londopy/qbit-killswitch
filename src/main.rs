// On Windows release builds, suppress the console window.
#![cfg_attr(all(not(debug_assertions), target_os = "windows"), windows_subsystem = "windows")]

use std::sync::{Arc, Mutex};

mod app;
mod config;
mod ip;
mod monitor;
mod qbit;
mod state;

use config::Config;
use state::SharedState;

fn main() -> anyhow::Result<()> {
    // ── Tokio runtime (multi-thread, for monitor + async GUI ops) ─────────────
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?,
    );

    // ── Config ────────────────────────────────────────────────────────────────
    let (mut config, load_warning) = Config::load();
    config.normalize();

    // ── Resolve password (keychain or plaintext) ──────────────────────────────
    let (password, keychain_warning) = config::resolve_password(&config);

    // ── Shared state ──────────────────────────────────────────────────────────
    let shared = Arc::new(Mutex::new(SharedState {
        keychain_warning,
        ..Default::default()
    }));

    // ── Command channel (GUI → monitor) ───────────────────────────────────────
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::unbounded_channel();

    // ── Spawn monitor task ────────────────────────────────────────────────────
    {
        let shared   = Arc::clone(&shared);
        let config   = config.clone();
        let password = password.clone();
        runtime.spawn(async move {
            let task = monitor::MonitorTask::new(shared, cmd_rx, config, password);
            task.run().await;
        });
    }

    // ── eframe native window ──────────────────────────────────────────────────
    let native_options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("qbit-killswitch")
            .with_inner_size([500.0, 600.0])
            .with_min_inner_size([400.0, 400.0])
            .with_resizable(true),
        ..Default::default()
    };

    let shared_for_app  = Arc::clone(&shared);
    let runtime_for_app = Arc::clone(&runtime);

    eframe::run_native(
        "qbit-killswitch",
        native_options,
        Box::new(move |cc| {
            Box::new(app::KillswitchApp::new(
                cc,
                config,
                password,
                shared_for_app,
                cmd_tx,
                runtime_for_app,
                load_warning,
            ))
        }),
    )
    .map_err(|e| anyhow::anyhow!("eframe error: {e}"))?;

    Ok(())
}
