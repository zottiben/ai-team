//! Shared, explicitly approved application updates. All program replacement goes
//! through `UpdateManager`; the pre-0.5 broken-app detector only explains recovery.
//! Downloads use curl, which is already required by the bootstrap installer.

pub(crate) mod fence;
mod manager;
pub use manager::{
    UpdateInspection, UpdateInstallation, UpdateManager, UpdatePermit, UpdateState, UpdateStatus,
    UpdateTarget, Updated,
};

use crate::error::{Error, Result};
use serde::Serialize;
use std::ffi::OsStr;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;

const REPO: &str = "zottiben/ai-team";
const CLI: &str = "ait";
const APP: &str = "ai-team.app";

pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Declared by the process, never inferred from a destination path. The old updater
/// replaced the desktop executable with `ait`, leaving an app that printed CLI help.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Host {
    #[default]
    Cli,
    Desktop,
}

/// Recognise the executable's bundle by its layout, not its product name. This is
/// deliberately tested on Linux too: platform-sensitive path rules must not rot.
fn bundle_of(binary: &Path) -> Option<PathBuf> {
    let macos = binary.parent()?;
    if macos.file_name() != Some(OsStr::new("MacOS")) {
        return None;
    }
    let contents = macos.parent()?;
    if contents.file_name() != Some(OsStr::new("Contents")) {
        return None;
    }
    let app = contents.parent()?;
    (app.extension() == Some(OsStr::new("app"))).then(|| app.to_path_buf())
}

/// Detect a CLI installed over the desktop by releases up to 0.5.0. Detection must
/// not repair automatically: opening an app is not approval to replace or restart it.
pub fn replaced_app(cli: &Path) -> Option<PathBuf> {
    if cli.file_name() == Some(OsStr::new(CLI)) {
        return None;
    }
    bundle_of(cli)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    Release,
    Source,
    /// No installer marker: do not overwrite an installation owned by other tooling.
    Unknown,
}

pub fn method() -> Method {
    let Ok(path) = crate::paths::data_dir() else {
        return Method::Unknown;
    };
    match std::fs::read_to_string(path.join("install-method"))
        .unwrap_or_default()
        .trim()
    {
        "release" => Method::Release,
        "source" => Method::Source,
        _ => Method::Unknown,
    }
}

/// A failed lookup is not evidence that the installed programs are current.
pub(crate) async fn latest() -> Option<String> {
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "10",
            "-o",
            "/dev/null",
            "-w",
            "%{url_effective}",
            &format!("https://github.com/{REPO}/releases/latest"),
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    if !output.status.success() {
        return None;
    }
    release_version(String::from_utf8_lossy(&output.stdout).trim())
}

fn release_version(url: &str) -> Option<String> {
    let value = url.strip_prefix(&format!("https://github.com/{REPO}/releases/tag/v"))?;
    let parsed = semver::Version::parse(value).ok()?;
    (parsed.pre.is_empty() && parsed.build.is_empty()).then(|| parsed.to_string())
}

fn version(text: &str) -> Option<semver::Version> {
    let mut text = text.to_string();
    // Preserve older callers' major.minor shorthand.
    if text.split('.').count() == 2 && !text.contains(['-', '+']) {
        text.push_str(".0");
    }
    semver::Version::parse(&text).ok()
}

pub fn is_newer(candidate: &str, than: &str) -> bool {
    match (version(candidate), version(than)) {
        (Some(a), Some(b)) => a.cmp_precedence(&b).is_gt(),
        _ => false,
    }
}

pub(crate) fn asset_for(version: &str) -> Option<String> {
    let arch = std::env::consts::ARCH;
    match std::env::consts::OS {
        "macos" => Some(format!("ai-team-v{version}-macos-universal.tar.gz")),
        "linux" => match arch {
            "x86_64" => Some(format!("ai-team-v{version}-linux-x86_64.tar.gz")),
            "aarch64" => Some(format!("ai-team-v{version}-linux-aarch64.tar.gz")),
            _ => None,
        },
        _ => None,
    }
}

/// Check the executable macOS will launch, not an assumed product name. Tauri writes
/// XML; a plist that cannot be checked is not permission to replace a working app.
fn executable_of(bundle: &Path) -> Result<PathBuf> {
    let plist = std::fs::read_to_string(bundle.join("Contents/Info.plist")).unwrap_or_default();
    let executable = plist
        .split_once("<key>CFBundleExecutable</key>")
        .and_then(|(_, rest)| rest.trim_start().strip_prefix("<string>"))
        .and_then(|rest| rest.split_once("</string>"))
        .map(|(name, _)| name.trim())
        .filter(|name| !name.is_empty() && !name.contains('/'))
        .map(|name| bundle.join("Contents/MacOS").join(name));
    match executable {
        Some(path) if path.is_file() => Ok(path),
        _ => Err(Error::invalid(format!(
            "this release's {APP} names no executable that it contains - not installing it"
        ))),
    }
}

fn same_contents(a: &Path, b: &Path) -> bool {
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) if a.len() == b.len() => {}
        _ => return false,
    }
    matches!((std::fs::read(a), std::fs::read(b)), (Ok(a), Ok(b)) if a == b)
}

async fn curl(url: &str, into: &Path) -> Result<()> {
    let status = Command::new("curl")
        .args(["-fsSL", "--max-time", "300", url, "-o"])
        .arg(into)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .status()
        .await
        .map_err(|error| Error::invalid(format!("could not run curl: {error}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::invalid(format!("could not download {url}")))
    }
}

fn verify(archive: &Path, sums: &Path, name: &str) -> Result<()> {
    let listed = std::fs::read_to_string(sums).map_err(|error| {
        Error::invalid(format!("could not read the published checksums: {error}"))
    })?;
    let matches: Vec<_> = listed
        .lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let digest = fields.next()?;
            let file = fields.next()?.trim_start_matches('*');
            (file == name && fields.next().is_none()).then_some(digest)
        })
        .collect();
    let [expected] = matches.as_slice() else {
        return Err(Error::invalid(
            "the release must publish exactly one checksum for the requested asset",
        ));
    };
    let bytes = std::fs::read(archive)
        .map_err(|error| Error::invalid(format!("could not read the download: {error}")))?;
    if sha256(&bytes).eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(Error::invalid(
            "the download does not match its published checksum - not installing it",
        ))
    }
}

/// SHA-256, checked against the standard's published test vectors.
pub(crate) fn sha256(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a_2f98,
        0x7137_4491,
        0xb5c0_fbcf,
        0xe9b5_dba5,
        0x3956_c25b,
        0x59f1_11f1,
        0x923f_82a4,
        0xab1c_5ed5,
        0xd807_aa98,
        0x1283_5b01,
        0x2431_85be,
        0x550c_7dc3,
        0x72be_5d74,
        0x80de_b1fe,
        0x9bdc_06a7,
        0xc19b_f174,
        0xe49b_69c1,
        0xefbe_4786,
        0x0fc1_9dc6,
        0x240c_a1cc,
        0x2de9_2c6f,
        0x4a74_84aa,
        0x5cb0_a9dc,
        0x76f9_88da,
        0x983e_5152,
        0xa831_c66d,
        0xb003_27c8,
        0xbf59_7fc7,
        0xc6e0_0bf3,
        0xd5a7_9147,
        0x06ca_6351,
        0x1429_2967,
        0x27b7_0a85,
        0x2e1b_2138,
        0x4d2c_6dfc,
        0x5338_0d13,
        0x650a_7354,
        0x766a_0abb,
        0x81c2_c92e,
        0x9272_2c85,
        0xa2bf_e8a1,
        0xa81a_664b,
        0xc24b_8b70,
        0xc76c_51a3,
        0xd192_e819,
        0xd699_0624,
        0xf40e_3585,
        0x106a_a070,
        0x19a4_c116,
        0x1e37_6c08,
        0x2748_774c,
        0x34b0_bcb5,
        0x391c_0cb3,
        0x4ed8_aa4a,
        0x5b9c_ca4f,
        0x682e_6ff3,
        0x748f_82ee,
        0x78a5_636f,
        0x84c8_7814,
        0x8cc7_0208,
        0x90be_fffa,
        0xa450_6ceb,
        0xbef9_a3f7,
        0xc671_78f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09_e667,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];
    let mut message = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bits.to_be_bytes());
    for block in message.as_chunks::<64>().0 {
        compress(&mut h, block, &K);
    }
    let mut out = String::with_capacity(64);
    for word in h {
        let _ = write!(out, "{word:08x}");
    }
    out
}

fn compress(h: &mut [u32; 8], block: &[u8; 64], k: &[u32; 64]) {
    let mut w = [0u32; 64];
    for (index, chunk) in block.as_chunks::<4>().0.iter().enumerate() {
        w[index] = u32::from_be_bytes(*chunk);
    }
    for index in 16..64 {
        let s0 =
            w[index - 15].rotate_right(7) ^ w[index - 15].rotate_right(18) ^ (w[index - 15] >> 3);
        let s1 =
            w[index - 2].rotate_right(17) ^ w[index - 2].rotate_right(19) ^ (w[index - 2] >> 10);
        w[index] = w[index - 16]
            .wrapping_add(s0)
            .wrapping_add(w[index - 7])
            .wrapping_add(s1);
    }
    let mut v = *h;
    for index in 0..64 {
        let s1 = v[4].rotate_right(6) ^ v[4].rotate_right(11) ^ v[4].rotate_right(25);
        let ch = (v[4] & v[5]) ^ ((!v[4]) & v[6]);
        let t1 = v[7]
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(k[index])
            .wrapping_add(w[index]);
        let s0 = v[0].rotate_right(2) ^ v[0].rotate_right(13) ^ v[0].rotate_right(22);
        let maj = (v[0] & v[1]) ^ (v[0] & v[2]) ^ (v[1] & v[2]);
        let t2 = s0.wrapping_add(maj);
        v[7] = v[6];
        v[6] = v[5];
        v[5] = v[4];
        v[4] = v[3].wrapping_add(t1);
        v[3] = v[2];
        v[2] = v[1];
        v[1] = v[0];
        v[0] = t1.wrapping_add(t2);
    }
    for (into, from) in h.iter_mut().zip(v) {
        *into = into.wrapping_add(from);
    }
}

#[cfg(test)]
mod tests;
