use std::net::IpAddr;
use std::time::Duration;

use crate::config::Config;
use crate::ip::fetch_ip;
use crate::qbit::QbitClient;
use crate::state::{AppState, CommandReceiver, MonitorCommand, SharedStateHandle};

pub struct MonitorTask {
    shared:            SharedStateHandle,
    cmd_rx:            CommandReceiver,
    config:            Config,
    password:          String,
    client:            QbitClient,
    /// The confirmed VPN IP. None until first successful fetch in STARTING state.
    vpn_ip:            Option<IpAddr>,
    fail_streak:       u32,
    start_fail_streak: u32,
}

impl MonitorTask {
    pub fn new(
        shared:   SharedStateHandle,
        cmd_rx:   CommandReceiver,
        config:   Config,
        password: String,
    ) -> Self {
        let client = QbitClient::new(&config.qbit_url, &config.qbit_user, &password)
            .expect("Failed to build HTTP client");
        Self {
            shared,
            cmd_rx,
            config,
            password,
            client,
            vpn_ip: None,
            fail_streak: 0,
            start_fail_streak: 0,
        }
    }

    // ── Main loop ─────────────────────────────────────────────────────────────

    pub async fn run(mut self) {
        if let Err(e) = self.client.login().await {
            eprintln!("[monitor] Initial login failed: {e}");
        }
        loop {
            let poll = Duration::from_secs(self.config.poll_secs);
            tokio::select! {
                _ = tokio::time::sleep(poll) => {
                    self.poll().await;
                }
                Some(cmd) = self.cmd_rx.recv() => {
                    self.handle_command(cmd).await;
                }
            }
        }
    }

    // ── Poll ──────────────────────────────────────────────────────────────────

    async fn poll(&mut self) {
        let ip    = fetch_ip(self.client.inner(), &self.config.ip_check_urls).await;
        let state = self.current_state();
        match state {
            AppState::Starting           => self.tick_starting(ip).await,
            AppState::Nominal
            | AppState::Degraded        => self.tick_monitoring(ip).await,
            AppState::Paused
            | AppState::Recovery        => self.tick_paused(ip).await,
        }
        self.flush_to_shared();
    }

    // ── STARTING ──────────────────────────────────────────────────────────────

    async fn tick_starting(&mut self, ip: Option<IpAddr>) {
        match ip {
            Some(addr) => {
                println!("[monitor] VPN IP recorded: {addr}");
                self.vpn_ip           = Some(addr);
                self.start_fail_streak = 0;
                self.fail_streak       = 0;
                self.set_state(AppState::Nominal);
            }
            None => {
                self.start_fail_streak += 1;
                let limit = self.config.fail_threshold.saturating_mul(3).max(3);
                eprintln!(
                    "[monitor] STARTING: no IP ({}/{})",
                    self.start_fail_streak, limit
                );
                if self.start_fail_streak >= limit {
                    eprintln!("[monitor] Could not determine VPN IP — pausing as precaution");
                    self.do_pause_all().await;
                    self.set_state(AppState::Paused);
                    self.shared.lock().unwrap().set_transient(
                        "Could not determine VPN IP — torrents paused as a precaution",
                    );
                }
            }
        }
    }

    // ── NOMINAL / DEGRADED ────────────────────────────────────────────────────

    async fn tick_monitoring(&mut self, ip: Option<IpAddr>) {
        let vpn = match self.vpn_ip {
            Some(v) => v,
            None => {
                self.set_state(AppState::Starting);
                return;
            }
        };
        match ip {
            Some(addr) if addr == vpn => {
                self.fail_streak = 0;
                self.set_state(AppState::Nominal);
            }
            Some(addr) => {
                eprintln!("[monitor] IP mismatch: got {addr}, expected {vpn}");
                self.fail_streak += 1;
                self.set_state(AppState::Degraded);
                if self.fail_streak >= self.config.fail_threshold {
                    eprintln!("[monitor] Threshold reached — pausing torrents");
                    self.do_pause_all().await;
                    self.set_state(AppState::Paused);
                }
            }
            None => {
                eprintln!("[monitor] IP check failed (all services unreachable)");
                self.fail_streak += 1;
                self.set_state(AppState::Degraded);
                if self.fail_streak >= self.config.fail_threshold {
                    eprintln!("[monitor] Threshold reached — pausing torrents");
                    self.do_pause_all().await;
                    self.set_state(AppState::Paused);
                }
            }
        }
    }

    // ── PAUSED ────────────────────────────────────────────────────────────────

    async fn tick_paused(&mut self, ip: Option<IpAddr>) {
        // ① Re-pause enforcement: catch torrents resumed outside the daemon
        match self.client.downloading_count().await {
            Ok(n) if n > 0 => {
                eprintln!("[monitor] Re-pausing {n} torrent(s) resumed outside the daemon");
                self.do_pause_all().await;
                self.shared.lock().unwrap().set_transient(
                    format!("Re-paused {n} torrent(s) that were manually resumed"),
                );
            }
            Ok(_)   => {}
            Err(e)  => eprintln!("[monitor] Could not check active torrents: {e}"),
        }

        // ② Check if VPN has recovered
        let vpn = match self.vpn_ip {
            Some(v) => v,
            None    => return,
        };
        if ip == Some(vpn) {
            println!("[monitor] VPN restored — resuming torrents");
            self.set_state(AppState::Recovery);
            self.flush_to_shared();

            match self.client.resume_all().await {
                Ok(()) => {
                    println!("[monitor] Torrents resumed");
                    self.fail_streak = 0;
                    self.set_state(AppState::Nominal);
                }
                Err(e) => {
                    eprintln!("[monitor] Resume failed: {e} — will retry next poll");
                    self.set_state(AppState::Paused);
                }
            }
        }
    }

    // ── Commands ──────────────────────────────────────────────────────────────

    async fn handle_command(&mut self, cmd: MonitorCommand) {
        match cmd {
            MonitorCommand::RedetectVpnIp => {
                println!("[monitor] Re-detecting VPN IP…");
                let ip = fetch_ip(self.client.inner(), &self.config.ip_check_urls).await;
                let result: Result<IpAddr, String> = match ip {
                    Some(addr) => {
                        println!("[monitor] Re-detected VPN IP: {addr}");
                        self.vpn_ip      = Some(addr);
                        self.fail_streak = 0;
                        let state = self.current_state();
                        if matches!(state, AppState::Paused | AppState::Degraded) {
                            match self.client.resume_all().await {
                                Ok(())  => self.set_state(AppState::Nominal),
                                Err(e)  => eprintln!("[monitor] Resume after re-detect failed: {e}"),
                            }
                        } else {
                            self.set_state(AppState::Nominal);
                        }
                        Ok(addr)
                    }
                    None => {
                        eprintln!("[monitor] Re-detect failed — all services unreachable");
                        Err("Could not reach any IP check service — try again".into())
                    }
                };
                {
                    let mut shared = self.shared.lock().unwrap();
                    shared.redetect_result = Some(result);
                }
                self.flush_to_shared();
            }

            MonitorCommand::ManualPause => {
                println!("[monitor] Manual pause");
                self.do_pause_all().await;
                self.set_state(AppState::Paused);
                self.flush_to_shared();
            }

            MonitorCommand::ManualResume => {
                println!("[monitor] Manual resume");
                match self.client.resume_all().await {
                    Ok(()) => {
                        self.fail_streak = 0;
                        self.set_state(AppState::Nominal);
                    }
                    Err(e) => {
                        eprintln!("[monitor] Manual resume failed: {e}");
                        self.shared.lock().unwrap()
                            .set_transient(format!("Resume failed: {e}"));
                    }
                }
                self.flush_to_shared();
            }

            MonitorCommand::ReloadConfig { config, password } => {
                println!("[monitor] Reloading config");
                self.client.update_credentials(
                    &config.qbit_url,
                    &config.qbit_user,
                    &password,
                );
                self.config   = config;
                self.password = password;
                if let Err(e) = self.client.login().await {
                    eprintln!("[monitor] Re-login with new config failed: {e}");
                }
            }
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn current_state(&self) -> AppState {
        self.shared.lock().unwrap().app_state
    }

    fn set_state(&self, state: AppState) {
        self.shared.lock().unwrap().app_state = state;
    }

    async fn do_pause_all(&self) {
        if let Err(e) = self.client.pause_all().await {
            eprintln!("[monitor] pause_all failed: {e}");
        }
    }

    fn flush_to_shared(&self) {
        let mut shared       = self.shared.lock().unwrap();
        shared.vpn_ip        = self.vpn_ip;
        shared.fail_streak   = self.fail_streak;
        shared.start_fail_streak = self.start_fail_streak;
        shared.request_repaint();
    }
}
