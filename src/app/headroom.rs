//! Background management of the `headroom proxy` process, toggled from the
//! global menu and optionally auto-started on herdr startup (see the
//! "auto start headroom proxy" experimental setting).

use std::net::{SocketAddr, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use super::App;

const HEADROOM_PROXY_PORT: u16 = 8787;
/// How often `poll_headroom_proxy` re-checks the proxy port when we don't
/// hold a child handle for it (e.g. right after startup, before we've
/// spawned or observed anything).
const HEADROOM_PROXY_PROBE_INTERVAL: Duration = Duration::from_secs(3);
const HEADROOM_PROXY_PROBE_TIMEOUT: Duration = Duration::from_millis(150);

/// Whether something is listening on the headroom proxy's port. Used to
/// detect an already-running proxy that this `App` instance didn't spawn
/// (e.g. left over from a previous herdr session).
fn headroom_proxy_port_open() -> bool {
    port_open(HEADROOM_PROXY_PORT)
}

fn port_open(port: u16) -> bool {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    TcpStream::connect_timeout(&addr, HEADROOM_PROXY_PROBE_TIMEOUT).is_ok()
}

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
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                self.state.config_diagnostic = Some(
                    "headroom is not installed — get it at https://github.com/headroomlabs-ai/headroom"
                        .to_string(),
                );
                self.config_diagnostic_deadline = Some(Instant::now() + Duration::from_secs(10));
            }
            Err(err) => {
                self.state.config_diagnostic =
                    Some(format!("failed to start headroom proxy: {err}"));
                self.config_diagnostic_deadline = Some(Instant::now() + Duration::from_secs(5));
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
    /// its own (crash, killed externally, etc), and, when we don't hold a
    /// child handle at all, periodically probes the proxy's port so the
    /// badge reflects an instance we didn't spawn ourselves (e.g. left
    /// running by a previous herdr session).
    pub(crate) fn poll_headroom_proxy(&mut self) {
        if let Some(child) = self.headroom_proxy_child.as_mut() {
            if matches!(child.try_wait(), Ok(Some(_))) {
                self.headroom_proxy_child = None;
                self.state.headroom_proxy_running = false;
            }
            return;
        }

        let now = Instant::now();
        if now < self.next_headroom_proxy_probe {
            return;
        }
        self.next_headroom_proxy_probe = now + HEADROOM_PROXY_PROBE_INTERVAL;
        self.state.headroom_proxy_running = headroom_proxy_port_open();
    }
}

#[cfg(test)]
mod tests {
    use super::port_open;
    use std::net::TcpListener;

    #[test]
    fn port_open_detects_listening_port() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let port = listener.local_addr().expect("local addr").port();
        assert!(port_open(port));

        drop(listener);
        assert!(!port_open(port));
    }
}
