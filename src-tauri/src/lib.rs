use std::path::{Path, PathBuf};
#[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
use std::process::Stdio;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::time::Instant;
#[cfg(any(target_os = "windows", test))]
use std::time::SystemTime;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, Monitor, PhysicalPosition, Runtime};

mod account_registry;
mod browser;
mod claude_profiles;
mod codex_profiles;
mod commands;
mod local_usage;
mod models;
mod profile_store;
mod providers;
mod remediation;

const MAX_TRAY_LABEL_CHARS: usize = 8;
const STARTUP_SMOKE_ARGUMENT: &str = "--startup-smoke-marker=";
const STARTUP_SMOKE_STARTED_PREFIX: &str = "native-bridge-started:";
const STARTUP_SMOKE_READY_PREFIX: &str = "native-bridge-ready:";
const STARTUP_SMOKE_WATCHDOG_TIMEOUT: Duration = Duration::from_secs(25);
#[cfg(any(target_os = "macos", target_os = "linux"))]
const UPDATE_RELAUNCH_READY_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(any(target_os = "windows", test))]
const UPDATE_RELAUNCH_VISIBLE_MARKER: &str = "update-relaunch-visible-v1";
#[cfg(any(target_os = "windows", test))]
const UPDATE_RELAUNCH_VISIBLE_RECOVERY_MARKER: &str = "update-relaunch-visible-v1.tmp";
#[cfg(any(target_os = "windows", test))]
const UPDATE_RELAUNCH_VISIBLE_PREFIX: &str = "show-updated-app:";
#[cfg(any(target_os = "windows", test))]
const UPDATE_RELAUNCH_VISIBLE_MAX_AGE: Duration = Duration::from_secs(15 * 60);
#[cfg(any(target_os = "windows", test))]
const UPDATE_RELAUNCH_VISIBLE_MAX_BYTES: u64 = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StartupSmokePhase {
    Started,
    Ready,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StartupSmokeMarker {
    phase: StartupSmokePhase,
    process_id: u32,
}

struct StartupSmokeState {
    marker_path: Option<PathBuf>,
    ready: Arc<AtomicBool>,
}

fn startup_smoke_marker_from(arguments: impl IntoIterator<Item = String>) -> Option<PathBuf> {
    arguments.into_iter().find_map(|argument| {
        argument
            .strip_prefix(STARTUP_SMOKE_ARGUMENT)
            .filter(|path| !path.is_empty())
            .map(PathBuf::from)
    })
}

fn startup_smoke_content(phase: StartupSmokePhase, process_id: u32) -> String {
    let prefix = match phase {
        StartupSmokePhase::Started => STARTUP_SMOKE_STARTED_PREFIX,
        StartupSmokePhase::Ready => STARTUP_SMOKE_READY_PREFIX,
    };
    format!("{prefix}{process_id}\n")
}

fn parse_startup_smoke_marker(contents: &str) -> Option<StartupSmokeMarker> {
    let contents = contents.trim();
    let (phase, process_id) = contents
        .strip_prefix(STARTUP_SMOKE_STARTED_PREFIX)
        .map(|process_id| (StartupSmokePhase::Started, process_id))
        .or_else(|| {
            contents
                .strip_prefix(STARTUP_SMOKE_READY_PREFIX)
                .map(|process_id| (StartupSmokePhase::Ready, process_id))
        })?;
    let process_id = process_id
        .parse()
        .ok()
        .filter(|process_id| *process_id > 0)?;
    Some(StartupSmokeMarker { phase, process_id })
}

fn read_startup_smoke_marker(marker_path: &Path) -> Option<StartupSmokeMarker> {
    std::fs::read_to_string(marker_path)
        .ok()
        .and_then(|contents| parse_startup_smoke_marker(&contents))
}

fn write_startup_smoke_marker(marker_path: &Path, phase: StartupSmokePhase) -> std::io::Result<()> {
    // The launcher creates this file as a one-use lease. Opening it without
    // `create` prevents a replacement that starts after the old instance has
    // timed out (and removed the lease) from lingering as a duplicate. Unix
    // launchers also refuse a replaced symlink instead of truncating its target.
    let mut options = std::fs::OpenOptions::new();
    options.write(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let mut marker = options.open(marker_path)?;
    if !marker.metadata()?.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "startup marker lease is not a regular file",
        ));
    }
    let contents = startup_smoke_content(phase, std::process::id());
    std::io::Write::write_all(&mut marker, contents.as_bytes())
}

#[tauri::command]
fn frontend_ready(state: tauri::State<'_, StartupSmokeState>) -> Result<&'static str, String> {
    if let Some(marker_path) = &state.marker_path {
        write_startup_smoke_marker(marker_path, StartupSmokePhase::Ready).map_err(|error| {
            format!(
                "could not write the startup smoke marker at {}: {error}",
                marker_path.display()
            )
        })?;
    }
    state.ready.store(true, Ordering::Release);
    Ok("native-bridge-ready")
}

#[cfg(any(target_os = "windows", test))]
fn update_relaunch_visible_marker_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(UPDATE_RELAUNCH_VISIBLE_MARKER)
}

#[cfg(any(target_os = "windows", test))]
fn update_relaunch_visible_recovery_marker_path(app_data_dir: &Path) -> PathBuf {
    app_data_dir.join(UPDATE_RELAUNCH_VISIBLE_RECOVERY_MARKER)
}

#[cfg(any(target_os = "windows", test))]
fn valid_update_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 64
        && version
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
}

#[cfg(any(target_os = "windows", test))]
fn update_relaunch_visible_content(version: &str) -> String {
    format!("{UPDATE_RELAUNCH_VISIBLE_PREFIX}{version}")
}

#[cfg(any(target_os = "windows", test))]
fn write_update_relaunch_visible_marker(app_data_dir: &Path, version: &str) -> std::io::Result<()> {
    if !valid_update_version(version) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "update version is invalid",
        ));
    }
    std::fs::create_dir_all(app_data_dir)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".update-relaunch-visible-v1.tmp-")
        .tempfile_in(app_data_dir)?;
    std::io::Write::write_all(
        &mut temporary,
        update_relaunch_visible_content(version).as_bytes(),
    )?;
    temporary.as_file().sync_all()?;
    temporary
        .persist(update_relaunch_visible_marker_path(app_data_dir))
        .map_err(|error| error.error)?;

    // A successful runtime preparation supersedes any recovery file left by
    // an interrupted NSIS rename. The committed final marker is already valid,
    // so cleanup failure is harmless and must not turn preparation into a lie.
    let _ = std::fs::remove_file(update_relaunch_visible_recovery_marker_path(app_data_dir));
    Ok(())
}

#[cfg(any(target_os = "windows", test))]
fn remove_update_relaunch_visible_marker(app_data_dir: &Path) -> std::io::Result<()> {
    let mut first_error = None;
    for marker_path in [
        update_relaunch_visible_marker_path(app_data_dir),
        update_relaunch_visible_recovery_marker_path(app_data_dir),
    ] {
        match std::fs::remove_file(marker_path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                first_error.get_or_insert(error);
            }
        };
    }
    first_error.map_or(Ok(()), Err)
}

#[cfg(any(target_os = "windows", test))]
fn update_relaunch_marker_is_fresh(modified: SystemTime, now: SystemTime) -> bool {
    now.duration_since(modified)
        .is_ok_and(|age| age <= UPDATE_RELAUNCH_VISIBLE_MAX_AGE)
}

#[cfg(any(target_os = "windows", test))]
fn update_relaunch_marker_matches(marker_path: &Path, version: &str, now: SystemTime) -> bool {
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::fs::OpenOptionsExt as _;
        // Open the reparse point itself so a same-user symlink cannot redirect
        // this bounded visibility-only read to an unrelated file.
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }

    let Ok(mut marker) = options.open(marker_path) else {
        return false;
    };
    let Ok(metadata) = marker.metadata() else {
        return false;
    };
    if !metadata.is_file() || metadata.len() > UPDATE_RELAUNCH_VISIBLE_MAX_BYTES {
        return false;
    }
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    if !update_relaunch_marker_is_fresh(modified, now) {
        return false;
    }

    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    let mut bounded = std::io::Read::take(&mut marker, UPDATE_RELAUNCH_VISIBLE_MAX_BYTES + 1);
    if std::io::Read::read_to_end(&mut bounded, &mut bytes).is_err()
        || bytes.len() as u64 > UPDATE_RELAUNCH_VISIBLE_MAX_BYTES
    {
        return false;
    }
    bytes == update_relaunch_visible_content(version).as_bytes()
}

#[cfg(any(target_os = "windows", test))]
fn has_fresh_update_relaunch_visible_marker(
    app_data_dir: &Path,
    version: &str,
    now: SystemTime,
) -> bool {
    [
        update_relaunch_visible_marker_path(app_data_dir),
        update_relaunch_visible_recovery_marker_path(app_data_dir),
    ]
    .iter()
    .any(|marker_path| update_relaunch_marker_matches(marker_path, version, now))
}

#[cfg(any(target_os = "windows", test))]
fn update_relaunch_visibility_requested(
    app_data_dir: &Path,
    version: &str,
    now: SystemTime,
) -> bool {
    let requested = has_fresh_update_relaunch_visible_marker(app_data_dir, version, now);
    if !requested {
        // Invalid state is consumed immediately. Valid state remains until the
        // window reports that it was actually shown.
        let _ = remove_update_relaunch_visible_marker(app_data_dir);
    }
    requested
}

/// Before the Tauri updater hands a Windows update to NSIS/MSI, record that
/// the replacement must be shown even when the installer inherits `--hidden`
/// from a login-item launch. The Windows updater exits the process internally,
/// so this durable one-use marker is consumed by the replacement at startup.
#[tauri::command]
fn prepare_update_relaunch(app: tauri::AppHandle, version: String) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let app_data_dir = app
            .path()
            .app_data_dir()
            .map_err(|error| format!("EyeUrAI could not prepare the update restart: {error}"))?;
        write_update_relaunch_visible_marker(&app_data_dir, &version)
            .map_err(|error| format!("EyeUrAI could not prepare the update restart: {error}"))?;
    }
    #[cfg(not(target_os = "windows"))]
    let _ = (app, version);
    Ok(())
}

/// Remove the Windows visibility marker when download or installation fails
/// without terminating the current process.
#[tauri::command]
fn cancel_update_relaunch(app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        let app_data_dir = app
            .path()
            .app_data_dir()
            .map_err(|error| format!("EyeUrAI could not cancel the update restart: {error}"))?;
        remove_update_relaunch_visible_marker(&app_data_dir)
            .map_err(|error| format!("EyeUrAI could not cancel the update restart: {error}"))?;
    }
    #[cfg(not(target_os = "windows"))]
    let _ = app;
    Ok(())
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
async fn wait_for_replacement_ready(marker_path: &Path) -> bool {
    let deadline = Instant::now() + UPDATE_RELAUNCH_READY_TIMEOUT;
    loop {
        if read_startup_smoke_marker(marker_path)
            .is_some_and(|marker| marker.phase == StartupSmokePhase::Ready)
        {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(any(target_os = "macos", test))]
fn macos_app_bundle(executable_path: &Path) -> Option<PathBuf> {
    let macos_directory = executable_path.parent()?;
    if macos_directory.file_name()? != "MacOS" {
        return None;
    }
    let contents_directory = macos_directory.parent()?;
    if contents_directory.file_name()? != "Contents" {
        return None;
    }
    let app_bundle = contents_directory.parent()?;
    (app_bundle.extension()? == "app").then(|| app_bundle.to_path_buf())
}

/// Relaunch after an updater install without inheriting the old process state.
/// macOS uses LaunchServices; Linux directly spawns the updated executable.
/// Both keep the working process alive until the replacement frontend proves
/// it reached the native bridge. Windows normally exits inside the updater
/// plugin before this command can run, so its actual flow uses the durable
/// visibility marker prepared before installation; this branch is a fallback.
#[tauri::command]
async fn relaunch_after_update(app: tauri::AppHandle) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // Resolve this only when an update actually needs to relaunch. Tauri
        // deliberately rejects symlinked executable paths on macOS, and making
        // that relaunch-only check part of setup would prevent an otherwise
        // healthy app from starting from a symlinked development/install path.
        let executable_path = tauri::process::current_binary(&app.env()).map_err(|error| {
            format!("EyeUrAI could not locate its executable for the update relaunch: {error}")
        })?;
        let app_bundle = macos_app_bundle(&executable_path).ok_or_else(|| {
            "EyeUrAI could not locate its installed macOS application bundle.".to_string()
        })?;
        let marker = tempfile::NamedTempFile::new().map_err(|error| {
            format!("EyeUrAI could not prepare its update relaunch check: {error}")
        })?;
        let marker_path = marker.path().to_path_buf();
        let marker_argument = format!("{STARTUP_SMOKE_ARGUMENT}{}", marker_path.to_string_lossy());
        let status = tokio::process::Command::new("/usr/bin/open")
            .arg("-n")
            .arg(&app_bundle)
            .arg("--args")
            .arg(marker_argument)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .map_err(|error| format!("EyeUrAI could not ask macOS to reopen the app: {error}"))?;
        if !status.success() {
            return Err(format!(
                "macOS declined to reopen EyeUrAI after the update (status {status})."
            ));
        }

        // Do not close the working instance until the replacement's React app
        // has reached its own native bridge. This turns a silent relaunch miss
        // into a visible error while preserving a usable process.
        if !wait_for_replacement_ready(&marker_path).await {
            // Cancel the launch lease before offering a retry. A replacement
            // that started late observes the missing lease and exits, while a
            // process that has not reached setup can no longer claim it.
            drop(marker);
            tokio::time::sleep(Duration::from_millis(250)).await;
            return Err(
                "The update installed, but the replacement EyeUrAI app did not become ready. The current window will stay open."
                    .to_string(),
            );
        }
        app.exit(0);
        Ok(())
    }

    #[cfg(target_os = "linux")]
    {
        let executable_path = tauri::process::current_binary(&app.env()).map_err(|error| {
            format!("EyeUrAI could not locate its executable for the update relaunch: {error}")
        })?;
        let marker = tempfile::NamedTempFile::new().map_err(|error| {
            format!("EyeUrAI could not prepare its update relaunch check: {error}")
        })?;
        let marker_path = marker.path().to_path_buf();
        let marker_argument = format!("{STARTUP_SMOKE_ARGUMENT}{}", marker_path.to_string_lossy());
        let mut replacement = tokio::process::Command::new(executable_path)
            .arg(marker_argument)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("EyeUrAI could not restart after the update: {error}"))?;

        if !wait_for_replacement_ready(&marker_path).await {
            let _ = replacement.kill().await;
            return Err(
                "The update installed, but the replacement EyeUrAI app did not become ready. The current window will stay open."
                    .to_string(),
            );
        }
        app.exit(0);
        Ok(())
    }

    #[cfg(target_os = "windows")]
    {
        // The plugin's normal Windows update path exits before JavaScript can
        // invoke this command. Keep a clean-argument fallback for tests and for
        // any future plugin version that returns after installing.
        let executable_path = tauri::process::current_binary(&app.env()).map_err(|error| {
            format!("EyeUrAI could not locate its executable for the update relaunch: {error}")
        })?;
        tokio::process::Command::new(executable_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| format!("EyeUrAI could not restart after the update: {error}"))?;
        app.exit(0);
        Ok(())
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    Err("EyeUrAI update relaunch is available only on desktop platforms.".to_string())
}

fn sanitize_tray_label(raw_label: &str) -> Option<String> {
    let mut label = String::with_capacity(raw_label.len().min(MAX_TRAY_LABEL_CHARS));
    let mut char_count = 0;
    let mut separator_pending = false;

    for character in raw_label.trim().chars() {
        if character.is_alphanumeric() {
            if separator_pending && !label.is_empty() {
                if char_count + 1 >= MAX_TRAY_LABEL_CHARS {
                    break;
                }
                label.push(' ');
                char_count += 1;
            }

            if char_count >= MAX_TRAY_LABEL_CHARS {
                break;
            }
            label.push(character);
            char_count += 1;
            separator_pending = false;
        } else if !label.is_empty() {
            // A tray title is plain text, but delimiters and control characters
            // can still make it misleading or needlessly wide. Normalize every
            // non-alphanumeric run to at most one ordinary space.
            separator_pending = true;
        }
    }

    (!label.is_empty()).then_some(label)
}

fn format_tray_title(
    window_label: Option<&str>,
    percent_used: Option<f64>,
    reset_countdown: Option<&str>,
) -> Option<String> {
    let label = sanitize_tray_label(window_label?)?;
    if let Some(countdown) = reset_countdown.and_then(sanitize_tray_label) {
        return Some(format!("{label}:{countdown}"));
    }
    let usage = percent_used
        .filter(|value| value.is_finite())
        .map(|value| format!("{}%", value.clamp(0.0, 100.0).round() as u8))
        .unwrap_or_else(|| "—".to_string());
    Some(format!("{label}:{usage}"))
}

fn tray_image() -> tauri::image::Image<'static> {
    const SIZE: u32 = 22;
    let mut rgba = vec![0_u8; (SIZE * SIZE * 4) as usize];
    let center = (SIZE as f32 - 1.0) / 2.0;

    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = (x as f32 - center) / center;
            let dy = (y as f32 - center) / center;
            let outer = dx * dx / 0.82 + dy * dy / 0.36;
            let inner = dx * dx / 0.42 + dy * dy / 0.13;
            let pupil = dx * dx + dy * dy;
            let alpha = if (0.74..=1.08).contains(&outer)
                || (0.80..=1.16).contains(&inner)
                || pupil < 0.075
            {
                255
            } else {
                0
            };
            let offset = ((y * SIZE + x) * 4) as usize;
            rgba[offset] = 255;
            rgba[offset + 1] = 255;
            rgba[offset + 2] = 255;
            rgba[offset + 3] = alpha;
        }
    }

    tauri::image::Image::new_owned(rgba, SIZE, SIZE)
}

fn same_monitor(left: &Monitor, right: &Monitor) -> bool {
    left.position() == right.position() && left.size() == right.size()
}

fn squared_distance_to_monitor(point: PhysicalPosition<f64>, monitor: &Monitor) -> f64 {
    let position = monitor.position();
    let size = monitor.size();
    let minimum_x = f64::from(position.x);
    let maximum_x = minimum_x + f64::from(size.width);
    let minimum_y = f64::from(position.y);
    let maximum_y = minimum_y + f64::from(size.height);
    let nearest_x = point.x.clamp(minimum_x, maximum_x);
    let nearest_y = point.y.clamp(minimum_y, maximum_y);
    let delta_x = point.x - nearest_x;
    let delta_y = point.y - nearest_y;

    delta_x * delta_x + delta_y * delta_y
}

fn monitor_nearest_point<R: Runtime>(
    window: &tauri::WebviewWindow<R>,
    point: PhysicalPosition<f64>,
) -> Option<Monitor> {
    window
        .available_monitors()
        .ok()?
        .into_iter()
        .min_by(|left, right| {
            squared_distance_to_monitor(point, left)
                .total_cmp(&squared_distance_to_monitor(point, right))
        })
}

fn clamped_position(
    desired_x: f64,
    desired_y: f64,
    window_size: tauri::PhysicalSize<u32>,
    monitor: &Monitor,
) -> PhysicalPosition<i32> {
    let work_area = monitor.work_area();
    let minimum_x = f64::from(work_area.position.x) + 8.0;
    let maximum_x = (f64::from(work_area.position.x) + f64::from(work_area.size.width)
        - f64::from(window_size.width)
        - 8.0)
        .max(minimum_x);
    let minimum_y = f64::from(work_area.position.y) + 8.0;
    let maximum_y = (f64::from(work_area.position.y) + f64::from(work_area.size.height)
        - f64::from(window_size.height)
        - 8.0)
        .max(minimum_y);

    PhysicalPosition::new(
        desired_x.clamp(minimum_x, maximum_x).round() as i32,
        desired_y.clamp(minimum_y, maximum_y).round() as i32,
    )
}

fn center_on_monitor<R: Runtime>(window: &tauri::WebviewWindow<R>, monitor: &Monitor) {
    let Ok(window_size) = window.outer_size() else {
        return;
    };
    let work_area = monitor.work_area();
    let x = f64::from(work_area.position.x)
        + (f64::from(work_area.size.width) - f64::from(window_size.width)) / 2.0;
    let y = f64::from(work_area.position.y)
        + (f64::from(work_area.size.height) - f64::from(window_size.height)) / 2.0;
    let _ = window.set_position(clamped_position(x, y, window_size, monitor));
}

fn position_near_invocation<R: Runtime>(
    window: &tauri::WebviewWindow<R>,
    invocation_position: PhysicalPosition<f64>,
) {
    let Ok(window_size) = window.outer_size() else {
        return;
    };
    let exact_monitor = window
        .monitor_from_point(invocation_position.x, invocation_position.y)
        .ok()
        .flatten();
    let invocation_is_inside_monitor = exact_monitor.is_some();
    let monitor = exact_monitor
        .or_else(|| monitor_nearest_point(window, invocation_position))
        .or_else(|| window.current_monitor().ok().flatten())
        .or_else(|| window.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else {
        return;
    };

    let already_visible_here = window.is_visible().unwrap_or(false)
        && window
            .current_monitor()
            .ok()
            .flatten()
            .as_ref()
            .map(|current| same_monitor(current, &monitor))
            .unwrap_or(false);
    if already_visible_here {
        // Raising an already-visible utility window must not destroy the
        // position the user chose on this display.
        return;
    }

    // tao can report a cursor/menu-bar point outside every physical monitor on
    // mixed-DPI macOS setups (for example, Retina + a 1x external display).
    // The nearest monitor is still reliable, but the corrupted point is not a
    // useful anchor. Centering is preferable to stranding the window off-screen.
    if !invocation_is_inside_monitor {
        center_on_monitor(window, &monitor);
        return;
    }

    let monitor_position = monitor.position();
    let monitor_size = monitor.size();
    let bottom_half = invocation_position.y
        > f64::from(monitor_position.y) + f64::from(monitor_size.height) / 2.0;

    let x = invocation_position.x - f64::from(window_size.width) + 24.0;
    let desired_y = if bottom_half {
        invocation_position.y - f64::from(window_size.height) - 10.0
    } else {
        invocation_position.y + 28.0
    };
    let _ = window.set_position(clamped_position(x, desired_y, window_size, &monitor));
}

fn position_on_active_display<R: Runtime>(window: &tauri::WebviewWindow<R>) {
    let cursor = window.cursor_position().ok();
    let target = cursor
        .and_then(|point| {
            window
                .monitor_from_point(point.x, point.y)
                .ok()
                .flatten()
                .or_else(|| monitor_nearest_point(window, point))
        })
        .or_else(|| window.current_monitor().ok().flatten())
        .or_else(|| window.primary_monitor().ok().flatten());
    let Some(target) = target else {
        return;
    };

    let current = window.current_monitor().ok().flatten();
    if current
        .as_ref()
        .map(|monitor| !same_monitor(monitor, &target))
        .unwrap_or(true)
    {
        center_on_monitor(window, &target);
        return;
    }

    // A display can be rearranged while EyeUrAI is hidden. Preserve the user's
    // position when possible, but clamp every edge back into the active work area.
    if let (Ok(position), Ok(size)) = (window.outer_position(), window.outer_size()) {
        let clamped = clamped_position(f64::from(position.x), f64::from(position.y), size, &target);
        if clamped != position {
            let _ = window.set_position(clamped);
        }
    }
}

fn reveal_window<R: Runtime>(window: &tauri::WebviewWindow<R>) -> bool {
    let _ = window.unminimize();
    let shown = window.show().is_ok();
    let _ = window.set_focus();
    shown
}

fn summon_window<R: Runtime>(window: &tauri::WebviewWindow<R>) -> bool {
    position_on_active_display(window);
    reveal_window(window)
}

#[tauri::command]
fn set_tray_display(
    app: tauri::AppHandle,
    window_label: Option<String>,
    percent_used: Option<f64>,
    reset_countdown: Option<String>,
) -> Result<(), String> {
    let tray = app
        .tray_by_id("eyeurai-tray")
        .ok_or_else(|| "the EyeUrAI menu-bar item is unavailable".to_string())?;

    if let Some(title) = format_tray_title(
        window_label.as_deref(),
        percent_used,
        reset_countdown.as_deref(),
    ) {
        #[cfg(target_os = "macos")]
        {
            tray.set_icon(None).map_err(|error| error.to_string())?;
            tray.set_title(Some(&title))
                .map_err(|error| error.to_string())?;
        }
        #[cfg(not(target_os = "macos"))]
        {
            // Windows does not support tray titles. Keep the icon visible and
            // expose the pinned reading through its tooltip instead.
            tray.set_icon(Some(tray_image()))
                .map_err(|error| error.to_string())?;
            tray.set_icon_as_template(true)
                .map_err(|error| error.to_string())?;
        }
        tray.set_tooltip(Some(format!("EyeUrAI — {title}")))
            .map_err(|error| error.to_string())?;
    } else {
        // `set_title(None)` does not clear an existing macOS status-item title,
        // so restore with an explicit empty string before putting the mark back.
        #[cfg(target_os = "macos")]
        tray.set_title(Some(""))
            .map_err(|error| error.to_string())?;
        tray.set_icon(Some(tray_image()))
            .map_err(|error| error.to_string())?;
        tray.set_icon_as_template(true)
            .map_err(|error| error.to_string())?;
        tray.set_tooltip(Some("EyeUrAI — AI limits at a glance"))
            .map_err(|error| error.to_string())?;
    }

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let launched_hidden = std::env::args_os().any(|argument| argument == "--hidden");
    let startup_smoke_marker = startup_smoke_marker_from(std::env::args());
    let builder = tauri::Builder::default().plugin(tauri_plugin_notification::init());

    #[cfg(desktop)]
    let builder = builder
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--hidden"]),
        ))
        .plugin(tauri_plugin_updater::Builder::new().build());

    builder
        .invoke_handler(tauri::generate_handler![
            commands::get_snapshot,
            commands::refresh_quotas,
            commands::start_claude_account_login,
            commands::start_codex_account_login,
            commands::execute_remediation,
            local_usage::scan_local_usage,
            set_tray_display,
            frontend_ready,
            prepare_update_relaunch,
            cancel_update_relaunch,
            relaunch_after_update,
        ])
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let app_data_dir = app.path().app_data_dir().map_err(std::io::Error::other)?;
            #[cfg(target_os = "windows")]
            let show_after_update = update_relaunch_visibility_requested(
                &app_data_dir,
                &app.package_info().version.to_string(),
                SystemTime::now(),
            );
            #[cfg(target_os = "windows")]
            let update_marker_app_data_dir = app_data_dir.clone();
            #[cfg(not(target_os = "windows"))]
            let show_after_update = false;
            app.manage(commands::AppState::new(app_data_dir).map_err(std::io::Error::other)?);
            let startup_smoke_ready = Arc::new(AtomicBool::new(false));
            let startup_smoke_watchdog_marker = startup_smoke_marker.clone();
            if let Some(marker_path) = &startup_smoke_marker {
                // Claim the launcher's lease during native setup. Besides
                // publishing an exact child PID for smoke cleanup, this makes a
                // late replacement fail fast after its launcher removes the lease.
                write_startup_smoke_marker(marker_path, StartupSmokePhase::Started)?;
            }
            app.manage(StartupSmokeState {
                marker_path: startup_smoke_marker,
                ready: Arc::clone(&startup_smoke_ready),
            });

            if let Some(marker_path) = startup_smoke_watchdog_marker {
                let handle = app.handle().clone();
                tauri::async_runtime::spawn(async move {
                    let deadline = tokio::time::Instant::now() + STARTUP_SMOKE_WATCHDOG_TIMEOUT;
                    loop {
                        if startup_smoke_ready.load(Ordering::Acquire) {
                            return;
                        }
                        if !marker_path.exists() {
                            // Confirm cancellation after a short grace period so
                            // a launcher reading `ready:<pid>` cannot race the
                            // immediately following in-process ready flag store.
                            tokio::time::sleep(Duration::from_millis(100)).await;
                            if !startup_smoke_ready.load(Ordering::Acquire) && !marker_path.exists()
                            {
                                handle.exit(1);
                                return;
                            }
                        }
                        if tokio::time::Instant::now() >= deadline {
                            // A relaunch/smoke instance that never reaches the
                            // frontend bridge must not survive as a duplicate
                            // tray process after its launcher has given up.
                            handle.exit(1);
                            return;
                        }
                        tokio::time::sleep(Duration::from_millis(100)).await;
                    }
                });
            }

            let handle = app.handle().clone();
            let open_item = MenuItem::with_id(app, "open", "Open EyeUrAI", true, None::<&str>)?;
            let refresh_item =
                MenuItem::with_id(app, "refresh", "Refresh limits", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "Quit EyeUrAI", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open_item, &refresh_item, &quit_item])?;
            let tray = TrayIconBuilder::with_id("eyeurai-tray")
                .tooltip("EyeUrAI — AI limits at a glance")
                .icon(tray_image())
                .icon_as_template(true)
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => {
                        if let Some(window) = app.get_webview_window("main") {
                            summon_window(&window);
                        }
                    }
                    "refresh" => {
                        let _ = app.emit("eyeurai://refresh-requested", ());
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(move |_tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        position,
                        ..
                    } = event
                    {
                        if let Some(window) = handle.get_webview_window("main") {
                            // A tray/menu-bar invocation always means "show it
                            // here". Closing is an explicit native window action.
                            position_near_invocation(&window, position);
                            reveal_window(&window);
                        }
                    }
                })
                .build(app)?;

            app.manage(tray);

            if let Some(window) = app.get_webview_window("main") {
                let close_window = window.clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        let _ = close_window.hide();
                    }
                });
                if !launched_hidden || show_after_update {
                    #[cfg(target_os = "windows")]
                    {
                        let shown = summon_window(&window);
                        if show_after_update && shown {
                            let _ =
                                remove_update_relaunch_visible_marker(&update_marker_app_data_dir);
                        }
                    }
                    #[cfg(not(target_os = "windows"))]
                    summon_window(&window);
                }
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build EyeUrAI")
        .run(|_app, _event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = _event {
                if let Some(window) = _app.get_webview_window("main") {
                    summon_window(&window);
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::SystemTime;

    use super::{
        format_tray_title, has_fresh_update_relaunch_visible_marker, macos_app_bundle,
        parse_startup_smoke_marker, remove_update_relaunch_visible_marker, sanitize_tray_label,
        startup_smoke_content, startup_smoke_marker_from, update_relaunch_marker_is_fresh,
        update_relaunch_visibility_requested, update_relaunch_visible_content,
        update_relaunch_visible_marker_path, update_relaunch_visible_recovery_marker_path,
        write_startup_smoke_marker, write_update_relaunch_visible_marker, StartupSmokeMarker,
        StartupSmokePhase, UPDATE_RELAUNCH_VISIBLE_MAX_AGE, UPDATE_RELAUNCH_VISIBLE_MAX_BYTES,
    };

    #[test]
    fn startup_smoke_marker_requires_a_non_empty_explicit_argument() {
        assert_eq!(
            startup_smoke_marker_from([
                "eyeurai".to_string(),
                "--startup-smoke-marker=/tmp/ready.txt".to_string(),
            ]),
            Some(PathBuf::from("/tmp/ready.txt"))
        );
        assert_eq!(
            startup_smoke_marker_from([
                "eyeurai".to_string(),
                "--startup-smoke-marker=".to_string(),
            ]),
            None
        );
    }

    #[test]
    fn startup_smoke_marker_requires_an_existing_launcher_lease() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let marker_path = directory.path().join("ready.txt");

        let error = write_startup_smoke_marker(&marker_path, StartupSmokePhase::Ready)
            .expect_err("the app must not recreate an expired launcher lease");
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);

        std::fs::File::create(&marker_path).expect("create launcher lease");
        write_startup_smoke_marker(&marker_path, StartupSmokePhase::Ready)
            .expect("write active launcher lease");
        assert_eq!(
            parse_startup_smoke_marker(&std::fs::read_to_string(marker_path).expect("read marker")),
            Some(StartupSmokeMarker {
                phase: StartupSmokePhase::Ready,
                process_id: std::process::id(),
            })
        );
    }

    #[test]
    fn startup_smoke_marker_roundtrips_started_and_ready_states() {
        for phase in [StartupSmokePhase::Started, StartupSmokePhase::Ready] {
            assert_eq!(
                parse_startup_smoke_marker(&startup_smoke_content(phase, 42)),
                Some(StartupSmokeMarker {
                    phase,
                    process_id: 42,
                })
            );
        }
        assert_eq!(parse_startup_smoke_marker("native-bridge-ready:0"), None);
        assert_eq!(parse_startup_smoke_marker("native-bridge-ready:nope"), None);
    }

    #[cfg(unix)]
    #[test]
    fn startup_smoke_marker_refuses_a_replaced_symlink() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("temporary directory");
        let target = directory.path().join("must-not-change.txt");
        let marker_path = directory.path().join("ready.txt");
        std::fs::write(&target, "untouched").expect("write symlink target");
        symlink(&target, &marker_path).expect("replace lease with symlink");

        write_startup_smoke_marker(&marker_path, StartupSmokePhase::Ready)
            .expect_err("startup marker writes must not follow symlinks");
        assert_eq!(
            std::fs::read_to_string(target).expect("read symlink target"),
            "untouched"
        );
    }

    #[test]
    fn update_relaunch_visibility_marker_is_fresh_bounded_and_one_use() {
        let now = std::time::UNIX_EPOCH + std::time::Duration::from_secs(10_000);
        assert!(update_relaunch_marker_is_fresh(
            now - UPDATE_RELAUNCH_VISIBLE_MAX_AGE,
            now
        ));
        assert!(!update_relaunch_marker_is_fresh(
            now - UPDATE_RELAUNCH_VISIBLE_MAX_AGE - std::time::Duration::from_secs(1),
            now
        ));
        assert!(!update_relaunch_marker_is_fresh(
            now + std::time::Duration::from_secs(1),
            now
        ));

        let directory = tempfile::tempdir().expect("temporary directory");
        write_update_relaunch_visible_marker(directory.path(), "1.3.2")
            .expect("write visibility marker");
        assert!(has_fresh_update_relaunch_visible_marker(
            directory.path(),
            "1.3.2",
            SystemTime::now()
        ));
        assert!(!has_fresh_update_relaunch_visible_marker(
            directory.path(),
            "1.3.3",
            SystemTime::now()
        ));
        assert!(update_relaunch_visibility_requested(
            directory.path(),
            "1.3.2",
            SystemTime::now()
        ));
        assert!(update_relaunch_visible_marker_path(directory.path()).exists());
        remove_update_relaunch_visible_marker(directory.path()).expect("consume marker");
        assert!(!update_relaunch_visible_marker_path(directory.path()).exists());
        assert!(!has_fresh_update_relaunch_visible_marker(
            directory.path(),
            "1.3.2",
            SystemTime::now()
        ));

        // NSIS deliberately leaves its fully written temporary marker behind
        // if the final rename fails. The replacement must still recover the
        // visible launch and consume both names after window.show succeeds.
        std::fs::write(
            update_relaunch_visible_recovery_marker_path(directory.path()),
            update_relaunch_visible_content("1.3.2"),
        )
        .expect("write recovery marker");
        assert!(update_relaunch_visibility_requested(
            directory.path(),
            "1.3.2",
            SystemTime::now()
        ));
        remove_update_relaunch_visible_marker(directory.path()).expect("consume recovery marker");
        assert!(!update_relaunch_visible_recovery_marker_path(directory.path()).exists());

        write_update_relaunch_visible_marker(directory.path(), "1.3.3")
            .expect("write mismatched marker");
        assert!(!update_relaunch_visibility_requested(
            directory.path(),
            "1.3.2",
            SystemTime::now()
        ));
        assert!(!update_relaunch_visible_marker_path(directory.path()).exists());

        std::fs::write(
            update_relaunch_visible_marker_path(directory.path()),
            "malformed\n",
        )
        .expect("write malformed marker");
        assert!(!update_relaunch_visibility_requested(
            directory.path(),
            "1.3.2",
            SystemTime::now()
        ));
        assert!(!update_relaunch_visible_marker_path(directory.path()).exists());

        std::fs::write(
            update_relaunch_visible_marker_path(directory.path()),
            vec![b'x'; UPDATE_RELAUNCH_VISIBLE_MAX_BYTES as usize + 1],
        )
        .expect("write oversized marker");
        assert!(!update_relaunch_visibility_requested(
            directory.path(),
            "1.3.2",
            SystemTime::now()
        ));
        assert!(!update_relaunch_visible_marker_path(directory.path()).exists());
    }

    #[cfg(unix)]
    #[test]
    fn update_relaunch_visibility_marker_does_not_follow_a_symlink() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("temporary directory");
        let target = directory.path().join("unrelated.txt");
        std::fs::write(&target, update_relaunch_visible_content("1.3.2"))
            .expect("write unrelated target");
        symlink(
            &target,
            update_relaunch_visible_marker_path(directory.path()),
        )
        .expect("create marker symlink");

        assert!(!update_relaunch_visibility_requested(
            directory.path(),
            "1.3.2",
            SystemTime::now()
        ));
        assert_eq!(
            std::fs::read_to_string(target).expect("read unrelated target"),
            update_relaunch_visible_content("1.3.2")
        );
    }

    #[test]
    fn macos_app_bundle_accepts_only_an_executable_inside_an_app_bundle() {
        assert_eq!(
            macos_app_bundle(Path::new(
                "/Applications/EyeUrAI.app/Contents/MacOS/eyeurai"
            )),
            Some(PathBuf::from("/Applications/EyeUrAI.app"))
        );
        assert_eq!(
            macos_app_bundle(Path::new("/tmp/eyeurai")),
            None,
            "a bare binary must never become a LaunchServices target"
        );
        assert_eq!(
            macos_app_bundle(Path::new(
                "/Applications/EyeUrAI.bundle/Contents/MacOS/eyeurai"
            )),
            None,
            "only a .app bundle is launchable"
        );
    }

    #[test]
    fn tray_title_uses_the_selected_window_label() {
        assert_eq!(
            format_tray_title(Some("5h"), Some(61.6), None),
            Some("5h:62%".to_string())
        );
        assert_eq!(
            format_tray_title(Some("Wk"), Some(42.5), None),
            Some("Wk:43%".to_string())
        );
    }

    #[test]
    fn tray_title_clamps_finite_percentages() {
        assert_eq!(
            format_tray_title(Some("5h"), Some(-12.0), None),
            Some("5h:0%".to_string())
        );
        assert_eq!(
            format_tray_title(Some("Wk"), Some(140.0), None),
            Some("Wk:100%".to_string())
        );
    }

    #[test]
    fn tray_title_uses_a_dash_for_an_unavailable_percentage() {
        assert_eq!(
            format_tray_title(Some("Wk"), None, None),
            Some("Wk:—".to_string())
        );
        assert_eq!(
            format_tray_title(Some("Wk"), Some(f64::NAN), None),
            Some("Wk:—".to_string())
        );
        assert_eq!(
            format_tray_title(Some("Wk"), Some(f64::INFINITY), None),
            Some("Wk:—".to_string())
        );
    }

    #[test]
    fn tray_label_is_short_plain_text() {
        assert_eq!(
            sanitize_tray_label("  Wk:\n%🔥 all\u{7}  "),
            Some("Wk all".to_string())
        );
        assert_eq!(
            sanitize_tray_label("MonthlyLimit"),
            Some("MonthlyL".to_string())
        );
    }

    #[test]
    fn tray_title_can_show_a_reset_countdown() {
        assert_eq!(
            format_tray_title(Some("5h"), None, Some("10m")),
            Some("5h:10m".to_string())
        );
        assert_eq!(
            format_tray_title(Some("Wk"), Some(87.0), Some("2h 14m")),
            Some("Wk:2h 14m".to_string())
        );
    }

    #[test]
    fn absent_or_empty_label_selects_the_logo() {
        assert_eq!(format_tray_title(None, Some(62.0), None), None);
        assert_eq!(format_tray_title(Some(" \n:%🔥 "), Some(62.0), None), None);
    }
}
