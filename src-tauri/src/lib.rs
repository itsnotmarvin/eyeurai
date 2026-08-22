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

const MAX_TRAY_LABEL_CHARS: usize = 8;

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

fn reveal_window<R: Runtime>(window: &tauri::WebviewWindow<R>) {
    let _ = window.unminimize();
    let _ = window.show();
    let _ = window.set_focus();
}

fn summon_window<R: Runtime>(window: &tauri::WebviewWindow<R>) {
    position_on_active_display(window);
    reveal_window(window);
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
    let builder = tauri::Builder::default().plugin(tauri_plugin_notification::init());

    #[cfg(desktop)]
    let builder = builder
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--hidden"]),
        ))
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build());

    builder
        .invoke_handler(tauri::generate_handler![
            commands::get_snapshot,
            commands::refresh_quotas,
            commands::start_claude_account_login,
            commands::start_codex_account_login,
            local_usage::scan_local_usage,
            set_tray_display,
        ])
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            let app_data_dir = app.path().app_data_dir().map_err(std::io::Error::other)?;
            app.manage(commands::AppState::new(app_data_dir).map_err(std::io::Error::other)?);

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
                if !launched_hidden {
                    summon_window(&window);
                }
            }

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build EyeUrAI")
        .run(|app, event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                if let Some(window) = app.get_webview_window("main") {
                    summon_window(&window);
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::{format_tray_title, sanitize_tray_label};

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
