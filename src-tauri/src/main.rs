#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod backdrop;
mod cache;
mod detail;
mod engine;
mod model;
#[cfg(windows)]
mod network;
mod placement;
mod popover;
mod pricing;
mod quota;
mod quota_history;
mod session_metadata;
mod settings;
mod sources;

use std::{fs, path::PathBuf, sync::{Arc, Mutex}, time::Instant};

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, LogicalSize, Manager, PhysicalSize, Rect, RunEvent, State,
    WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

use detail::{Detail, Sort};
use engine::{current_span, CollectionStatus, Engine, Update};
use model::{Activity, Period, SessionPage, Summary};
use popover::Popover;
use quota::{QuotaCache, Quotas};
use quota::sampler::Sampler;
use quota_history::{HistoryPreview, QuotaKey};
use settings::{Language, Preferences, PreferencesStore};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

const WINDOW: &str = "details";
const DEFAULT_OPACITY: u8 = 88;
const MIN_OPACITY: u8 = 65;
const DEFAULT_FONT_SCALE: u8 = 100;
const FONT_SCALES: [u8; 3] = [DEFAULT_FONT_SCALE, 110, 120];
const DEFAULT_SIZE: LogicalSize<f64> = LogicalSize::new(432.0, 880.0);

#[derive(Default)]
struct WindowMemory
{
    view: Mutex<Value>,
    size: Mutex<Option<LogicalSize<f64>>>,
    backdrop: Mutex<Option<backdrop::Backdrop>>,
    appearance: Mutex<Appearance>,
    popover: Mutex<Popover>,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(default, rename_all = "camelCase")]
struct Appearance
{
    opacity: u8,
    font_scale: u8,
}

impl Default for Appearance
{
    fn default() -> Self
    {
        Self { opacity: DEFAULT_OPACITY, font_scale: DEFAULT_FONT_SCALE }
    }
}

fn app_data_root() -> PathBuf
{
    if let Some(path) = std::env::var_os("TOKEN_MONITOR_LITE_DATA_DIR").map(PathBuf::from).filter(|path| path.is_absolute())
    {
        return path;
    }
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("TokenMonitorLite")
}

fn show_details(app: &AppHandle, anchor: Option<Rect>) -> tauri::Result<()>
{
    if let Some(window) = app.get_webview_window(WINDOW)
    {
        if window.is_visible()? { window.set_focus()?; }
        return Ok(());
    }
    let memory = app.state::<WindowMemory>();
    memory.popover.lock().expect("popover").opening();
    let icon = anchor.or_else(|| app.tray_by_id("usage").and_then(|tray| tray.rect().ok().flatten()));
    let preferred = memory.size.lock().expect("window size").unwrap_or(DEFAULT_SIZE);
    let placement = placement::for_tray(app, icon, preferred);
    let size = placement.as_ref().map_or(preferred, |target| target.logical_size);
    let window = WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::App("index.html".into()))
        // DWM can paint the native title behind the transparent WebView2 header.
        .title("")
        .inner_size(size.width, size.height)
        .min_inner_size(330.0_f64.min(size.width), 370.0_f64.min(size.height))
        .decorations(false)
        .shadow(false)
        .skip_taskbar(true)
        .theme(Some(tauri::Theme::Dark))
        .transparent(true)
        .background_color(tauri::window::Color(0, 0, 0, 0))
        .visible(false)
        .focused(false)
        .data_directory(app_data_root().join("WebView2"))
        .on_navigation(|url| {
            url.scheme() == "tauri" || url.host_str() == Some("tauri.localhost") ||
                (cfg!(debug_assertions) && url.host_str() == Some("127.0.0.1"))
        })
        .build()?;
    if let Some(target) = placement
    {
        // Move to the tray's monitor before applying the size in that monitor's DPI.
        window.set_position(target.position)?;
        window.set_size(target.size)?;
        window.set_position(target.position)?;
    }
    let material = backdrop::apply(&window);
    *memory.backdrop.lock().expect("window material") = Some(material);
    Ok(())
}

#[tauri::command]
fn details_ready(window: WebviewWindow) -> tauri::Result<()>
{
    // The transparent host stays hidden while WebView2 loads the panel.
    if !window.is_visible()?
    {
        window.show()?;
        window.set_focus()?;
    }
    Ok(())
}

fn open(app: &AppHandle, anchor: Option<Rect>)
{
    if let Err(error) = show_details(app, anchor)
    {
        eprintln!("Unable to open details: {error}");
        if let Some(tray) = app.tray_by_id("usage")
        {
            let language = app.state::<Arc<PreferencesStore>>().get().language.resolve();
            let message = if language == Language::Chinese { "Token Monitor Lite · 无法打开 WebView2" }
                else { "Token Monitor Lite · Could not open WebView2" };
            let _ = tray.set_tooltip(Some(message));
        }
    }
}

fn remember_size(app: &AppHandle)
{
    if let Some(window) = app.get_webview_window(WINDOW)
    {
        if !window.is_minimized().unwrap_or(false)
        {
            if let (Ok(scale), Ok(size)) = (window.scale_factor(), window.inner_size())
            {
                *app.state::<WindowMemory>().size.lock().expect("window size") = Some(size.to_logical(scale));
            }
        }
    }
}

#[tauri::command]
fn dismiss_window(window: WebviewWindow) -> Result<(), String>
{
    close_details(window.app_handle()).map_err(|error| error.to_string())
}

fn close_details(app: &AppHandle) -> tauri::Result<()>
{
    let Some(window) = app.get_webview_window(WINDOW) else { return Ok(()) };
    app.state::<WindowMemory>().popover.lock().expect("popover").closing();
    remember_size(app);
    window.destroy()
}

#[tauri::command]
fn get_view_state(
    memory: State<'_, WindowMemory>, quotas: State<'_, Arc<QuotaCache>>, store: State<'_, Arc<PreferencesStore>>,
) -> Value
{
    let preferences = store.get();
    json!({
        "view": memory.view.lock().expect("view state").clone(),
        "backdrop": memory.backdrop.lock().expect("window material").unwrap_or_default(),
        "appearance": *memory.appearance.lock().expect("appearance"),
        "preferences": preferences,
        "systemLanguage": Language::System.resolve(),
        "cachedQuotas": quotas.cached(preferences.quotas.tools),
    })
}

#[tauri::command]
fn set_preferences(
    app: AppHandle, store: State<'_, Arc<PreferencesStore>>, engine: State<'_, Arc<Engine>>,
    preferences: Preferences, period: Period,
) -> Result<(), String>
{
    let previous = store.save(preferences)?;
    if previous.quotas != preferences.quotas || previous.collection.tools != preferences.collection.tools
    {
        app.state::<Sampler>().settings_changed();
    }
    if previous.collection != preferences.collection
    {
        engine.collection_changed(period, previous.collection.tools != preferences.collection.tools);
    }
    if previous.language != preferences.language
    {
        if let Some(tray) = app.tray_by_id("usage")
        {
            let chinese = preferences.language.resolve() == Language::Chinese;
            if let Ok(menu) = tray_menu(&app, chinese) { let _ = tray.set_menu(Some(menu)); }
            let summary = engine.summary(Period::Today).unwrap_or_default();
            let _ = tray.set_tooltip(Some(tooltip(&summary, chinese)));
        }
    }
    Ok(())
}

#[tauri::command]
async fn get_collection_status(engine: State<'_, Arc<Engine>>) -> Result<CollectionStatus, String>
{
    let engine = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || engine.collection_status()
        .map_err(|_| "Could not read collection status.".to_string())).await.map_err(|error| error.to_string())?
}

#[tauri::command]
fn remember_view(memory: State<'_, WindowMemory>, view: Value)
{
    *memory.view.lock().expect("view state") = view;
}

fn decode_appearance(bytes: &[u8]) -> Appearance
{
    // Version 0.1.1 stored only a JSON number for opacity.
    let mut stored = serde_json::from_slice::<Appearance>(bytes)
        .or_else(|_| serde_json::from_slice::<u8>(bytes).map(|opacity| Appearance { opacity, ..Appearance::default() }))
        .unwrap_or_default();
    if !(MIN_OPACITY..=100).contains(&stored.opacity) { stored.opacity = DEFAULT_OPACITY; }
    if !FONT_SCALES.contains(&stored.font_scale) { stored.font_scale = DEFAULT_FONT_SCALE; }
    stored
}

#[tauri::command]
fn set_appearance(memory: State<'_, WindowMemory>, appearance: Appearance) -> Result<(), String>
{
    if !(MIN_OPACITY..=100).contains(&appearance.opacity)
        || !FONT_SCALES.contains(&appearance.font_scale)
    {
        return Err("Invalid appearance settings.".into());
    }
    let root = app_data_root();
    let temporary = root.join("appearance.tmp");
    let encoded = serde_json::to_vec(&appearance).map_err(|error| error.to_string())?;
    fs::write(&temporary, encoded).and_then(|_| fs::rename(&temporary, root.join("appearance.json")))
        .map_err(|_| "Could not save appearance.".to_string())?;
    *memory.appearance.lock().expect("appearance") = appearance;
    Ok(())
}

#[tauri::command]
async fn get_summary(engine: State<'_, Arc<Engine>>, period: Period) -> Result<Summary, String>
{
    let engine = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut result = engine.summary(period).map_err(|_| "Could not read the usage summary.".to_string())?;
        if engine.tracked_tools().antigravity
        {
            (result.antigravity_local, result.antigravity_pb_only) = engine.sources.antigravity_status();
        }
        Ok(result)
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command]
async fn get_activity(engine: State<'_, Arc<Engine>>, force: bool) -> Result<Activity, String>
{
    let engine = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || engine.activity(force))
        .await.map_err(|error| error.to_string())?
}

#[tauri::command]
async fn get_quotas(
    engine: State<'_, Arc<Engine>>, quotas: State<'_, Arc<QuotaCache>>, store: State<'_, Arc<PreferencesStore>>, force: bool,
) -> Result<Quotas, String>
{
    let sources = engine.sources.clone();
    let quotas = quotas.inner().clone();
    let settings = store.get().quotas;
    tauri::async_runtime::spawn_blocking(move || quotas.read(&sources, settings, force)).await.map_err(|error| error.to_string())
}

#[tauri::command]
fn record_banked_reset_panel(
    quotas: State<'_, Arc<QuotaCache>>, limits_view: bool, codex_enabled: bool, read_failed: bool, count: Option<u64>, rendered: bool,
)
{
    quotas.record_banked_reset_panel(limits_view, codex_enabled, read_failed, count, rendered);
}

#[tauri::command]
async fn reset_quota_forecast(
    quotas: State<'_, Arc<QuotaCache>>, store: State<'_, Arc<PreferencesStore>>, key: QuotaKey,
) -> Result<Quotas, String>
{
    let tools = store.get().quotas.tools;
    let quotas = quotas.inner().clone();
    tauri::async_runtime::spawn_blocking(move || quotas.reset_forecast(&key, tools)).await.map_err(|error| error.to_string())?
}

#[tauri::command]
async fn get_quota_history(
    quotas: State<'_, Arc<QuotaCache>>, store: State<'_, Arc<PreferencesStore>>, key: QuotaKey,
) -> Result<HistoryPreview, String>
{
    if !store.get().quotas.tools.contains(key.tool()) { return Err("This quota is disabled.".into()); }
    let quotas = quotas.inner().clone();
    tauri::async_runtime::spawn_blocking(move || quotas.history(&key)).await.map_err(|error| error.to_string())?
}

#[tauri::command]
async fn list_sessions(
    engine: State<'_, Arc<Engine>>, period: Period, tool: Option<String>, query: String, page: u32, preview: Option<bool>,
) -> Result<SessionPage, String>
{
    let engine = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        engine.sessions(period, tool.as_deref(), &query, page, preview.unwrap_or(false))
            .map_err(|_| "Could not list the sessions.".into())
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command]
async fn get_session_detail(
    engine: State<'_, Arc<Engine>>, period: Period, tool: String, session_id: String, sort: Sort, page: u32,
) -> Result<Option<Detail>, String>
{
    let engine = engine.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let span = current_span(period);
        let session = engine.cache.session(period, &engine.scope_key(&span), &tool, &session_id)
            .map_err(|_| "Could not open the session.".to_string())?;
        Ok(session.map(|mut session| {
            session_metadata::enrich(&engine.sources, std::slice::from_mut(&mut session));
            detail::read(&engine.sources, session, &span, sort, page)
        }))
    }).await.map_err(|error| error.to_string())?
}

#[tauri::command]
fn refresh(engine: State<'_, Arc<Engine>>, period: Period)
{
    engine.refresh(period);
}

fn scanner_path() -> std::io::Result<PathBuf>
{
    let app = std::env::current_exe()?;
    let bundled = app.with_file_name("tokscale.exe");
    if !cfg!(debug_assertions) || bundled.is_file()
    {
        return Ok(bundled);
    }
    Ok(PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../vendor/tokscale.exe"))
}

fn tray_menu(app: &AppHandle, chinese: bool) -> tauri::Result<Menu<tauri::Wry>>
{
    let show = MenuItem::with_id(app, "open", if chinese { "打开" } else { "Open" }, true, None::<&str>)?;
    let update = MenuItem::with_id(app, "refresh", if chinese { "刷新" } else { "Refresh" }, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", if chinese { "退出" } else { "Exit" }, true, None::<&str>)?;
    Menu::with_items(app, &[&show, &update, &quit])
}

fn compact_tokens(tokens: u64) -> String
{
    let units = [(1_000, "K"), (1_000_000, "M"), (1_000_000_000, "B"), (1_000_000_000_000, "T")];
    let Some(mut index) = units.iter().rposition(|(divisor, _)| tokens >= *divisor)
        else { return tokens.to_string() };
    let scaled = |divisor: u64| (u128::from(tokens) * 10 + u128::from(divisor / 2)) / u128::from(divisor);
    if index + 1 < units.len() && scaled(units[index].0) >= 10_000 { index += 1; }
    let (divisor, unit) = units[index];
    let value = scaled(divisor);
    if value % 10 == 0 { format!("{}{unit}", value / 10) }
    else { format!("{}.{}{unit}", value / 10, value % 10) }
}

fn tooltip(summary: &Summary, chinese: bool) -> String
{
    if summary.updated_at.is_none()
    {
        return if summary.failure.is_some()
        {
            if chinese { "今日数据读取失败" } else { "Could not read today's usage" }
        }
        else if chinese { "正在读取今日用量…" } else { "Reading today's usage…" }.into();
    }
    let tokens = compact_tokens(summary.usage.tokens);
    let cost = match summary.usage.cost_usd
    {
        Some(value) if summary.usage.missing_price =>
            format!("${value:.4} {}", if chinese { "已计价" } else { "priced subtotal" }),
        Some(value) => format!("${value:.4}"),
        None => if chinese { "暂无估价" } else { "Price unavailable" }.into(),
    };
    let mut value = format!("{} {tokens} tokens · {cost}", if chinese { "今日" } else { "Today" });
    if summary.failure.is_some() { value.push_str(if chinese { " · 上次结果" } else { " · Last result" }); }
    value
}

fn main()
{
    #[cfg(windows)]
    if let Some(proxy) = network::proxy_server(std::env::args_os().skip(1).filter_map(|arg| arg.into_string().ok()))
    {
        // Forward the shortcut's setting to every HTTP worker before Tauri starts its threads.
        std::env::set_var(network::PROXY_SERVER_ENV, proxy);
    }
    tauri::Builder::default()
        .manage(WindowMemory::default())
        .plugin(tauri_plugin_single_instance::init(|app, _, _| open(app, None)))
        .invoke_handler(tauri::generate_handler![
            details_ready, dismiss_window, get_view_state, remember_view, set_appearance, set_preferences, get_collection_status,
            get_summary, get_activity, get_quotas, record_banked_reset_panel, get_quota_history, reset_quota_forecast,
            list_sessions, get_session_detail, refresh
        ])
        .on_window_event(|window, event| {
            if window.label() != WINDOW
            {
                return;
            }
            let app = window.app_handle();
            let memory = app.state::<WindowMemory>();
            match event
            {
                WindowEvent::CloseRequested { api, .. } =>
                {
                    api.prevent_close();
                    let _ = close_details(app);
                }
                WindowEvent::Resized(_) if window.is_minimized().unwrap_or(false) =>
                {
                    let _ = close_details(app);
                }
                #[cfg(windows)]
                WindowEvent::Destroyed =>
                {
                    *memory.backdrop.lock().expect("window material") = None;
                    backdrop::release();
                }
                #[cfg(windows)]
                WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } =>
                {
                    let material = *memory.backdrop.lock().expect("window material");
                    // Size events can arrive before the new HWND has selected its corner treatment.
                    if let Some(backdrop::Backdrop::Accent | backdrop::Backdrop::None) = material
                    {
                        backdrop::resize(window);
                    }
                }
                WindowEvent::Focused(true) =>
                {
                    memory.popover.lock().expect("popover").focus();
                }
                WindowEvent::Focused(false) if !window.is_focused().unwrap_or(false) =>
                {
                    let point = app.cursor_position().ok();
                    let should_close = memory.popover.lock().expect("popover").blur(point, Instant::now());
                    if should_close { let _ = close_details(app); }
                }
                _ => {}
            }
        })
        .setup(|app| {
            let root = app_data_root();
            fs::create_dir_all(&root)?;
            let preferences = Arc::new(PreferencesStore::load(&root));
            app.manage(preferences.clone());
            let chinese = preferences.get().language.resolve() == Language::Chinese;
            let menu = tray_menu(app.handle(), chinese)?;
            TrayIconBuilder::with_id("usage")
                .icon(app.default_window_icon().expect("application icon").clone())
                .tooltip("Token Monitor Lite")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref()
                {
                    "open" => open(app, None),
                    "refresh" => app.state::<Arc<Engine>>().refresh(Period::Today),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state, position, rect, .. } = event
                    {
                        let app = tray.app_handle();
                        let memory = app.state::<WindowMemory>();
                        let open_or_loading = app.get_webview_window(WINDOW).is_some();
                        if button_state == MouseButtonState::Down
                        {
                            memory.popover.lock().expect("popover").tray_down(open_or_loading, position, Instant::now());
                            return;
                        }
                        let closes = memory.popover.lock().expect("popover").tray_up(open_or_loading, position, Instant::now());
                        if closes
                        {
                            let _ = close_details(app);
                        }
                        else
                        {
                            let size: PhysicalSize<u32> = rect.size.to_physical(1.0);
                            let anchor = if size.width > 0 && size.height > 0 { rect }
                                else { Rect { position: position.into(), size: PhysicalSize::new(1, 1).into() } };
                            open(app, Some(anchor));
                        }
                    }
                })
                .build(app)?;
            let handle = app.handle().clone();
            let quotas = Arc::new(QuotaCache::new(&root));
            let panel = handle.clone();
            let display_cache = quotas.clone();
            let display_preferences = preferences.clone();
            let (sampler, usage_signal) = Sampler::start(quotas.clone(), preferences.clone(), move || {
                if panel.get_webview_window(WINDOW).is_some()
                {
                    if let Some(snapshot) = display_cache.cached(display_preferences.get().quotas.tools)
                    {
                        let _ = panel.emit_to(WINDOW, "quota-forecast-updated", snapshot);
                    }
                }
            })?;
            app.manage(quotas);
            app.manage(sampler);
            let engine = Engine::start(&root, scanner_path()?, preferences, move |period, revision, today| {
                if let Some(summary) = &today { usage_signal.updated(summary); }
                let callback_handle = handle.clone();
                let _ = handle.run_on_main_thread(move || {
                    if let (Some(summary), Some(tray)) = (today, callback_handle.tray_by_id("usage"))
                    {
                        let language = callback_handle.state::<Arc<PreferencesStore>>().get().language;
                        let _ = tray.set_tooltip(Some(tooltip(&summary, language.resolve() == Language::Chinese)));
                    }
                    if callback_handle.get_webview_window(WINDOW).is_some()
                    {
                        let _ = callback_handle.emit_to(WINDOW, "usage-updated", Update { period, revision });
                    }
                });
            })?;
            app.manage(Arc::new(engine));
            let saved = fs::read(app_data_root().join("appearance.json")).unwrap_or_default();
            *app.state::<WindowMemory>().appearance.lock().expect("appearance") = decode_appearance(&saved);
            if std::env::args_os().any(|arg| arg == "--open")
            {
                open(app.handle(), None);
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("Token Monitor Lite initialization failed")
        .run(|app, event| {
            match event
            {
                RunEvent::ExitRequested { api, code: None, .. } => api.prevent_exit(),
                RunEvent::ExitRequested { code: Some(_), .. } =>
                {
                    app.state::<Arc<Engine>>().shutdown();
                    app.state::<Sampler>().shutdown();
                }
                _ => {}
            }
        });
}

#[cfg(test)]
mod tests
{
    use super::*;

    #[test]
    fn migrates_the_old_opacity_and_reads_independent_text_scale()
    {
        let before = decode_appearance(b"72");
        assert_eq!((before.opacity, before.font_scale), (72, 100));
        let after = decode_appearance(br#"{"opacity":90,"fontScale":120}"#);
        assert_eq!((after.opacity, after.font_scale), (90, 120));
    }

    #[test]
    fn tray_compacts_totals_at_the_same_unit_boundaries_as_details()
    {
        assert_eq!(compact_tokens(999), "999");
        assert_eq!(compact_tokens(1_250), "1.3K");
        assert_eq!(compact_tokens(999_950), "1M");
        assert_eq!(compact_tokens(51_896_280), "51.9M");
        assert_eq!(compact_tokens(1_008_000_000), "1B");
    }
}
