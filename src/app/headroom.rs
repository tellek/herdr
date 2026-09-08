//! Background management of the `headroom proxy` process, toggled from the
//! global menu and optionally auto-started on herdr startup (see the
//! "auto start headroom proxy" experimental setting).

use std::process::{Child, Command, Stdio};

use super::App;

pub(super) fn spawn_headroom_proxy() -> std::io::Result<Child> {
    let mut cmd = Command::new("headroom");
    cmd.args(["proxy", "--port", "8787"]);
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.spawn()
}

impl App {
    /// Starts the headroom proxy if it isn't running, or kills it if it is.
    pub(crate) fn toggle_headroom_proxy(&mut self) {
        if let Some(mut child) = self.headroom_proxy_child.take() {
            let _ = child.kill();
            let _ = child.wait();
            self.state.headroom_proxy_running = false;
            return;
        }
        match spawn_headroom_proxy() {
            Ok(child) => {
                self.headroom_proxy_child = Some(child);
                self.state.headroom_proxy_running = true;
            }
            Err(err) => {
                self.state.config_diagnostic =
                    Some(format!("failed to start headroom proxy: {err}"));
                self.config_diagnostic_deadline =
                    Some(std::time::Instant::now() + std::time::Duration::from_secs(5));
            }
        }
    }

    /// Starts the headroom proxy if it isn't already running (used at
    /// startup when the "auto start headroom proxy" setting is enabled).
    pub(crate) fn start_headroom_proxy_if_needed(&mut self) {
        if self.headroom_proxy_child.is_some() {
            return;
        }
        self.toggle_headroom_proxy();
    }

    /// Clears `headroom_proxy_running` if the tracked process has exited on
    /// its own (crash, killed externally, etc).
    pub(crate) fn poll_headroom_proxy(&mut self) {
        if let Some(child) = self.headroom_proxy_child.as_mut() {
            if matches!(child.try_wait(), Ok(Some(_))) {
                self.headroom_proxy_child = None;
                self.state.headroom_proxy_running = false;
            }
        }
    }
}
