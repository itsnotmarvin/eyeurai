use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, PhysicalPosition, Runtime};

mod account_registry;
mod commands;
mod local_usage;
mod models;
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

fn format_tray_title(window_label: Option<&str>, percent_used: Option<f64>) -> Option<String> {
    let label = sanitize_tray_label(window_label?)?;
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
            let alpha = if (0.74..=1.08).contains(&outer) || (0.80..=1.16).contains(&inner) {
                255
            } else if pupil < 0.075 {
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

fn position_popover<R: Runtime>(
    window: &tauri::WebviewWindow<R>,
    tray_position: PhysicalPosition<f64>,
) {
    let Ok(window_size) = window.outer_size() else {
        return;
    };
    let Ok(Some(monitor)) = window
        .current_monitor()
        .or_else(|_| window.primary_monitor())
    else {
        return;
    };
    let monitor_position = monitor.position();
    let monitor_size = monitor.size();
    let bottom_half =
        tray_position.y > f64::from(monitor_position.y) + f64::from(monitor_size.height) / 2.0;

    let x = (tray_position.x - f64::from(window_size.width) + 24.0)
        .max(f64::from(monitor_position.x) + 8.0);
    let y = if bottom_half {
        tray_position.y - f64::from(window_size.height) - 10.0
    } else {
        tray_position.y + 28.0
    };

    let _ = window.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
}

#[tauri::command]
fn set_tray_display(
    app: tauri::AppHandle,
    window_label: Option<String>,
    percent_used: Option<f64>,
) -> Result<(), String> {
    let tray = app
        .tray_by_id("eyeurai-tray")
        .ok_or_else(|| "the EyeUrAI menu-bar item is unavailable".to_string())?;

    if let Some(title) = format_tray_title(window_label.as_deref(), percent_used) {
        #[cfg(target_os = "macos")]
        tray.set_icon(None).map_err(|error| error.to_string())?;
        tray.set_title(Some(&title))
            .map_err(|error| error.to_string())?;
        tray.set_tooltip(Some(format!("EyeUrAI — {title}")))
            .map_err(|error| error.to_string())?;
    } else {
        // `set_title(None)` does not clear an existing macOS status-item title,
        // so restore with an explicit empty string before putting the mark back.
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
    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_store::Builder::new().build());

    #[cfg(desktop)]
    let builder = builder
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build());

    builder
        .invoke_handler(tauri::generate_handler![
            commands::get_snapshot,
            commands::refresh_quotas,
            commands::refresh_provider,
            commands::provider_capabilities,
            commands::get_demo_snapshot,
            local_usage::scan_local_usage,
            set_tray_display,
        ])
        .setup(|app| {
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
                            let _ = window.show();
                            let _ = window.set_focus();
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
                            if window.is_visible().unwrap_or(false) {
                                let _ = window.hide();
                            } else {
                                position_popover(&window, position);
                                let _ = window.show();
                                let _ = window.set_focus();
                            }
                        }
                    }
                })
                .build(app)?;

            app.manage(tray);

            if let Some(window) = app.get_webview_window("main") {
                let hide_window = window.clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::Focused(false) = event {
                        let _ = hide_window.hide();
                    }
                });
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("failed to run EyeUrAI");
}

#[cfg(test)]
mod tests {
    use super::{format_tray_title, sanitize_tray_label};

    #[test]
    fn tray_title_uses_the_selected_window_label() {
        assert_eq!(
            format_tray_title(Some("5h"), Some(61.6)),
            Some("5h:62%".to_string())
        );
        assert_eq!(
            format_tray_title(Some("Wk"), Some(42.5)),
            Some("Wk:43%".to_string())
        );
    }

    #[test]
    fn tray_title_clamps_finite_percentages() {
        assert_eq!(
            format_tray_title(Some("5h"), Some(-12.0)),
            Some("5h:0%".to_string())
        );
        assert_eq!(
            format_tray_title(Some("Wk"), Some(140.0)),
            Some("Wk:100%".to_string())
        );
    }

    #[test]
    fn tray_title_uses_a_dash_for_an_unavailable_percentage() {
        assert_eq!(
            format_tray_title(Some("Wk"), None),
            Some("Wk:—".to_string())
        );
        assert_eq!(
            format_tray_title(Some("Wk"), Some(f64::NAN)),
            Some("Wk:—".to_string())
        );
        assert_eq!(
            format_tray_title(Some("Wk"), Some(f64::INFINITY)),
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
    fn absent_or_empty_label_selects_the_logo() {
        assert_eq!(format_tray_title(None, Some(62.0)), None);
        assert_eq!(format_tray_title(Some(" \n:%🔥 "), Some(62.0)), None);
    }
}
