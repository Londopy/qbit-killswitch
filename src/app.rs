use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::{Color32, RichText, Ui, ViewportCommand};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    TrayIcon, TrayIconBuilder, TrayIconEvent,
};

use crate::config::{self, Config};
use crate::qbit::test_connection;
use crate::state::{AppState, CommandSender, MonitorCommand, SharedStateHandle};

// ── Menu item IDs ─────────────────────────────────────────────────────────────

const ID_OPEN:     &str = "open";
const ID_RESUME:   &str = "resume";
const ID_PAUSE:    &str = "pause";
const ID_REDETECT: &str = "redetect";
const ID_QUIT:     &str = "quit";

// ── Tray icon generation ──────────────────────────────────────────────────────

fn circle_icon(r: u8, g: u8, b: u8) -> tray_icon::Icon {
    const SIZE: u32 = 16;
    let total = (SIZE * SIZE * 4) as usize;
    let mut rgba = vec![0u8; total];
    let center = SIZE as f32 / 2.0;
    let outer  = center - 0.5;
    let inner  = outer  - 1.0; // anti-alias band

    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 - center + 0.5;
            let dy = y as f32 - center + 0.5;
            let dist = (dx * dx + dy * dy).sqrt();
            let alpha = if dist <= inner {
                255u8
            } else if dist <= outer {
                ((outer - dist) / (outer - inner) * 255.0) as u8
            } else {
                0u8
            };
            let i = ((y * SIZE + x) * 4) as usize;
            rgba[i]     = r;
            rgba[i + 1] = g;
            rgba[i + 2] = b;
            rgba[i + 3] = alpha;
        }
    }
    tray_icon::Icon::from_rgba(rgba, SIZE, SIZE).expect("valid icon")
}

fn state_icon(state: AppState) -> tray_icon::Icon {
    let (r, g, b) = state.rgb();
    circle_icon(r, g, b)
}

fn state_color(state: AppState) -> Color32 {
    let (r, g, b) = state.rgb();
    Color32::from_rgb(r, g, b)
}

/// Paint a heraldic shield (flat top, sides tapering to a bottom point) inside `rect`.
fn paint_shield(painter: &egui::Painter, rect: egui::Rect, fill: Color32) {
    use egui::{pos2, Shape, Stroke};
    let (x0, x1) = (rect.left(), rect.right());
    let (y0, y1) = (rect.top(), rect.bottom());
    let cx       = (x0 + x1) / 2.0;
    let taper_y  = y0 + (y1 - y0) * 0.58;
    let pts = vec![
        pos2(x0, y0),
        pos2(x1, y0),
        pos2(x1, taper_y),
        pos2(cx, y1),
        pos2(x0, taper_y),
    ];
    painter.add(Shape::convex_polygon(pts, fill, Stroke::NONE));
}

// ── Form state (mirrors Config; holds unsaved edits) ─────────────────────────

#[derive(Clone)]
struct FormState {
    config:       Config,
    /// The actual password shown/edited in the Pass field. May come from keychain.
    password:     String,
    /// Original username, to detect changes for keychain re-keying.
    original_user: String,
}

impl FormState {
    fn from(config: &Config, password: &str) -> Self {
        Self {
            original_user: config.qbit_user.clone(),
            password: password.to_string(),
            config: config.clone(),
        }
    }

    fn has_changes(&self, saved: &FormState) -> bool {
        self.password != saved.password
            || self.config.qbit_url          != saved.config.qbit_url
            || self.config.qbit_user         != saved.config.qbit_user
            || self.config.use_keychain      != saved.config.use_keychain
            || self.config.ip_check_urls     != saved.config.ip_check_urls
            || self.config.poll_secs         != saved.config.poll_secs
            || self.config.fail_threshold    != saved.config.fail_threshold
            || self.config.launch_at_startup != saved.config.launch_at_startup
            || self.config.minimize_to_tray  != saved.config.minimize_to_tray
    }
}

// ── Tab selection ─────────────────────────────────────────────────────────────

#[derive(PartialEq)]
enum AppTab {
    Settings,
    Help,
}

// ── KillswitchApp ─────────────────────────────────────────────────────────────

pub struct KillswitchApp {
    // Shared with monitor
    shared:  SharedStateHandle,
    cmd_tx:  CommandSender,
    runtime: Arc<tokio::runtime::Runtime>,

    // Form state
    saved:      FormState,
    editing:    FormState,
    save_errors: Vec<String>,

    // UI flags
    force_quit:          bool,
    show_unsaved_dialog: bool,
    /// Whether to actually close (true) or minimize-to-tray (false) after confirming.
    quit_after_dialog:   bool,
    show_password:       bool,
    /// When the password was last revealed; auto-hides after 60 s.
    show_password_since: Option<Instant>,
    active_tab:          AppTab,
    blink_start:         Instant,

    // Test connection
    test_conn_pending:       Option<tokio::task::JoinHandle<Result<(), String>>>,
    test_conn_result:        Option<Result<(), String>>,
    /// Password snapshot taken when a test is launched; restored if the field empties.
    test_conn_pass_snapshot: String,

    // Tray
    _tray_icon:  TrayIcon,
    resume_item: MenuItem,
    _pause_item: MenuItem,
    status_item: MenuItem,
    ip_item:     MenuItem,

    // Load-time error (shown once)
    load_warning: Option<String>,
}

impl KillswitchApp {
    pub fn new(
        cc:           &eframe::CreationContext<'_>,
        config:       Config,
        password:     String,
        shared:       SharedStateHandle,
        cmd_tx:       CommandSender,
        runtime:      Arc<tokio::runtime::Runtime>,
        load_warning: Option<String>,
    ) -> Self {
        // Register egui context in shared state so monitor can trigger repaints
        shared.lock().unwrap().egui_ctx = Some(cc.egui_ctx.clone());

        // ── Build tray menu ───────────────────────────────────────────────────
        let open_item    = MenuItem::with_id(ID_OPEN,     "Open qbit-killswitch",   true,  None);
        let status_item  = MenuItem::with_id("status",    "Status: STARTING",       false, None);
        let ip_item      = MenuItem::with_id("vpnip",     "VPN IP: —",              false, None);
        let resume_item  = MenuItem::with_id(ID_RESUME,   "Resume all torrents",    false, None);
        let pause_item   = MenuItem::with_id(ID_PAUSE,    "Pause all torrents",     true,  None);
        let redetect_item= MenuItem::with_id(ID_REDETECT, "Re-detect VPN IP",       true,  None);
        let quit_item    = MenuItem::with_id(ID_QUIT,     "Quit",                   true,  None);

        let menu = Menu::new();
        let _ = menu.append_items(&[
            &open_item,
            &PredefinedMenuItem::separator(),
            &status_item,
            &ip_item,
            &PredefinedMenuItem::separator(),
            &resume_item,
            &pause_item,
            &redetect_item,
            &PredefinedMenuItem::separator(),
            &quit_item,
        ]);

        // ── Build tray icon ───────────────────────────────────────────────────
        let tray_icon = TrayIconBuilder::new()
            .with_tooltip("qbit-killswitch — STARTING")
            .with_icon(state_icon(AppState::Starting))
            .with_menu(Box::new(menu))
            .build()
            .expect("Failed to create tray icon");

        let form = FormState::from(&config, &password);

        Self {
            shared,
            cmd_tx,
            runtime,
            saved: form.clone(),
            editing: form,
            save_errors: Vec::new(),
            force_quit: false,
            show_unsaved_dialog: false,
            quit_after_dialog: false,
            show_password: false,
            show_password_since: None,
            active_tab: AppTab::Settings,
            blink_start: Instant::now(),
            test_conn_pending: None,
            test_conn_result: None,
            test_conn_pass_snapshot: String::new(),
            _tray_icon: tray_icon,
            resume_item,
            _pause_item: pause_item,
            status_item,
            ip_item,
            load_warning,
        }
    }

    // ── Tray event polling ────────────────────────────────────────────────────

    fn poll_tray_events(&mut self, ctx: &egui::Context) {
        // Tray icon left-click → show window
        while let Ok(event) = TrayIconEvent::receiver().try_recv() {
            if matches!(event, TrayIconEvent::Click { .. }) {
                ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                ctx.send_viewport_cmd(ViewportCommand::Focus);
            }
        }

        // Menu item clicks
        while let Ok(event) = MenuEvent::receiver().try_recv() {
            match event.id.0.as_str() {
                ID_OPEN => {
                    ctx.send_viewport_cmd(ViewportCommand::Visible(true));
                    ctx.send_viewport_cmd(ViewportCommand::Focus);
                }
                ID_QUIT => {
                    self.force_quit = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
                ID_RESUME => {
                    let _ = self.cmd_tx.send(MonitorCommand::ManualResume);
                }
                ID_PAUSE => {
                    let _ = self.cmd_tx.send(MonitorCommand::ManualPause);
                }
                ID_REDETECT => {
                    let _ = self.cmd_tx.send(MonitorCommand::RedetectVpnIp);
                }
                _ => {}
            }
        }
    }

    // ── Tray state update (called each frame) ─────────────────────────────────

    fn update_tray(&mut self, state: AppState, vpn_ip: Option<std::net::IpAddr>, blink_phase: bool) {
        // Icon: blink between cyan and grey in RECOVERY
        let icon = if state == AppState::Recovery && !blink_phase {
            circle_icon(100, 100, 100)
        } else {
            state_icon(state)
        };
        let _ = self._tray_icon.set_icon(Some(icon));

        let tooltip = format!("qbit-killswitch — {}", state.label());
        let _ = self._tray_icon.set_tooltip(Some(&tooltip));

        self.status_item.set_text(format!("Status: {}", state.label()));
        let ip_str = vpn_ip.map_or("—".to_string(), |ip| ip.to_string());
        self.ip_item.set_text(format!("VPN IP: {ip_str}"));

        // Enable Resume only when paused; grey it out otherwise
        self.resume_item.set_enabled(
            matches!(state, AppState::Paused | AppState::Degraded)
        );
    }

    // ── Settings form ─────────────────────────────────────────────────────────

    fn render_settings(&mut self, ui: &mut Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(4.0);

            // ── qBittorrent ──────────────────────────────────────────────────
            ui.group(|ui| {
                ui.strong("qBittorrent");
                ui.add_space(4.0);

                egui::Grid::new("qbit_grid")
                    .num_columns(2)
                    .spacing([8.0, 6.0])
                    .show(ui, |ui| {
                        ui.label("URL");
                        ui.text_edit_singleline(&mut self.editing.config.qbit_url);
                        ui.end_row();

                        ui.label("User");
                        ui.text_edit_singleline(&mut self.editing.config.qbit_user);
                        ui.end_row();

                        ui.label("Password");
                        ui.horizontal(|ui| {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.editing.password)
                                    .password(!self.show_password),
                            );
                            let eye = if self.show_password { "🙈" } else { "👁" };
                            if ui.small_button(eye).on_hover_text("Show / hide password (auto-hides after 1 min)").clicked() {
                                self.show_password = !self.show_password;
                                self.show_password_since =
                                    if self.show_password { Some(Instant::now()) } else { None };
                            }
                            // Green dot = password is set; gives confidence even when field is masked
                            if !self.editing.password.is_empty() {
                                ui.label(
                                    RichText::new("●")
                                        .color(Color32::from_rgb(40, 200, 40))
                                        .small(),
                                ).on_hover_text("Password is set");
                            }
                        });
                        ui.end_row();

                        // Hint when field looks empty but a saved password exists
                        if self.editing.password.is_empty() && !self.saved.password.is_empty() {
                            ui.label("");
                            ui.label(
                                RichText::new("✓ saved password on file — type to replace")
                                    .small()
                                    .color(Color32::GRAY),
                            );
                            ui.end_row();
                        }

                        ui.label("");
                        ui.checkbox(
                            &mut self.editing.config.use_keychain,
                            "Store in OS keychain",
                        );
                        ui.end_row();
                    });

                ui.add_space(4.0);
                if ui.button("Test Connection").clicked() {
                    self.test_conn_result = None;
                    // Snapshot password so we can restore it if the masked field
                    // appears to empty itself while the async task runs.
                    self.test_conn_pass_snapshot = self.editing.password.clone();
                    let url  = self.editing.config.qbit_url.clone();
                    let user = self.editing.config.qbit_user.clone();
                    let pass = self.editing.password.clone();
                    let handle = self.runtime.spawn(async move {
                        test_connection(&url, &user, &pass)
                            .await
                            .map_err(|e| e.to_string())
                    });
                    self.test_conn_pending = Some(handle);
                }

                // Poll test connection result
                if let Some(handle) = &self.test_conn_pending {
                    if handle.is_finished() {
                        let h = self.test_conn_pending.take().unwrap();
                        self.test_conn_result = Some(
                            self.runtime.block_on(h).unwrap_or(Err("Task panicked".into()))
                        );
                        // Restore password if it was cleared while the test ran
                        // (egui can clear a masked field on certain focus events).
                        if self.editing.password.is_empty()
                            && !self.test_conn_pass_snapshot.is_empty()
                        {
                            self.editing.password = self.test_conn_pass_snapshot.clone();
                        }
                        self.test_conn_pass_snapshot.clear();
                    }
                }

                if let Some(result) = &self.test_conn_result {
                    ui.add_space(2.0);
                    match result {
                        Ok(()) => { ui.colored_label(Color32::from_rgb(40, 200, 40), "✓ Connected"); }
                        Err(e) => { ui.colored_label(Color32::from_rgb(220, 60, 60), format!("✗ {e}")); }
                    }
                }
            });

            ui.add_space(6.0);

            // ── Monitor ──────────────────────────────────────────────────────
            ui.group(|ui| {
                ui.strong("Monitor");
                ui.add_space(4.0);

                ui.label("IP Check URLs");
                let mut to_remove: Option<usize> = None;
                for (i, url) in self.editing.config.ip_check_urls.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.text_edit_singleline(url);
                        if ui.small_button("✕").clicked() {
                            to_remove = Some(i);
                        }
                    });
                }
                if let Some(i) = to_remove {
                    self.editing.config.ip_check_urls.remove(i);
                }
                if ui.small_button("+ Add").clicked() {
                    self.editing.config.ip_check_urls.push(String::new());
                }

                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label("Poll interval");
                    ui.add(
                        egui::DragValue::new(&mut self.editing.config.poll_secs)
                            .clamp_range(5..=3600)
                            .speed(1.0),
                    );
                    ui.label("seconds");
                });

                ui.horizontal(|ui| {
                    ui.label("Fail threshold");
                    ui.add(
                        egui::DragValue::new(&mut self.editing.config.fail_threshold)
                            .clamp_range(1..=20)
                            .speed(1.0),
                    );
                    ui.label("consecutive failures");
                });
            });

            ui.add_space(6.0);

            // ── Application ──────────────────────────────────────────────────
            ui.group(|ui| {
                ui.strong("Application");
                ui.add_space(4.0);

                if ui.checkbox(
                    &mut self.editing.config.launch_at_startup,
                    "Launch at startup",
                ).changed() {
                    // Apply immediately (no need to Save)
                    match config::set_auto_launch(self.editing.config.launch_at_startup) {
                        Ok(())  => self.saved.config.launch_at_startup = self.editing.config.launch_at_startup,
                        Err(e)  => {
                            // Revert the checkbox
                            self.editing.config.launch_at_startup = !self.editing.config.launch_at_startup;
                            self.save_errors = vec![format!("Auto-launch error: {e}")];
                        }
                    }
                }
                ui.checkbox(
                    &mut self.editing.config.minimize_to_tray,
                    "Minimize to tray on close",
                );
            });

            // ── Validation errors ─────────────────────────────────────────────
            if !self.save_errors.is_empty() {
                ui.add_space(4.0);
                for err in &self.save_errors {
                    ui.colored_label(Color32::from_rgb(220, 60, 60), format!("⚠ {err}"));
                }
            }

            ui.add_space(8.0);

            // ── Save / Cancel ─────────────────────────────────────────────────
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    self.do_save();
                }
                if ui.button("Cancel").clicked() {
                    self.editing = self.saved.clone();
                    self.save_errors.clear();
                    self.test_conn_result = None;
                }
            });
        });
    }

    // ── Save logic ────────────────────────────────────────────────────────────

    fn do_save(&mut self) {
        let mut cfg = self.editing.config.clone();
        cfg.normalize();

        let errors = cfg.validate();
        if !errors.is_empty() {
            self.save_errors = errors.into_iter().map(|(_, msg)| msg).collect();
            return;
        }
        self.save_errors.clear();

        // Handle keychain
        if cfg.use_keychain {
            // If username changed, remove old entry
            if self.editing.config.qbit_user != self.editing.original_user {
                let _ = config::keychain_delete(&self.editing.original_user);
            }
            if let Err(e) = config::keychain_set(&cfg.qbit_user, &self.editing.password) {
                self.save_errors = vec![
                    format!("Keychain unavailable — password not saved. {e}"),
                    "Disable 'Store in OS keychain' to save password in config file.".into(),
                ];
                return;
            }
            cfg.qbit_pass = String::new(); // don't write password to TOML
        } else {
            cfg.qbit_pass = self.editing.password.clone();
            // Remove old keychain entry if switching off
            let _ = config::keychain_delete(&cfg.qbit_user);
        }

        if let Err(e) = cfg.save() {
            self.save_errors = vec![format!("Could not write config: {e}")];
            return;
        }

        // Resolve runtime password
        let (password, _) = config::resolve_password(&cfg);

        // Notify monitor
        let _ = self.cmd_tx.send(MonitorCommand::ReloadConfig {
            config:   cfg.clone(),
            password: password.clone(),
        });

        self.editing.config    = cfg.clone();
        self.editing.password  = password.clone();
        self.editing.original_user = cfg.qbit_user.clone();
        self.saved = self.editing.clone();
    }

    // ── Help / wiki tab ───────────────────────────────────────────────────────

    fn render_help(ui: &mut Ui) {
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.add_space(4.0);

            // ── Overview ──────────────────────────────────────────────────────
            ui.group(|ui| {
                ui.strong("How it works");
                ui.add_space(4.0);
                ui.label(
                    "qbit-killswitch watches your public IP address on a regular interval. \
                     The first IP it sees after launch is recorded as your safe VPN IP. \
                     If the IP ever changes — or stops being reachable — it immediately \
                     tells qBittorrent to pause every active torrent. Once the VPN comes \
                     back and the same IP is seen again, it resumes them automatically."
                );
            });

            ui.add_space(6.0);

            // ── States ────────────────────────────────────────────────────────
            ui.group(|ui| {
                ui.strong("States");
                ui.add_space(4.0);

                let states: &[(&str, Color32, &str)] = &[
                    ("STARTING",   Color32::GRAY,                        "Waiting for the first successful IP fetch to anchor the VPN IP."),
                    ("NOMINAL",    Color32::from_rgb(40, 200, 40),       "Current IP matches the recorded VPN IP. Torrents run normally."),
                    ("DEGRADED",   Color32::from_rgb(230, 180, 0),       "IP mismatch or fetch failure detected. Counting toward the fail threshold before pausing."),
                    ("PAUSED",     Color32::from_rgb(220, 60, 60),       "Threshold reached — all torrents are paused. Any torrent manually resumed will be re-paused automatically."),
                    ("RECOVERING", Color32::from_rgb(0, 210, 210),       "VPN restored — attempting to resume all torrents."),
                ];

                egui::Grid::new("help_states")
                    .num_columns(2)
                    .spacing([8.0, 5.0])
                    .show(ui, |ui| {
                        for (name, color, desc) in states {
                            ui.label(RichText::new(*name).color(*color).strong());
                            ui.label(*desc);
                            ui.end_row();
                        }
                    });
            });

            ui.add_space(6.0);

            // ── Settings ──────────────────────────────────────────────────────
            ui.group(|ui| {
                ui.strong("Settings reference");
                ui.add_space(4.0);

                egui::Grid::new("help_settings")
                    .num_columns(2)
                    .spacing([8.0, 5.0])
                    .show(ui, |ui| {
                        let rows: &[(&str, &str)] = &[
                            ("URL",              "qBittorrent Web UI address, e.g. http://127.0.0.1:8080"),
                            ("User / Password",  "Web UI login credentials (Tools → Options → Web UI in qBittorrent)."),
                            ("OS keychain",      "When enabled the password is stored in Windows Credential Manager / macOS Keychain / Linux Secret Service — never written to the config file."),
                            ("IP Check URLs",    "Services queried to get your current public IP. Tried in order; the first success wins. Multiple URLs prevent a dead service from causing a false positive."),
                            ("Poll interval",    "How often (in seconds) the IP is checked. Minimum 5 s."),
                            ("Fail threshold",   "How many consecutive failures are tolerated before torrents are paused. A value of 3 means three bad checks in a row, not three bad seconds."),
                            ("Launch at startup","Registers the app to start with Windows / Linux / macOS automatically."),
                            ("Minimize to tray", "Closing the window hides it to the system tray rather than quitting."),
                        ];
                        for (field, desc) in rows {
                            ui.label(RichText::new(*field).strong());
                            ui.label(*desc);
                            ui.end_row();
                        }
                    });
            });

            ui.add_space(6.0);

            // ── Tray ──────────────────────────────────────────────────────────
            ui.group(|ui| {
                ui.strong("Tray menu");
                ui.add_space(4.0);
                ui.label("Right-click the tray icon to access:");
                ui.add_space(4.0);

                egui::Grid::new("help_tray")
                    .num_columns(2)
                    .spacing([8.0, 5.0])
                    .show(ui, |ui| {
                        let rows: &[(&str, &str)] = &[
                            ("Open qbit-killswitch", "Show the main window."),
                            ("Status / VPN IP",      "Read-only status display."),
                            ("Resume all torrents",  "Manually resume — available when paused or degraded."),
                            ("Pause all torrents",   "Manually pause all torrents immediately."),
                            ("Re-detect VPN IP",     "Re-anchor the daemon to the current public IP (use after switching VPN server)."),
                            ("Quit",                 "Exit the app. Torrents will not be managed after this."),
                        ];
                        for (item, desc) in rows {
                            ui.label(RichText::new(*item).strong());
                            ui.label(*desc);
                            ui.end_row();
                        }
                    });
            });

            ui.add_space(6.0);

            // ── Tips ──────────────────────────────────────────────────────────
            ui.group(|ui| {
                ui.strong("Tips");
                ui.add_space(4.0);
                for tip in [
                    "Click the 👁 button next to the password field to reveal it. It hides again automatically after 1 minute.",
                    "Use Test Connection before saving to confirm your credentials work.",
                    "After switching VPN servers, click Re-detect VPN IP (or use the tray menu) to update the reference IP.",
                    "If the Web UI is on a non-standard port, make sure to include it in the URL (e.g. http://127.0.0.1:9090).",
                    "A fail threshold of 2–3 is recommended to absorb brief IP-check service outages without false positives.",
                ] {
                    ui.horizontal(|ui| {
                        ui.label("•");
                        ui.label(tip);
                    });
                }
            });

            ui.add_space(4.0);
        });
    }

    // ── Unsaved-changes modal ─────────────────────────────────────────────────

    fn render_unsaved_dialog(&mut self, ctx: &egui::Context) {
        egui::Window::new("Unsaved Changes")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.label("You have unsaved changes. What would you like to do?");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        self.do_save();
                        self.show_unsaved_dialog = false;
                        if self.quit_after_dialog {
                            self.force_quit = true;
                            ctx.send_viewport_cmd(ViewportCommand::Close);
                        }
                    }
                    if ui.button("Discard").clicked() {
                        self.editing = self.saved.clone();
                        self.show_unsaved_dialog = false;
                        if self.quit_after_dialog {
                            self.force_quit = true;
                            ctx.send_viewport_cmd(ViewportCommand::Close);
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        self.show_unsaved_dialog = false;
                        self.quit_after_dialog = false;
                    }
                });
            });
    }
}

// ── eframe::App ───────────────────────────────────────────────────────────────

impl eframe::App for KillswitchApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // ── Handle window close request ────────────────────────────────────────
        if ctx.input(|i| i.viewport().close_requested())
            && !self.force_quit
            && self.saved.config.minimize_to_tray
        {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            if self.editing.has_changes(&self.saved) {
                self.show_unsaved_dialog = true;
                self.quit_after_dialog   = false;
            } else {
                ctx.send_viewport_cmd(ViewportCommand::Visible(false));
            }
        }
        // If force_quit or minimize_to_tray=false, eframe handles the close normally.

        // ── Auto-hide password after 60 s ─────────────────────────────────────
        if let Some(since) = self.show_password_since {
            if since.elapsed() >= Duration::from_secs(60) {
                self.show_password = false;
                self.show_password_since = None;
            }
        }

        // ── Poll async tasks & tray ────────────────────────────────────────────
        self.poll_tray_events(ctx);

        // ── Read shared state ──────────────────────────────────────────────────
        let (app_state, vpn_ip, fail_streak, fail_threshold, keychain_warn, transient, redetect) = {
            let mut shared = self.shared.lock().unwrap();
            let state      = shared.app_state;
            let ip         = shared.vpn_ip;
            let streak     = shared.fail_streak;
            let threshold  = self.saved.config.fail_threshold;
            let kw         = shared.keychain_warning;
            // Expire transient messages after 3 s
            let msg = shared.transient_message.as_ref().and_then(|(m, t)| {
                if t.elapsed() < Duration::from_secs(3) { Some(m.clone()) } else { None }
            });
            if msg.is_none() { shared.transient_message = None; }
            let rd = shared.redetect_result.take();
            (state, ip, streak, threshold, kw, msg, rd)
        };

        // Surface re-detect result as a transient message
        if let Some(result) = redetect {
            let msg = match &result {
                Ok(ip) => format!("VPN IP updated to {ip}"),
                Err(e) => e.clone(),
            };
            self.shared.lock().unwrap().set_transient(msg);
        }

        // ── Blink phase for RECOVERY (toggles every 500 ms) ────────────────────
        let blink_phase = (self.blink_start.elapsed().as_millis() / 500).is_multiple_of(2);
        if app_state == AppState::Recovery {
            ctx.request_repaint_after(Duration::from_millis(500));
        }

        // ── Update tray icon ───────────────────────────────────────────────────
        self.update_tray(app_state, vpn_ip, blink_phase);

        // ── Status bar ─────────────────────────────────────────────────────────
        egui::TopBottomPanel::top("status_bar")
            .min_height(48.0)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    // Dot indicator (blink for recovery)
                    let dot_color = if app_state == AppState::Recovery && !blink_phase {
                        Color32::DARK_GRAY
                    } else {
                        state_color(app_state)
                    };
                    let (rect, _) = ui.allocate_exact_size(
                        egui::vec2(12.0, 16.0),
                        egui::Sense::hover(),
                    );
                    paint_shield(&ui.painter(), rect, dot_color);

                    // Status label
                    let label = match app_state {
                        AppState::Starting => "STARTING — waiting for VPN…".to_string(),
                        AppState::Nominal  => {
                            let ip_str = vpn_ip.map_or("—".to_string(), |ip| ip.to_string());
                            if keychain_warn {
                                format!("⚠ Keychain unavailable — password in plaintext  |  VPN IP: {ip_str}")
                            } else {
                                format!("NOMINAL — VPN IP: {ip_str}")
                            }
                        }
                        AppState::Degraded => format!("DEGRADED — {fail_streak}/{fail_threshold} failures"),
                        AppState::Paused   => "PAUSED — torrents halted".to_string(),
                        AppState::Recovery => "RECOVERING…".to_string(),
                    };

                    let color = if keychain_warn && app_state == AppState::Nominal {
                        Color32::from_rgb(240, 195, 0)
                    } else {
                        state_color(app_state)
                    };
                    ui.label(RichText::new(&label).color(color).strong());

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Re-detect VPN IP").clicked() {
                            let _ = self.cmd_tx.send(MonitorCommand::RedetectVpnIp);
                        }
                    });
                });

                // Transient message row
                if let Some(msg) = &transient {
                    ui.add_space(2.0);
                    ui.label(RichText::new(msg).small().italics().color(Color32::GRAY));
                }

                // One-time load warning
                if let Some(warn) = self.load_warning.take() {
                    ui.add_space(2.0);
                    ui.colored_label(Color32::from_rgb(240, 150, 0), format!("⚠ {warn}"));
                }

                ui.add_space(4.0);
            });

        // ── Main panel (tabbed) ────────────────────────────────────────────────
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.active_tab, AppTab::Settings, "⚙  Settings");
                ui.selectable_value(&mut self.active_tab, AppTab::Help,     "?  Help");
            });
            ui.separator();
            match self.active_tab {
                AppTab::Settings => self.render_settings(ui),
                AppTab::Help     => Self::render_help(ui),
            }
        });

        // ── Unsaved changes dialog ──────────────────────────────────────
        if self.show_unsaved_dialog {
            self.render_unsaved_dialog(ctx);
        }

        // ── Keep update() running while minimized to tray ───────────────
        // Without this, eframe stops calling update() when the window is
        // hidden, so poll_tray_events() never fires and the tray menu is dead.
        if self.saved.config.minimize_to_tray {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}
