pub mod api;
pub mod app;
pub mod cli;
pub mod commands;
pub mod cookies;
pub mod engine;
pub mod extensions;
pub mod fingerprint;
pub mod flow;
pub mod humanize;
pub mod lock;
pub mod menu;
pub mod network;
pub mod paths;
pub mod relay;
pub mod security;
pub mod store;
pub mod supervisor;
pub mod sync;
pub mod sys;

use lock::{DirLock, LockError};
use paths::Paths;
use std::sync::{Arc, Mutex, OnceLock};
use supervisor::{Event, EventSink};
use tauri::{Emitter, Manager, RunEvent, WindowEvent};
use tauri_plugin_log::{Target, TargetKind};

/// Мягкий лимит открытых файлов (у приложений, запущенных из Finder, он 256).
pub(crate) const FD_LIMIT: u64 = 4096;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let _ = sys::raise_fd_limit(FD_LIMIT);
    let paths = Paths::new(Paths::default_root());
    let handle: Arc<OnceLock<tauri::AppHandle>> = Arc::default();
    let sink_handle = handle.clone();
    let sink: EventSink = Arc::new(move |event: Event| {
        if let Some(app) = sink_handle.get() {
            let _ = app.emit(commands::event_channel(&event), event);
        }
    });
    let state = commands::AppState::new(paths.clone());
    let log_handle = handle.clone();
    let log_lock = Mutex::new(());
    let dispatch = fern::Dispatch::new().chain(fern::Output::call(move |record| {
        let Some(app) = log_handle.get() else { return };
        let Some(state) = app.try_state::<commands::AppState>() else { return };
        let Ok(core) = state.core() else { return };
        let _guard = log_lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let text = format!("[{}][{}][{}] {}\n", sys::unix_now(), record.level(), record.target(), record.args());
        let _ = security::append_log(&core.paths, text.as_bytes());
    }));
    let mut log_targets = vec![Target::new(TargetKind::Dispatch(dispatch))];
    if cfg!(debug_assertions) { log_targets.push(Target::new(TargetKind::Stdout)); }

    tauri::Builder::default()
        // Первым плагином, как требует single-instance: повторный запуск выводит окно первого экземпляра
        // на передний план и завершается, не дойдя до остальных плагинов и `setup`.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| commands::reveal_main_window(app)))
        .plugin(
            tauri_plugin_log::Builder::new()
                .clear_targets()
                .targets(log_targets)
                .level(log::LevelFilter::Info)
                // На уровне trace tungstenite пишет сообщения WebSocket целиком, а в ответах CDP — значения cookies.
                .level_for("tungstenite", log::LevelFilter::Info)
                .level_for("tokio_tungstenite", log::LevelFilter::Info)
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .manage(state)
        .setup(move |app| {
            let _ = handle.set(app.handle().clone());
            let state = app.state::<commands::AppState>();
            // Без настроенной защиты открываем обычное ядро; с защитой ждём подтверждения 2FA.
            match DirLock::acquire(paths.root()) {
                Ok(lock) => {
                    state.hold_instance(lock);
                    state.initialize_security(sink);
                }
                Err(LockError::Busy) => {
                    // Второй рубеж: плагин не застал первый экземпляр, а папку держит другой процесс Ombra.
                    log::warn!("Ombra уже запущен с папкой данных {}", paths.root().display());
                    app.handle().exit(0);
                    return Ok(());
                }
                Err(e) => {
                    // Без блокировки второй экземпляр открыл бы ту же базу: ядро не открывается, интерфейс покажет ошибку.
                    let e = app::CommandError::new(
                        "Io",
                        format!("не удалось заблокировать папку данных {}: {e}", paths.root().display()),
                    );
                    log::error!("{} ({})", e.message, e.code);
                    state.set_core(Err(e));
                }
            }
            let language = security::boot_language(&paths);
            menu::install(app.handle(), &language)?;
            if state.core().is_ok() { commands::activate_core(app.handle()); }
            Ok(())
        })
        .on_menu_event(|app, event| {
            if event.id() == menu::QUIT && !commands::handle_quit_request(app) {
                app.exit(0);
            }
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event
                && commands::handle_quit_request(window.app_handle())
            {
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_announcements,
            commands::open_announcement,
            commands::security_status,
            commands::security_begin_setup,
            commands::security_verify_setup,
            commands::security_cancel_setup,
            commands::security_finish_setup,
            commands::security_unlock,
            commands::security_change_password,
            commands::security_replace_authenticator,
            commands::app_info,
            commands::list_profiles,
            commands::list_profile_labels,
            commands::save_profile_label,
            commands::delete_profile_label,
            commands::set_profile_label,
            commands::get_profile,
            commands::reset_persona,
            commands::totp_code,
            commands::create_profile,
            commands::update_profile,
            commands::delete_profile,
            commands::start_profile,
            commands::stop_profile,
            commands::list_groups,
            commands::create_group,
            commands::update_group,
            commands::delete_group,
            commands::start_group,
            commands::list_tags,
            commands::change_profile_tags,
            commands::clone_profiles,
            commands::create_profiles_bulk,
            commands::start_profiles,
            commands::arrange_windows,
            commands::sync_state,
            commands::sync_start,
            commands::sync_stop,
            commands::sync_release,
            commands::sync_resume,
            commands::sync_open_like_main,
            commands::show_window,
            commands::get_sync_settings,
            commands::set_sync_settings,
            commands::traffic_summary,
            commands::connection_log,
            commands::list_extensions,
            commands::add_store_extension,
            commands::open_store_search,
            commands::open_store_page,
            commands::add_file_extension,
            commands::update_extension,
            commands::remove_extension,
            commands::list_ext_categories,
            commands::create_ext_category,
            commands::update_ext_category,
            commands::set_ext_category_items,
            commands::delete_ext_category,
            commands::set_profiles_ext_category,
            commands::set_group_ext_category,
            commands::stop_profiles,
            commands::delete_profiles,
            commands::list_flows,
            commands::list_tasks,
            commands::list_batches,
            commands::list_runs,
            commands::run_journal,
            commands::delete_task,
            commands::update_task_settings,
            commands::get_task,
            commands::update_task,
            commands::preview_table_data,
            commands::create_task,
            commands::start_task,
            commands::stop_task,
            commands::debug_flow,
            commands::test_node,
            commands::stop_debug,
            commands::cancel_waiting,
            commands::preview_table,
            commands::open_batch_files,
            commands::import_flow_file,
            commands::import_flow_json,
            commands::export_flow_file,
            commands::copy_flow,
            commands::delete_flow,
            commands::ops_catalog,
            commands::get_flow,
            commands::validate_flow,
            commands::save_flow,
            commands::set_flow_defaults,
            commands::list_trash,
            commands::restore_profiles,
            commands::purge_profiles,
            commands::empty_trash,
            commands::move_profiles,
            commands::edit_profiles,
            commands::export_cookies_to,
            commands::export_profile,
            commands::import_profile,
            commands::export_csv_file,
            commands::import_csv_file,
            commands::list_proxies,
            commands::clear_cache,
            commands::create_proxy,
            commands::update_proxy,
            commands::delete_proxy,
            commands::set_proxy_enabled,
            commands::import_proxies,
            commands::check_proxy,
            commands::check_connection,
            commands::cookie_stats,
            commands::import_cookies,
            commands::clear_cookies,
            commands::profile_settings,
            commands::set_on_quit,
            commands::get_services_proxy,
            commands::set_services_proxy,
            commands::set_google_key,
            commands::clear_google_key,
            commands::google_key_email,
            commands::api_config,
            commands::set_api_config,
            commands::regenerate_api_key,
            commands::new_fingerprint,
            commands::regenerate_fingerprint,
            commands::validate_fingerprint,
            commands::fingerprint_options,
            commands::import_engine,
            commands::engine_catalog,
            commands::select_engine,
            commands::set_engine_automatic,
            commands::update_engine,
            commands::cancel_engine_update,
            commands::set_language,
            commands::reveal_data_dir,
            commands::confirm_quit,
            commands::ui_ready,
        ])
        .build(tauri::generate_context!())
        .expect("приложение Tauri не собралось")
        .run(|app, event| {
            // Закрытие последнего окна и Cmd+Q при работающих профилях — по настройке «При закрытии приложения»:
            // вопрос или «Остановить все». Выход из Dock или системой приходит сразу как `RunEvent::Exit` и не
            // отменяется (в tao нет `applicationShouldTerminate:`), поэтому профили там останавливаются без вопроса.
            match &event {
                RunEvent::ExitRequested { code: None, api, .. } => {
                    if commands::handle_quit_request(app) {
                        api.prevent_exit();
                    }
                }
                // Local API закрывается, профили останавливаются до общего срока выхода — и без активных профилей.
                // Повторный запуск из Finder/Dock/Spotlight: macOS не создаёт процесс, а шлёт reopen.
                #[cfg(target_os = "macos")]
                RunEvent::Reopen { has_visible_windows: false, .. } => commands::reveal_main_window(app),
                RunEvent::Exit => commands::finish_exit(&app.state::<commands::AppState>()),
                _ => {}
            }
        });
}
