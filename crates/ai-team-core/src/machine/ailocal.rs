//! ai-local owns its endpoint and credential; ai-team only reads them.

use std::net::{IpAddr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

const AILOCAL_DEFAULT_URL: &str = "http://127.0.0.1:8081/v1";

#[derive(Debug, Clone)]
pub(super) struct AilocalSettings {
    pub(super) base_url: String,
    pub(super) key: Option<String>,
    address: IpAddr,
    port: u16,
    pub(super) issue: Option<String>,
}

#[derive(Debug, Deserialize)]
struct AilocalFile {
    gateway_host: Option<String>,
    gateway_port: Option<u16>,
}

impl AilocalSettings {
    pub(super) fn load() -> Self {
        let Some(dir) = ailocal_config_dir() else {
            return Self::unavailable("cannot locate ailocal's config directory");
        };
        Self::from_dir(&dir)
    }

    fn from_dir(dir: &Path) -> Self {
        let path = dir.join("config.toml");
        let source = match std::fs::read_to_string(&path) {
            Ok(source) => source,
            Err(e) => {
                return Self::unavailable(&format!("cannot read {}: {e}", path.display()));
            }
        };
        let config: AilocalFile = match toml::from_str(&source) {
            Ok(config) => config,
            Err(e) => {
                return Self::unavailable(&format!("cannot parse {}: {e}", path.display()));
            }
        };
        let host = config.gateway_host.as_deref().unwrap_or("127.0.0.1");
        let host = match host {
            "0.0.0.0" | "::" | "[::]" | "localhost" => "127.0.0.1",
            other => other,
        };
        let address = match host.parse::<IpAddr>() {
            Ok(address) if address.is_loopback() => address,
            _ => {
                return Self::unavailable(&format!(
                    "ailocal gateway_host {host:?} is not loopback; ai-team will not send prompts to it"
                ));
            }
        };
        let port = config.gateway_port.unwrap_or(8081);
        if port == 0 {
            return Self::unavailable("ailocal gateway_port cannot be 0");
        }
        let url_host = match address {
            IpAddr::V4(address) => address.to_string(),
            IpAddr::V6(address) => format!("[{address}]"),
        };
        let key_path = dir.join("gateway.key");
        let key = std::fs::read_to_string(&key_path)
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let issue = key
            .is_none()
            .then(|| format!("{} is missing or empty", key_path.display()));
        AilocalSettings {
            base_url: format!("http://{url_host}:{port}/v1"),
            key,
            address,
            port,
            issue,
        }
    }

    fn unavailable(reason: &str) -> Self {
        AilocalSettings {
            base_url: AILOCAL_DEFAULT_URL.into(),
            key: None,
            address: IpAddr::V4([127, 0, 0, 1].into()),
            port: 8081,
            issue: Some(reason.into()),
        }
    }

    pub(super) fn probe(&self) -> std::result::Result<String, String> {
        if let Some(issue) = &self.issue {
            return Err(issue.clone());
        }
        let address = SocketAddr::new(self.address, self.port);
        TcpStream::connect_timeout(&address, Duration::from_millis(250))
            .map_err(|_| format!("ailocal gateway is not listening on {address}"))?;
        Ok(format!("ailocal gateway on {address}"))
    }
}

fn ailocal_config_dir() -> Option<PathBuf> {
    if let Some(path) = non_empty_env("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(path).join("ailocal"));
    }
    non_empty_env("HOME").map(|home| PathBuf::from(home).join(".config/ailocal"))
}

pub(super) fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(config: &str) -> (tempfile::TempDir, AilocalSettings) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), config).unwrap();
        std::fs::write(dir.path().join("gateway.key"), "secret\n").unwrap();
        let settings = AilocalSettings::from_dir(dir.path());
        (dir, settings)
    }

    #[test]
    fn a_bind_all_address_is_reached_only_over_loopback() {
        let (_, settings) = settings("gateway_host='0.0.0.0'\ngateway_port=9191\n");
        assert_eq!(settings.base_url, "http://127.0.0.1:9191/v1");
        assert_eq!(settings.key.as_deref(), Some("secret"));
    }

    #[test]
    fn ipv6_loopback_is_bracketed_and_non_loopback_is_refused() {
        let (_, ipv6) = settings("gateway_host='::1'\ngateway_port=8081\n");
        assert_eq!(ipv6.base_url, "http://[::1]:8081/v1");

        let (_, remote) = settings("gateway_host='192.0.2.10'\ngateway_port=8081\n");
        assert!(remote.issue.unwrap().contains("not loopback"));
    }
}
