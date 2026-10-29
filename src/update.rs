//! stats-lite updates itself, the way kil does. It reads `releases/latest.toml` on
//! `main`; when that names a newer version, it downloads the DMG beside it, checks
//! the checksum, the version, the Developer ID team and Apple's notarization, and
//! keeps the app from it until stats-lite quits. Quitting then replaces the running
//! bundle, so the next launch is the new version.
//!
//! A manual check shows its progress and outcome in `update_window`; automatic
//! checks stay silent. Everything blocking runs on one background thread; only the
//! system tools (`curl`, `codesign`, `spctl`, …) do the real work, so this file
//! needs no new dependencies.

use std::{
    ffi::OsStr,
    fs,
    io::Read as _,
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    sync::{Mutex, OnceLock},
};

/// Raw from `main`, where the release commit puts the DMG and this file together.
const MANIFEST_URL: &str =
    "https://raw.githubusercontent.com/timwuhaotian/stats-lite/main/releases/latest.toml";
const DMG_URL: &str =
    "https://github.com/timwuhaotian/stats-lite/raw/main/releases/StatsLite-arm64.dmg";

/// What a check is doing, for the menu item and update window to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Checking,
    /// The DMG's bytes so far, and its size once the server has said it.
    Downloading {
        version: String,
        got: u64,
        total: Option<u64>,
    },
    Verifying {
        version: String,
    },
}

/// The check under way, if any: written by its thread, read by the menu.
fn progress() -> &'static Mutex<Option<Step>> {
    static PROGRESS: OnceLock<Mutex<Option<Step>>> = OnceLock::new();
    PROGRESS.get_or_init(|| Mutex::new(None))
}

/// How a check ended, for a manual check's update window to report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ready(String),
    UpToDate,
    Failed(String),
}

/// A newer stats-lite, checked and waiting for stats-lite to quit.
#[derive(Clone, Debug)]
pub struct Ready {
    pub version: String,
    pub app: PathBuf,
    pub bundle: PathBuf,
}

#[derive(Debug, Default)]
struct State {
    outcome: Option<Outcome>,
    /// A manual check is owed its outcome in the update window.
    notify: bool,
    ready: Option<Ready>,
    /// Restart to Update: open the new app once this one has quit.
    reopen: bool,
}

fn state() -> &'static Mutex<State> {
    static STATE: OnceLock<Mutex<State>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(State::default()))
}

fn lock<T>(mutex: &'static Mutex<T>) -> std::sync::MutexGuard<'static, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

/// The menu item's title and whether it restarts (`true`) or checks (`false`).
pub fn menu_status() -> (String, bool) {
    if let Some(ready) = &lock(state()).ready {
        return (format!("Restart to Update to {}", ready.version), true);
    }
    let title = match &*lock(progress()) {
        None => String::from("Check for Updates…"),
        Some(Step::Checking) => String::from("Checking for Updates…"),
        Some(Step::Downloading { got, total, .. }) => match total {
            Some(total) if *total > 0 => {
                format!("Downloading {}%…", got.saturating_mul(100) / total)
            }
            _ => String::from("Downloading…"),
        },
        Some(Step::Verifying { .. }) => String::from("Verifying Update…"),
    };
    (title, false)
}

/// The current step, for an open update window to follow.
pub fn current_step() -> Option<Step> {
    lock(progress()).clone()
}

/// A finished manual check's outcome for the update window, if one is owed.
pub fn take_notified_outcome() -> Option<Outcome> {
    let mut state = lock(state());
    if state.notify {
        state.notify = false;
        state.outcome.take()
    } else {
        None
    }
}

/// Check for Updates…: a manual request also follows a check already under way.
pub fn check_now(manual: bool) {
    {
        let mut state = lock(state());
        if let Some(ready) = &state.ready {
            if manual {
                state.outcome = Some(Outcome::Ready(ready.version.clone()));
                state.notify = true;
            }
            return;
        }
        if lock(progress()).is_some() {
            state.notify |= manual;
            return;
        }
        state.outcome = None;
        state.notify = manual;
        *lock(progress()) = Some(Step::Checking);
    }
    std::thread::spawn(|| {
        let found = find(env!("CARGO_PKG_VERSION"));
        *lock(progress()) = None;
        let mut state = lock(state());
        match found {
            Ok(Some(ready)) => {
                state.outcome = Some(Outcome::Ready(ready.version.clone()));
                state.ready = Some(ready);
            }
            Ok(None) => state.outcome = Some(Outcome::UpToDate),
            Err(err) => state.outcome = Some(Outcome::Failed(err)),
        }
    });
}

/// Restart to Update: marks the staged app to open after quitting. `false` when
/// nothing is staged.
pub fn restart() -> bool {
    let mut state = lock(state());
    if state.ready.is_some() {
        state.reopen = true;
        true
    } else {
        false
    }
}

/// Installs the staged update, if one is still newer than what's on disk.
/// `Some` message means the quit must be aborted and the message shown.
pub fn install_on_quit() -> Option<String> {
    let (ready, reopen) = {
        let state = lock(state());
        let ready = state.ready.clone()?;
        (ready, state.reopen)
    };
    // The bundle may have been replaced by hand since the update was staged; a newer
    // app there stays, and the stale download goes.
    let on_disk = bundle_version(&ready.bundle).ok();
    let stale = on_disk
        .as_deref()
        .is_some_and(|on_disk| !is_newer(&ready.version, on_disk));
    if stale {
        clear_ready();
        return None;
    }
    if let Err(err) = install(&ready.app, &ready.bundle) {
        return Some(format!(
            "Couldn't install stats-lite {}: {err}",
            ready.version
        ));
    }
    if let Some(dir) = ready.app.parent() {
        let _ = fs::remove_dir_all(dir);
    }
    clear_ready();
    if reopen {
        // Once this process is gone, so the new app doesn't fight it over anything.
        let spawned = Command::new("/bin/sh")
            .args([
                "-c",
                r#"while kill -0 "$1" 2>/dev/null; do sleep 0.2; done; exec /usr/bin/open "$2""#,
                "sh",
            ])
            .arg(std::process::id().to_string())
            .arg(&ready.bundle)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Err(err) = spawned {
            return Some(format!("Updated, but reopening failed: {err}"));
        }
    }
    None
}

fn clear_ready() {
    let mut state = lock(state());
    state.ready = None;
    state.reopen = false;
}

/// `releases/latest.toml`: the release's version and its DMG's SHA-256.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    pub version: String,
    pub sha256: String,
}

impl Manifest {
    /// Parses the two-line manifest; anything else shaped is rejected.
    ///
    /// # Errors
    /// If `text` isn't exactly `version` and `sha256` string entries.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut version = None;
        let mut sha256 = None;
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .ok_or_else(|| format!("latest.toml line {}: no `=`", n + 1))?;
            let value = value
                .trim()
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'));
            match (key.trim(), value) {
                ("version", Some(value)) if version.is_none() => version = Some(value.to_owned()),
                ("sha256", Some(value)) if sha256.is_none() => sha256 = Some(value.to_owned()),
                _ => return Err(format!("latest.toml line {}: not a manifest entry", n + 1)),
            }
        }
        match (version, sha256) {
            (Some(version), Some(sha256)) => Ok(Self { version, sha256 }),
            _ => Err("latest.toml: needs `version` and `sha256`".to_owned()),
        }
    }
}

/// `major.minor.patch`, compared as numbers.
fn parse_version(text: &str) -> Option<[u64; 3]> {
    let mut parts = text.trim().split('.').map(|part| part.parse().ok());
    let version = [parts.next()??, parts.next()??, parts.next()??];
    parts.next().is_none().then_some(version)
}

/// Whether `candidate` is a later version than `current`; anything unreadable is not.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    matches!(
        (parse_version(candidate), parse_version(current)),
        (Some(candidate), Some(current)) if candidate > current
    )
}

/// The `.app` that `exe` runs in, if stats-lite may replace it: not a bare binary
/// (`cargo run`), not the copy inside the DMG, and not one Gatekeeper translocated
/// to a read-only place.
fn replaceable_bundle(exe: &Path) -> Result<PathBuf, &'static str> {
    let bundle = exe
        .parent()
        .filter(|dir| dir.ends_with("Contents/MacOS"))
        .and_then(|dir| dir.parent()?.parent())
        .filter(|bundle| bundle.extension().is_some_and(|ext| ext == "app"))
        .ok_or("stats-lite isn't running from StatsLite.app")?;
    let path = bundle.to_string_lossy();
    if path.starts_with("/Volumes/") || path.contains("/AppTranslocation/") {
        return Err("stats-lite is running from the disk image: drag it to Applications first");
    }
    Ok(bundle.to_owned())
}

/// Runs `program`; a failure is its stderr, else how it ended.
fn run(program: &str, args: &[&OsStr]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("{program}: {err}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned());
    }
    Err(failure(program, output.status, &output.stderr))
}

fn failure(program: &str, status: ExitStatus, stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let name = Path::new(program).file_name().unwrap_or_default().display();
    match stderr.trim() {
        "" => format!("{name}: {status}"),
        why => format!("{name}: {why}"),
    }
}

/// The body size the last response in a `curl -D` dump gives; redirects come before it.
fn content_length(headers: &str) -> Option<u64> {
    let last = headers
        .rsplit_once("\nHTTP/")
        .map_or(headers, |(_, last)| last);
    last.lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())?
        })
        .filter(|&size| size > 0)
}

/// Downloads `url` to `to`, publishing progress for the menu, by the file's size and
/// the headers curl writes before the body.
fn download(url: &str, to: &Path) -> Result<(), String> {
    const CURL: &str = "/usr/bin/curl";
    let headers = to.with_extension("headers");
    let mut curl = Command::new(CURL)
        .args(["-fsSL", "--max-time", "900", "-D"])
        .arg(&headers)
        .arg("-o")
        .arg(to)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| format!("{CURL}: {err}"))?;
    let report = || {
        if let Some(Step::Downloading { got, total, .. }) = lock(progress()).as_mut() {
            *got = fs::metadata(to).map_or(0, |file| file.len());
            *total = fs::read_to_string(&headers)
                .ok()
                .and_then(|headers| content_length(&headers));
        }
    };
    let status = loop {
        report();
        match curl.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(super::UPDATE_POLL),
            Err(err) => return Err(format!("{CURL}: {err}")),
        }
    };
    report();
    let _ = fs::remove_file(&headers);
    if status.success() {
        return Ok(());
    }
    let mut stderr = Vec::new();
    if let Some(mut pipe) = curl.stderr.take() {
        let _ = pipe.read_to_end(&mut stderr);
    }
    Err(failure(CURL, status, &stderr))
}

/// The Developer ID team that signed `app`; `None` for an ad-hoc signature (a local build).
fn team_of(app: &Path) -> Option<String> {
    let output = Command::new("/usr/bin/codesign")
        .args(["-dv", "--verbose=2"])
        .arg(app)
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stderr)
        .lines()
        .find_map(|line| line.strip_prefix("TeamIdentifier="))
        .filter(|team| *team != "not set")
        .map(str::to_owned)
}

/// The version `app`'s Info.plist shows.
fn bundle_version(app: &Path) -> Result<String, String> {
    let plist = app.join("Contents/Info.plist");
    run(
        "/usr/libexec/PlistBuddy",
        &[
            "-c".as_ref(),
            "Print :CFBundleShortVersionString".as_ref(),
            plist.as_os_str(),
        ],
    )
}

/// Checks that `app` is stats-lite `version`, signed by `team` and notarized by Apple.
fn verify(app: &Path, version: &str, team: &str) -> Result<(), String> {
    let shown = bundle_version(app)?;
    if shown != version {
        return Err(format!("the download is stats-lite {shown}, not {version}"));
    }
    let requirement =
        format!("=anchor apple generic and certificate leaf[subject.OU] = \"{team}\"");
    run(
        "/usr/bin/codesign",
        &[
            "--verify".as_ref(),
            "--deep".as_ref(),
            "--strict".as_ref(),
            "-R".as_ref(),
            requirement.as_ref(),
            app.as_os_str(),
        ],
    )
    .map_err(|err| format!("the download isn't signed as stats-lite is ({err})"))?;
    run(
        "/usr/sbin/spctl",
        &[
            "--assess".as_ref(),
            "--type".as_ref(),
            "execute".as_ref(),
            app.as_os_str(),
        ],
    )
    .map_err(|err| format!("Apple hasn't notarized the download ({err})"))?;
    Ok(())
}

/// Reads the manifest at `url`.
///
/// # Errors
/// If it can't be fetched or parsed.
fn fetch_manifest(url: &str) -> Result<Manifest, String> {
    let text = run(
        "/usr/bin/curl",
        &[
            "-fsSL".as_ref(),
            "--max-time".as_ref(),
            "30".as_ref(),
            url.as_ref(),
        ],
    )?;
    Manifest::parse(&text)
}

/// Downloads the DMG at `dmg_url` into `dir`, checks it against `manifest` and `team`,
/// and returns the app copied out of it. An app staged earlier for the same version
/// is reused.
///
/// # Errors
/// If the download fails or any check does.
fn stage(manifest: &Manifest, dmg_url: &str, dir: &Path, team: &str) -> Result<PathBuf, String> {
    let version = &manifest.version;
    let verifying = || {
        *lock(progress()) = Some(Step::Verifying {
            version: version.clone(),
        })
    };
    let app = dir.join("StatsLite.app");
    if app.exists() {
        verifying();
        if verify(&app, version, team).is_ok() {
            return Ok(app);
        }
    }
    let _ = fs::remove_dir_all(dir);
    fs::create_dir_all(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    let dmg = dir.join("StatsLite.dmg");
    *lock(progress()) = Some(Step::Downloading {
        version: version.clone(),
        got: 0,
        total: None,
    });
    download(dmg_url, &dmg)?;
    verifying();
    let sum = run(
        "/usr/bin/shasum",
        &["-a".as_ref(), "256".as_ref(), dmg.as_os_str()],
    )?;
    if !sum
        .split_whitespace()
        .next()
        .is_some_and(|sum| sum.eq_ignore_ascii_case(&manifest.sha256))
    {
        return Err("the download doesn't match its checksum".to_owned());
    }
    let mount = dir.join("mnt");
    run(
        "/usr/bin/hdiutil",
        &[
            "attach".as_ref(),
            "-nobrowse".as_ref(),
            "-readonly".as_ref(),
            "-noautoopen".as_ref(),
            "-quiet".as_ref(),
            "-mountpoint".as_ref(),
            mount.as_os_str(),
            dmg.as_os_str(),
        ],
    )?;
    let copied = run(
        "/usr/bin/ditto",
        &[mount.join("StatsLite.app").as_os_str(), app.as_os_str()],
    );
    let detach = |force: bool| {
        let mut args = vec!["detach".as_ref(), "-quiet".as_ref(), mount.as_os_str()];
        if force {
            args.push("-force".as_ref());
        }
        run("/usr/bin/hdiutil", &args)
    };
    if detach(false).is_err() {
        let _ = detach(true);
    }
    copied?;
    let _ = fs::remove_file(&dmg);
    verify(&app, version, team).inspect_err(|_| {
        let _ = fs::remove_dir_all(&app);
    })?;
    Ok(app)
}

/// Replaces `bundle` with `staged`: copied in beside it, then two renames, so `bundle`
/// is never missing or half-written, and a failure leaves the old app in place. The
/// running app keeps the code it loaded.
///
/// # Errors
/// If the folder can't be written or a rename fails.
fn install(staged: &Path, bundle: &Path) -> Result<(), String> {
    let (Some(parent), Some(name)) = (bundle.parent(), bundle.file_name()) else {
        return Err(format!("{}: not an app", bundle.display()));
    };
    let name = name.to_string_lossy();
    let new = parent.join(format!(".{name}.new"));
    let old = parent.join(format!(".{name}.old"));
    for leftover in [&new, &old] {
        let _ = fs::remove_dir_all(leftover);
    }
    run("/usr/bin/ditto", &[staged.as_os_str(), new.as_os_str()])?;
    if let Err(err) = fs::rename(bundle, &old) {
        let _ = fs::remove_dir_all(&new);
        return Err(format!("{}: {err}", bundle.display()));
    }
    if let Err(err) = fs::rename(&new, bundle) {
        let _ = fs::rename(&old, bundle);
        let _ = fs::remove_dir_all(&new);
        return Err(format!("{}: {err}", bundle.display()));
    }
    let _ = fs::remove_dir_all(&old);
    Ok(())
}

/// Whether stats-lite can create files in `dir`, which replacing the bundle needs.
fn writable(dir: &Path) -> Result<(), String> {
    let probe = dir.join(format!(".statslite-update-{}", std::process::id()));
    fs::create_dir(&probe)
        .and_then(|()| fs::remove_dir(&probe))
        .map_err(|err| {
            format!(
                "stats-lite can't replace itself in {} ({err})",
                dir.display()
            )
        })
}

/// `~/Library/Caches/dev.statslite/update`.
fn cache_dir() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(|home| PathBuf::from(home).join("Library/Caches/dev.statslite/update"))
        .ok_or_else(|| "no home directory".to_owned())
}

/// One check, start to end; blocks, so it runs on a background thread.
/// `Ok(None)` is up to date.
fn find(current: &str) -> Result<Option<Ready>, String> {
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    let bundle = replaceable_bundle(&exe)?;
    let team = team_of(&bundle).ok_or(
        "this copy of stats-lite isn't signed with a Developer ID, so it doesn't update itself",
    )?;
    let manifest = fetch_manifest(MANIFEST_URL)?;
    if !is_newer(&manifest.version, current) {
        return Ok(None);
    }
    writable(bundle.parent().unwrap_or(&bundle))?;
    let app = stage(&manifest, DMG_URL, &cache_dir()?, &team)?;
    Ok(Some(Ready {
        version: manifest.version,
        app,
        bundle,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_newer_should_compare_numbers_not_text() {
        assert_eq!(
            [
                is_newer("0.2.0", "0.1.9"),
                is_newer("0.1.0", "0.0.99"),
                is_newer("0.0.5", "0.0.5"),
                is_newer("0.0.4", "0.0.5"),
                is_newer("0.0.x", "0.0.5"),
                is_newer("0.0.6.1", "0.0.5"),
            ],
            [true, true, false, false, false, false]
        );
    }

    #[test]
    fn manifest_should_parse_and_reject_anything_else_shaped() {
        assert_eq!(
            Manifest::parse("version = \"0.2.0\"\nsha256 = \"ab\"\n"),
            Ok(Manifest {
                version: "0.2.0".into(),
                sha256: "ab".into()
            })
        );
        assert!(Manifest::parse("version = \"0.2.0\"\nurl = \"x\"\n").is_err());
        assert!(Manifest::parse("version = \"0.2.0\"\n").is_err());
        assert!(Manifest::parse("version = 2\nsha256 = \"ab\"\n").is_err());
        assert!(
            Manifest::parse("version = \"0.2.0\"\nversion = \"0.2.1\"\nsha256 = \"ab\"\n").is_err()
        );
    }

    #[test]
    fn only_an_installed_bundle_should_be_replaceable() {
        let bundle = |exe: &str| replaceable_bundle(Path::new(exe)).ok();
        assert_eq!(
            [
                bundle("/Applications/StatsLite.app/Contents/MacOS/stats-lite"),
                bundle("/Users/me/code/stats-lite/target/release/stats-lite"),
                bundle("/Volumes/StatsLite/StatsLite.app/Contents/MacOS/stats-lite"),
                bundle(
                    "/private/var/folders/x/AppTranslocation/1/d/StatsLite.app/Contents/MacOS/stats-lite"
                ),
            ],
            [
                Some(PathBuf::from("/Applications/StatsLite.app")),
                None,
                None,
                None
            ]
        );
    }

    #[test]
    fn content_length_should_be_the_last_responses() {
        let redirected = "HTTP/2 302 \r\ncontent-length: 0\r\n\r\nHTTP/2 200 \r\nContent-Length: 4570610\r\n\r\n";
        assert_eq!(
            [
                content_length(redirected),
                content_length("HTTP/2 302 \r\ncontent-length: 0\r\n\r\n"),
                content_length("HTTP/2 302 \r\ncontent-length: 9\r\n\r\nHTTP/2 200 \r\n"),
                content_length("Content-Length: 94\r\nAccept-ranges: bytes\r\n"),
            ],
            [Some(4_570_610), None, None, Some(94)]
        );
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("statslite-update-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fake_app(at: &Path, version: &str) {
        fs::create_dir_all(at.join("Contents/MacOS")).unwrap();
        fs::write(at.join("Contents/MacOS/stats-lite"), version).unwrap();
    }

    #[test]
    fn install_should_swap_the_bundle_and_leave_nothing_beside_it() {
        let dir = scratch("install");
        let (bundle, staged) = (
            dir.join("Apps/StatsLite.app"),
            dir.join("cache/StatsLite.app"),
        );
        fake_app(&bundle, "old");
        fake_app(&staged, "new");
        install(&staged, &bundle).unwrap();
        let mut left: Vec<_> = fs::read_dir(dir.join("Apps"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        left.sort();
        assert_eq!(
            (
                fs::read_to_string(bundle.join("Contents/MacOS/stats-lite")).unwrap(),
                left
            ),
            (
                "new".to_owned(),
                vec![std::ffi::OsString::from("StatsLite.app")]
            )
        );
    }

    #[test]
    fn install_should_keep_the_old_app_when_the_new_one_is_missing() {
        let dir = scratch("missing");
        let bundle = dir.join("StatsLite.app");
        fake_app(&bundle, "old");
        assert!(install(&dir.join("nowhere/StatsLite.app"), &bundle).is_err());
        assert_eq!(
            fs::read_to_string(bundle.join("Contents/MacOS/stats-lite")).unwrap(),
            "old"
        );
    }

    /// The release on `main` end to end, from the repository instead of GitHub: the manifest
    /// matches the DMG, and the app in it passes every check an update makes.
    #[test]
    fn the_committed_release_should_pass_the_checks_an_update_makes() {
        let releases = Path::new(env!("CARGO_MANIFEST_DIR")).join("releases");
        let manifest =
            Manifest::parse(&fs::read_to_string(releases.join("latest.toml")).unwrap()).unwrap();
        let dmg = fs::canonicalize(releases.join("StatsLite-arm64.dmg")).unwrap();
        let dir = scratch("stage");
        let url = format!("file://{}", dmg.display());
        let staged = stage(&manifest, &url, &dir, "43VLF3KTFZ").unwrap();
        assert!(staged.join("Contents/MacOS/stats-lite").is_file());
        assert!(verify(&staged, &manifest.version, "AAAAAAAAAA").is_err());
        let _ = fs::remove_dir_all(&dir);
    }
}
