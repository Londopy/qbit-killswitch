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

/// Generate the app icon: a green shield with a white padlock.
/// 64×64 RGBA, rendered pixel-by-pixel from analytic shape tests.
fn app_icon() -> egui::IconData {
    const SIZE: u32 = 64;
    let mut rgba = vec![0u8; (SIZE * SIZE * 4) as usize];
    let s = SIZE as f32;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let fx = (x as f32 + 0.5) / s;
            let fy = (y as f32 + 0.5) / s;
            let i  = ((y * SIZE + x) * 4) as usize;
            if !icon_in_shield(fx, fy) {
                // transparent — leave as zeroes
            } else if icon_in_lock(fx, fy) {
                // white padlock glyph
                rgba[i]     = 255;
                rgba[i + 1] = 255;
                rgba[i + 2] = 255;
                rgba[i + 3] = 255;
            } else {
                // green shield body
                rgba[i]     = 28;
                rgba[i + 1] = 158;
                rgba[i + 2] = 70;
                rgba[i + 3] = 255;
            }
        }
    }
    egui::IconData { rgba, width: SIZE, height: SIZE }
}

/// True if the normalised point (x, y) ∈ [0,1]² is inside the shield outline.
/// Flat top, straight sides tapering to a point at the bottom.
fn icon_in_shield(x: f32, y: f32) -> bool {
    const TOP:   f32 = 0.07;  // top edge
    const BOT:   f32 = 0.95;  // tip of point
    const TAPER: f32 = 0.58;  // y where sides start converging
    const HALF:  f32 = 0.40;  // half-width of the straight section
    const CX:    f32 = 0.50;

    if y < TOP || y > BOT { return false; }
    let hw = if y <= TAPER {
        HALF
    } else {
        HALF * (BOT - y) / (BOT - TAPER)
    };
    (x - CX).abs() < hw
}

/// True if the normalised point (x, y) falls on the padlock glyph.
/// Consists of a hollow shackle (upper semicircle) and a rectangular body.
fn icon_in_lock(x: f32, y: f32) -> bool {
    const CX: f32 = 0.50;
    // Shackle: hollow semicircle arc (ring) above the body
    let dx = x - CX;
    let dy = y - 0.43;
    let r  = (dx * dx + dy * dy).sqrt();
    if r >= 0.09 && r <= 0.155 && y <= 0.43 {
        return true;
    }
    // Body: filled rectangle
    x >= 0.315 && x <= 0.685 && y >= 0.44 && y <= 0.72
}

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
            .with_resizable(true)
            .with_icon(Arc::new(app_icon())),
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
