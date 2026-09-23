//! Updating ai-team in place.
//!
//! How ai-team was installed decides how it updates, and guessing gets it wrong in the
//! one direction that matters. `install.sh` writes a marker saying `release` or `source`
//! precisely because a cargo-installed binary that was later replaced by a release still
//! has stale `~/.cargo` metadata - and a self-update that trusts that metadata would
//! rebuild an old clone and silently *downgrade* somebody.
//!
//! Downloads go through `curl` rather than a Rust HTTP client. It is already required to
//! install ai-team at all, it ships with macOS and every Linux worth the name, and adding
//! a TLS stack to this crate to fetch one tarball a month would be a large dependency for
//! a small job (D4's reasoning, applied to a library rather than a neighbour).
//!
//! A release archive carries **two** programs - `ait` and the desktop app - so an update
//! has to install the one it is replacing. Which one that is comes from the caller as a
//! [`Host`], because the process knows what it is and a path does not.

use std::ffi::OsStr;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use serde::Serialize;
use tokio::process::Command;

use crate::error::{Error, Result};

const REPO: &str = "zottiben/ai-team";

/// What the CLI is called inside a release archive.
const CLI: &str = "ait";
/// What the macOS app is called inside a release archive.
const APP: &str = "ai-team.app";

/// The version this binary was built as.
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Which of the two programs a release ships is asking to be replaced.
///
/// Declared by the caller rather than guessed at, because guessing is what broke: `apply`
/// used to install `ait` over whatever binary was running, and the window's Update button
/// runs inside the desktop app. Pressing it put the CLI inside `ai-team.app`, so the app
/// launched, printed `--help` where nobody could see it, and exited - it simply stopped
/// opening, and nothing said why.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Host {
    /// The `ait` binary. Replaced file for file.
    #[default]
    Cli,
    /// The desktop app, which on macOS is a bundle and elsewhere is somebody else's
    /// package.
    Desktop,
}

/// What an update replaces on disk, and therefore which half of the archive to install.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Replace {
    /// One file.
    File(PathBuf),
    /// A whole `.app`.
    ///
    /// Whole, because the code signature seals `Info.plist` and `Resources/` together
    /// with the executable: replacing only the executable leaves a bundle `codesign`
    /// reports as not signed at all, which is a second way to arrive at an app that will
    /// not open.
    Bundle(PathBuf),
}

impl Replace {
    /// The path being replaced - and so the directory the download unpacks beside, since
    /// the final step is a rename and a rename cannot cross a filesystem.
    fn path(&self) -> &Path {
        match self {
            Replace::File(path) | Replace::Bundle(path) => path,
        }
    }
}

/// Decide what to replace, or refuse.
fn plan(host: Host, binary: &Path) -> Result<Replace> {
    match host {
        Host::Cli => Ok(Replace::File(binary.to_path_buf())),
        Host::Desktop => bundle_of(binary).map(Replace::Bundle).ok_or_else(|| {
            Error::invalid(format!(
                "{} is not inside a .app bundle, and a release archive carries no other \
                 kind of desktop app - update it the way you installed it (the .deb, the \
                 AppImage, or the Windows installer)",
                binary.display()
            ))
        }),
    }
}

/// The `.app` a binary is the executable of, recognised by shape rather than by name.
///
/// `<name>.app/Contents/MacOS/<exe>` is the only layout macOS will launch, and the
/// executable's name is whatever the product is called - so the shape is the part worth
/// matching. Deliberately not behind a `cfg`: macOS is the target but Linux is the
/// machine this is built on (D12), and a rule only one CI leg can reach is a rule that
/// rots unseen.
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

/// How this copy of ai-team got here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// A prebuilt archive from a GitHub release.
    Release,
    /// Built locally with cargo.
    Source,
    /// No marker: installed by hand, by a package manager, or from a clone before the
    /// marker existed. Reported rather than guessed at - replacing a binary somebody
    /// else's tooling manages is not ai-team's call.
    Unknown,
}

/// Read the marker `install.sh` leaves behind.
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

/// What an update would do.
#[derive(Debug, Clone, Serialize)]
pub struct Available {
    pub current: String,
    pub latest: Option<String>,
    pub method: Method,
    /// True only when there is a newer version *and* ai-team knows how to install it.
    pub can_update: bool,
    /// Why not, when it cannot.
    pub blocked: Option<String>,
}

/// Ask GitHub what the latest release is.
///
/// A failure here is not an error worth interrupting anybody for - the machine may be
/// offline, or on a network that blocks it - so the caller gets `None` and says "could
/// not check" rather than "you are up to date", which would be a claim it cannot make.
pub(crate) async fn latest() -> Option<String> {
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "10",
            &format!("https://api.github.com/repos/{REPO}/releases/latest"),
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
    let body = String::from_utf8_lossy(&output.stdout);
    let value: serde_json::Value = serde_json::from_str(&body).ok()?;
    let tag = value.get("tag_name")?.as_str()?;
    Some(tag.trim_start_matches('v').to_string())
}

/// Compare two dotted versions numerically.
///
/// String comparison says `0.10.0` is older than `0.9.0`, which would offer an update
/// that goes backwards and then refuse the real one forever after.
pub fn is_newer(candidate: &str, than: &str) -> bool {
    let parts = |text: &str| -> Vec<u64> {
        text.split(['.', '-'])
            .map(|part| part.parse().unwrap_or(0))
            .collect()
    };
    let (a, b) = (parts(candidate), parts(than));
    for index in 0..a.len().max(b.len()) {
        let left = a.get(index).copied().unwrap_or(0);
        let right = b.get(index).copied().unwrap_or(0);
        if left != right {
            return left > right;
        }
    }
    false
}

/// Whether an update is available, and whether it can be applied.
pub async fn check() -> Available {
    let current = current_version().to_string();
    let method = method();
    let latest = latest().await;

    let newer = latest
        .as_deref()
        .is_some_and(|latest| is_newer(latest, &current));

    let blocked = match method {
        Method::Unknown => Some(
            "ai-team was not installed by its own installer, so it will not replace the \
             binary - update it the way you installed it"
                .to_string(),
        ),
        _ if latest.is_none() => Some("could not reach GitHub to check".to_string()),
        _ => None,
    };

    Available {
        current,
        latest,
        method,
        can_update: newer && blocked.is_none(),
        blocked,
    }
}

/// The archive name a release publishes for this platform.
///
/// macOS ships one universal binary; Linux ships per-architecture. Anything else has no
/// prebuilt archive, which is a real answer rather than a failure - `--from-source` is
/// the path there.
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

/// How far along an update is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    Checking,
    Downloading,
    Verifying,
    Replacing,
    Done,
}

/// Download, verify and swap in a new binary.
///
/// The swap is a rename, not a write. A running executable cannot be written over on
/// Unix - the kernel refuses with ETXTBSY - but it can be *replaced*, because renaming
/// over a path leaves the running process attached to the old inode and gives every later
/// exec the new one. That is why the temporary file is put beside the target rather than
/// in `/tmp`: rename cannot cross a filesystem boundary, and `/tmp` is very often its own.
pub async fn apply<F>(version: &str, binary: &Path, host: Host, mut on_step: F) -> Result<()>
where
    F: FnMut(Step),
{
    if method() == Method::Unknown {
        return Err(Error::invalid(
            "ai-team was not installed by its own installer - update it the way you installed it",
        ));
    }

    let asset = asset_for(version).ok_or_else(|| {
        Error::invalid(format!(
            "no prebuilt release for {} {} - reinstall with --from-source",
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
    })?;
    let base = format!("https://github.com/{REPO}/releases/download/v{version}");

    // Worked out before anything is downloaded, so a desktop app this updater cannot
    // replace is told so immediately rather than after a 40MB download.
    let plan = plan(host, binary)?;

    // Unpacked beside what is being replaced rather than in /tmp. The swap at the end is
    // a rename, rename cannot cross a filesystem boundary, and /tmp very often is one -
    // so putting the work there would turn the atomic step into a copy that can
    // half-finish. Beside the *bundle* for a desktop update, not beside its executable,
    // which would put the scratch directory inside the thing being replaced.
    let work = Scratch::beside(plan.path())?;
    let archive = work.path().join(&asset);

    on_step(Step::Downloading);
    curl(&format!("{base}/{asset}"), &archive).await?;

    on_step(Step::Verifying);
    // Best effort, and deliberately so: a release that predates published checksums must
    // still be installable. What is not acceptable is a checksum that exists and differs,
    // which stops here.
    let sums = work.path().join("checksums.txt");
    if curl(&format!("{base}/checksums.txt"), &sums).await.is_ok() {
        verify(&archive, &sums, &asset)?;
    }

    on_step(Step::Replacing);
    let status = Command::new("tar")
        .args(["xzf"])
        .arg(&archive)
        .arg("-C")
        .arg(work.path())
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .status()
        .await
        .map_err(|error| Error::invalid(format!("could not run tar: {error}")))?;
    if !status.success() {
        return Err(Error::invalid("the downloaded archive would not unpack"));
    }

    install(&plan, work.path())?;

    on_step(Step::Done);
    Ok(())
}

/// Put the unpacked archive where it belongs.
///
/// Split out from [`apply`] so the choice between the two programs can be tested without
/// a network. Choosing wrong is what bricked a desktop install, and a release is a bad
/// place to find that out.
fn install(plan: &Replace, unpacked: &Path) -> Result<()> {
    match plan {
        Replace::File(target) => {
            let fresh = unpacked.join(CLI);
            if !fresh.is_file() {
                return Err(Error::invalid("the archive contained no `ait`"));
            }
            swap(&fresh, target)
        }
        Replace::Bundle(target) => {
            let fresh = unpacked.join(APP);
            if !fresh.is_dir() {
                return Err(Error::invalid(format!(
                    "this release publishes no {APP} - update the desktop app the way you \
                     installed it"
                )));
            }
            swap_bundle(&fresh, target)
        }
    }
}

/// A directory that cleans itself up, next to the binary being replaced.
///
/// A handful of lines instead of a runtime dependency on `tempfile`, and it puts the work
/// where the rename needs it rather than wherever `TMPDIR` points.
struct Scratch(PathBuf);

impl Scratch {
    fn beside(binary: &Path) -> Result<Scratch> {
        let dir = binary
            .parent()
            .ok_or_else(|| Error::invalid("the binary has no directory"))?
            .join(".ait-update");
        // Removed first: a previous run that was killed mid-download would otherwise
        // leave a half-unpacked archive for this one to trip over.
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).map_err(|error| {
            Error::invalid(format!(
                "could not write next to {} - update ai-team the way you installed it ({error})",
                binary.display()
            ))
        })?;
        Ok(Scratch(dir))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Put `fresh` where `target` is, atomically.
fn swap(fresh: &Path, target: &Path) -> Result<()> {
    let dir = target
        .parent()
        .ok_or_else(|| Error::invalid("the binary has no directory"))?;

    // Beside the target, because rename cannot cross filesystems and /tmp usually is one.
    let staged = dir.join(".ait.update");
    std::fs::copy(fresh, &staged).map_err(|error| {
        Error::invalid(format!(
            "could not write to {} - update ai-team the way you installed it ({error})",
            dir.display()
        ))
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755));
    }

    std::fs::rename(&staged, target).map_err(|error| {
        let _ = std::fs::remove_file(&staged);
        Error::invalid(format!("could not replace {}: {error}", target.display()))
    })
}

/// Put a whole `.app` where `target` is.
///
/// `rename` will not replace a directory that has anything in it, so the installed bundle
/// is moved aside first and removed only once the new one is in place. The move-aside is
/// what makes it recoverable: if the second rename fails the old app goes back, because
/// an update that removes the app and installs nothing is worse than one that does
/// nothing at all.
fn swap_bundle(fresh: &Path, target: &Path) -> Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| Error::invalid("the app has no directory"))?;
    let aside = parent.join(".ai-team.app.previous");
    let _ = std::fs::remove_dir_all(&aside);

    let installed = target.exists();
    if installed {
        std::fs::rename(target, &aside).map_err(|error| {
            Error::invalid(format!(
                "could not replace {} - update ai-team the way you installed it ({error})",
                target.display()
            ))
        })?;
    }

    if let Err(error) = std::fs::rename(fresh, target) {
        if installed {
            let _ = std::fs::rename(&aside, target);
        }
        return Err(Error::invalid(format!(
            "could not replace {}: {error}",
            target.display()
        )));
    }

    let _ = std::fs::remove_dir_all(&aside);
    Ok(())
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

/// Check one file against a `sha256sum`-format list.
fn verify(archive: &Path, sums: &Path, name: &str) -> Result<()> {
    let listed = std::fs::read_to_string(sums).unwrap_or_default();
    let Some(expected) = listed
        .lines()
        .find(|line| line.trim_end().ends_with(name))
        .and_then(|line| line.split_whitespace().next())
    else {
        return Ok(());
    };

    let bytes = std::fs::read(archive)
        .map_err(|error| Error::invalid(format!("could not read the download: {error}")))?;
    let actual = sha256(&bytes);
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(Error::invalid(
            "the download does not match its published checksum - not installing it",
        ))
    }
}

/// SHA-256, so a checksum can be checked without a dependency or a shell-out.
///
/// Written out rather than pulled in: it is sixty lines, it has fixed test vectors, and
/// the alternative is depending on a crate to verify the integrity of a download - which
/// is a supply chain question answered by adding to the supply chain.
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

/// One 64-byte block, mixed into the running digest.
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
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_the_published_vectors() {
        // Written out rather than depended on, so it is checked against the standard's
        // own answers rather than against itself.
        assert_eq!(
            sha256(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        // Long enough to span several blocks, which is where a padding mistake shows.
        assert_eq!(
            sha256(&b"a".repeat(1_000_000)),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
    }

    /// An unpacked release archive: the CLI and the app, side by side.
    fn unpacked(at: &Path) {
        std::fs::create_dir_all(at.join(APP).join("Contents/MacOS")).unwrap();
        std::fs::write(at.join(CLI), b"the cli").unwrap();
        std::fs::write(at.join(APP).join("Contents/MacOS/ai-team"), b"the app").unwrap();
        std::fs::write(at.join(APP).join("Contents/Info.plist"), b"fresh").unwrap();
    }

    /// An installed `.app`, one version behind. Returns its executable - the path a
    /// desktop process sees as `current_exe`.
    fn installed_app(at: &Path) -> PathBuf {
        let app = at.join(APP);
        std::fs::create_dir_all(app.join("Contents/MacOS")).unwrap();
        std::fs::write(app.join("Contents/MacOS/ai-team"), b"the old app").unwrap();
        std::fs::write(app.join("Contents/Info.plist"), b"stale").unwrap();
        app.join("Contents/MacOS/ai-team")
    }

    #[test]
    fn the_desktop_app_is_never_replaced_with_the_cli() {
        // The bug this whole split exists for. A release archive carries `ait` and the
        // app together, and `install` used to take `ait` whatever it was replacing - so
        // pressing Update in the window wrote the CLI into `ai-team.app`. The app then
        // launched, printed `--help` where nobody could see it, and exited: it stopped
        // opening, with nothing on screen to say why.
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("unpacked");
        std::fs::create_dir_all(&archive).unwrap();
        unpacked(&archive);

        let applications = dir.path().join("Applications");
        std::fs::create_dir_all(&applications).unwrap();
        let running = installed_app(&applications);

        let plan = plan(Host::Desktop, &running).unwrap();
        assert_eq!(plan, Replace::Bundle(applications.join(APP)));

        install(&plan, &archive).unwrap();

        assert_eq!(
            std::fs::read(&running).unwrap(),
            b"the app",
            "the app must be replaced with the app, never with the CLI"
        );
        // The whole bundle, not only its executable: the signature seals Info.plist and
        // Resources with it, so a half-replaced bundle is one Gatekeeper refuses.
        assert_eq!(
            std::fs::read(applications.join(APP).join("Contents/Info.plist")).unwrap(),
            b"fresh"
        );
        // And nothing is left lying next to it.
        assert!(!applications.join(".ai-team.app.previous").exists());
    }

    #[test]
    fn the_cli_still_gets_the_cli() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("unpacked");
        std::fs::create_dir_all(&archive).unwrap();
        unpacked(&archive);

        let target = dir.path().join("bin/ait");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, b"the old cli").unwrap();

        let plan = plan(Host::Cli, &target).unwrap();
        assert_eq!(plan, Replace::File(target.clone()));
        install(&plan, &archive).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"the cli");
    }

    #[test]
    fn a_bundle_is_recognised_by_its_shape_not_its_name() {
        // Runs on both CI legs on purpose (D12): macOS is the target, Linux is where this
        // is built, and path logic only one leg can reach is logic nobody checks.
        assert_eq!(
            bundle_of(Path::new(
                "/Applications/ai-team.app/Contents/MacOS/ai-team"
            )),
            Some(PathBuf::from("/Applications/ai-team.app"))
        );
        // The executable is named for the product, so any name inside the shape counts.
        assert_eq!(
            bundle_of(Path::new(
                "/Applications/Whatever.app/Contents/MacOS/anything"
            )),
            Some(PathBuf::from("/Applications/Whatever.app"))
        );
        // And nothing else does.
        assert_eq!(bundle_of(Path::new("/usr/local/bin/ait")), None);
        assert_eq!(
            bundle_of(Path::new("/Applications/ai-team.app/ai-team")),
            None
        );
        assert_eq!(
            bundle_of(Path::new("/Applications/ai-team/Contents/MacOS/ai-team")),
            None
        );
    }

    #[test]
    fn a_desktop_app_that_is_not_a_bundle_says_how_to_update_it() {
        // The Linux and Windows desktop builds are a .deb, an AppImage and an installer,
        // and the tarball has nothing in it for any of them. Installing `ait` over one
        // would brick it exactly the way it bricked the .app.
        let error = plan(Host::Desktop, Path::new("/usr/bin/ai-team"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("the way you installed it"), "{error}");
    }

    #[test]
    fn an_archive_with_no_app_in_it_refuses_rather_than_improvises() {
        // Fail closed: the Linux tarball carries only `ait`, and the one thing that must
        // not happen is it being installed over a desktop app anyway.
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("unpacked");
        std::fs::create_dir_all(&archive).unwrap();
        std::fs::write(archive.join(CLI), b"the cli").unwrap();

        let applications = dir.path().join("Applications");
        std::fs::create_dir_all(&applications).unwrap();
        let running = installed_app(&applications);

        let error = install(&plan(Host::Desktop, &running).unwrap(), &archive)
            .unwrap_err()
            .to_string();
        assert!(error.contains("no ai-team.app"), "{error}");
        assert_eq!(std::fs::read(&running).unwrap(), b"the old app");
    }

    #[test]
    fn a_bundle_that_cannot_be_replaced_is_put_back() {
        // There is nothing to move in, so the second rename fails - and the app that was
        // there has to come back. An update that removes the app and installs nothing is
        // worse than one that does nothing.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(APP);
        std::fs::create_dir_all(target.join("Contents")).unwrap();
        std::fs::write(target.join("Contents/Info.plist"), b"installed").unwrap();

        let error = swap_bundle(&dir.path().join("nothing"), &target)
            .unwrap_err()
            .to_string();
        assert!(error.contains("could not replace"), "{error}");

        assert_eq!(
            std::fs::read(target.join("Contents/Info.plist")).unwrap(),
            b"installed",
            "a failed update must leave the app that was working where it was"
        );
        assert!(!dir.path().join(".ai-team.app.previous").exists());
    }

    #[test]
    fn versions_compare_numerically_not_alphabetically() {
        // `0.10.0` < `0.9.0` as strings, which would offer an update that goes backwards
        // and then refuse the real one forever after.
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(is_newer("1.0.0", "0.99.99"));
        assert!(is_newer("0.2.1", "0.2.0"));
        assert!(!is_newer("0.2.0", "0.2.0"));
        assert!(!is_newer("0.1.9", "0.2.0"));
    }

    #[test]
    fn a_shorter_version_is_not_automatically_older() {
        assert!(!is_newer("1.0", "1.0.0"));
        assert!(is_newer("1.1", "1.0.9"));
    }

    #[test]
    fn a_checksum_that_differs_stops_the_update() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("thing.tar.gz");
        std::fs::write(&archive, b"abc").unwrap();

        let sums = dir.path().join("checksums.txt");
        std::fs::write(&sums, format!("{}  thing.tar.gz\n", sha256(b"abc"))).unwrap();
        assert!(verify(&archive, &sums, "thing.tar.gz").is_ok());

        std::fs::write(&sums, "0000  thing.tar.gz\n").unwrap();
        let error = verify(&archive, &sums, "thing.tar.gz")
            .unwrap_err()
            .to_string();
        assert!(error.contains("checksum"), "{error}");
    }

    #[test]
    fn a_release_with_no_checksums_is_still_installable() {
        // Best effort on purpose: a release published before checksums existed must not
        // become uninstallable. A checksum that exists and differs is the thing that must
        // stop it, and that is tested above.
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("thing.tar.gz");
        std::fs::write(&archive, b"abc").unwrap();
        let sums = dir.path().join("checksums.txt");
        std::fs::write(&sums, "deadbeef  something-else.tar.gz\n").unwrap();
        assert!(verify(&archive, &sums, "thing.tar.gz").is_ok());
    }

    #[test]
    fn the_asset_name_matches_what_the_release_publishes() {
        // These strings are a contract with release.yml and install.sh; getting one wrong
        // is a 404 at the moment somebody tries to update.
        let name = asset_for("0.2.0");
        match std::env::consts::OS {
            "macos" => assert_eq!(
                name.as_deref(),
                Some("ai-team-v0.2.0-macos-universal.tar.gz")
            ),
            "linux" => assert!(
                name.as_deref()
                    .is_some_and(|name| name.starts_with("ai-team-v0.2.0-linux-")),
                "{name:?}"
            ),
            _ => assert!(name.is_none()),
        }
    }

    #[cfg(unix)]
    #[test]
    fn swapping_replaces_the_target_and_keeps_it_executable() {
        use std::os::unix::fs::PermissionsExt as _;

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("ait");
        std::fs::write(&target, b"old").unwrap();
        let fresh = dir.path().join("fresh");
        std::fs::write(&fresh, b"new").unwrap();

        swap(&fresh, &target).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        let mode = std::fs::metadata(&target).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o111,
            0o111,
            "an update that is not executable is a brick"
        );

        // And nothing is left behind next to it.
        assert!(!dir.path().join(".ait.update").exists());
    }

    /// The test below, named so it can be run as a child of itself.
    const SLEEPER: &str = "update::tests::sleeps_when_a_parent_test_asks_it_to";
    /// Where that child reports that it is up. Its presence is also what asks it to sleep.
    const SLEEPER_MARKER: &str = "AI_TEAM_TEST_SLEEPER_MARKER";

    #[test]
    fn sleeps_when_a_parent_test_asks_it_to() {
        // Not a test of anything: a body for the test below to run as a child process, so
        // it has something of its own to copy and execute. A normal run returns here
        // immediately.
        let Some(marker) = std::env::var_os(SLEEPER_MARKER) else {
            return;
        };
        // Report the exec, rather than leaving the parent to guess at it with a sleep.
        std::fs::write(marker, b"up").unwrap();
        std::thread::sleep(std::time::Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn a_binary_can_be_replaced_while_it_is_running() {
        // The reason `swap` renames instead of writing. The kernel refuses to write over
        // an executing file - ETXTBSY, "Text file busy" - but it will happily replace the
        // *name*, leaving the running process attached to the old inode. That is also why
        // an update asks for a restart: the swap succeeds and changes nothing about the
        // process already up.
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("runner");

        // This test's own binary, not a copy of `/bin/sh`. macOS 26 SIGKILLs a platform
        // binary that has been copied out of the system - `/bin/sh` is arm64e and signed
        // as `com.apple.sh` - so that spelling passed on GitHub's runner and died
        // instantly on a current Mac, which is the wrong way round for the platform that
        // matters (D12). A binary this workspace built has no such restriction.
        std::fs::copy(std::env::current_exe().unwrap(), &target).unwrap();

        let marker = dir.path().join("up");
        let mut child = std::process::Command::new(&target)
            .args([SLEEPER, "--exact"])
            .env(SLEEPER_MARKER, &marker)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("the copy should be runnable");

        // `spawn` returns once the fork is under way, which is not the same moment the
        // kernel has the text segment mapped. Waiting for the child to say so beats a
        // fixed sleep, which fails under load rather than on merit.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while !marker.exists() {
            assert!(
                std::time::Instant::now() < deadline,
                "the child never started"
            );
            assert!(
                child.try_wait().unwrap().is_none(),
                "the child exited instead of running"
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        // Linux refuses to write over an executing file - ETXTBSY - and macOS does not:
        // the text-segment protection is a Linux one. Which means a naive updater that
        // writes in place passes on the Mac and fails on the machine this is built on,
        // the opposite of the usual direction (D12). The rename below is correct on both,
        // so it is what ships; this only records the difference.
        #[cfg(target_os = "linux")]
        assert!(
            std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&target)
                .is_err(),
            "Linux should refuse to write an executing file"
        );

        let fresh = dir.path().join("fresh");
        std::fs::write(&fresh, b"#!/bin/sh\ntrue\n").unwrap();
        swap(&fresh, &target).unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"#!/bin/sh\ntrue\n");
        // And the process that was already running is untouched.
        assert!(
            child.try_wait().unwrap().is_none(),
            "replacing the file must not kill the running process"
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    #[test]
    fn an_unwritable_destination_says_what_to_do_instead() {
        // The common case is a binary in /usr/local/bin owned by root. "permission
        // denied" alone leaves somebody with a half-finished update and no next step.
        let error = swap(
            Path::new("/nonexistent/fresh"),
            Path::new("/nonexistent/dir/ait"),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("the way you installed it"), "{error}");
    }
}
