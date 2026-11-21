use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

use crate::{app_data_root, app_navigation, app_log::AppLog, placement, quota_history::QuotaKey,
    settings::Language, WindowMemory, WINDOW as DETAILS};

pub const WINDOW: &str = "quota-history";
const SELECTION: &str = "quota-popup-selection";
const INTERACTION: &str = "quota-popup-interaction";

#[derive(Clone, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct Selection
{
    id: u32,
    key: QuotaKey,
    label: String,
    current: String,
    reset_at: Option<i64>,
    revision: Option<i64>,
    language: Language,
    font_scale: u8,
}

#[derive(Deserialize)]
pub struct Target
{
    selection: Selection,
    top: f64,
    focus: bool,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Report
{
    Ready,
    Size { id: u32, height: f64 },
    Enter { id: u32 },
    Leave { id: u32 },
    Close { id: u32 },
}

#[derive(Default)]
pub struct Popup
{
    generation: u32,
    target: Option<Target>,
    ready: bool,
    height: Option<f64>,
}

#[tauri::command]
pub async fn set_quota_popup(window: WebviewWindow, generation: u32, target: Option<Target>) -> Result<(), String>
{
    if window.label() != DETAILS { return Err("Expected the details window.".into()); }
    // Creating WebView2 from within a synchronous IPC callback can deadlock Windows.
    window.app_handle().clone().run_on_main_thread(move || {
        let app = window.app_handle();
        if let Err(error) = update(&window, generation, target)
        {
            log_error(app, &error);
            dismiss(app, None);
        }
    }).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn report_quota_popup(window: WebviewWindow, report: Report) -> Result<(), String>
{
    if window.label() != WINDOW { return Err("Expected the quota history window.".into()); }
    window.app_handle().clone().run_on_main_thread(move || receive(&window, report)).map_err(|error| error.to_string())
}

fn update(owner: &WebviewWindow, generation: u32, mut target: Option<Target>) -> tauri::Result<()>
{
    if !owner.is_visible()? || owner.is_minimized()? { return Ok(()); }
    let app = owner.app_handle();
    let memory = app.state::<WindowMemory>();
    let (changed_id, selection, selection_changed, top, ready) = {
        let mut popup = memory.quota_popup.lock().expect("quota popup");
        if generation <= popup.generation { return Ok(()); }
        popup.generation = generation;
        let previous = popup.target.as_ref().map(|target| &target.selection);
        let next = target.as_ref().map(|target| &target.selection);
        let changed_id = previous.map(|selection| selection.id) != next.map(|selection| selection.id);
        let selection_changed = previous != next;
        let selection = next.cloned();
        if changed_id { popup.height = None; }
        else if let Some(target) = &mut target
        {
            target.focus = popup.target.as_ref().is_some_and(|previous| previous.focus);
        }
        let top = target.as_ref().map(|target| target.top);
        popup.target = target;
        (changed_id, selection, selection_changed, top, popup.ready)
    };
    let window = app.get_webview_window(WINDOW);
    if changed_id
    {
        if let Some(window) = &window { hide(window); }
    }
    if ready && selection_changed { app.emit_to(WINDOW, SELECTION, selection)?; }
    if let Some(top) = top
    {
        if window.is_none() { create(owner, top)?; }
        else { place(app)?; }
    }
    Ok(())
}

fn create(owner: &WebviewWindow, top: f64) -> tauri::Result<()>
{
    let app = owner.app_handle();
    let initial = placement::for_quota_popup(owner, top, 80.0)?;
    let width = initial.as_ref().map_or(placement::QUOTA_POPUP_WIDTH, |bounds| bounds.logical_size.width);
    let builder = WebviewWindowBuilder::new(app, WINDOW, WebviewUrl::App("index.html".into()))
        .title("")
        .inner_size(width, 80.0)
        .decorations(false)
        .shadow(false)
        .resizable(false)
        .skip_taskbar(true)
        .theme(Some(tauri::Theme::Dark))
        .transparent(true)
        .background_color(tauri::window::Color(0, 0, 0, 0))
        .visible(false)
        .focused(false)
        .data_directory(app_data_root().join("WebView2"))
        .on_navigation(app_navigation);
    let window = builder.parent(owner)?.build()?;
    if let Some(bounds) = initial { set_bounds(&window, &bounds, false)?; }
    Ok(())
}

fn receive(window: &WebviewWindow, report: Report)
{
    let app = window.app_handle();
    let memory = app.state::<WindowMemory>();
    match report
    {
        Report::Ready =>
        {
            let selection = {
                let mut popup = memory.quota_popup.lock().expect("quota popup");
                popup.ready = true;
                popup.target.as_ref().map(|target| target.selection.clone())
            };
            let _ = app.emit_to(WINDOW, SELECTION, selection);
        }
        Report::Size { id, height } if height.is_finite() && height > 0.0 =>
        {
            {
                let mut popup = memory.quota_popup.lock().expect("quota popup");
                if !popup.target.as_ref().is_some_and(|target| target.selection.id == id) { return; }
                popup.height = Some(height);
            }
            reposition(app);
        }
        Report::Enter { id } => interaction(app, id, "enter"),
        Report::Leave { id } => interaction(app, id, "leave"),
        Report::Close { id } => dismiss(app, Some(id)),
        _ => {}
    }
}

fn interaction(app: &AppHandle, id: u32, kind: &str)
{
    let _ = app.emit_to(DETAILS, INTERACTION, serde_json::json!({ "id": id, "kind": kind }));
}

pub fn reposition(app: &AppHandle)
{
    if let Err(error) = place(app)
    {
        log_error(app, &error);
        dismiss(app, None);
    }
}

fn place(app: &AppHandle) -> tauri::Result<()>
{
    let memory = app.state::<WindowMemory>();
    let (top, height, focus) = {
        let mut popup = memory.quota_popup.lock().expect("quota popup");
        let Some(height) = popup.height else { return Ok(()) };
        let Some(target) = &mut popup.target else { return Ok(()) };
        (target.top, height, std::mem::take(&mut target.focus))
    };
    let Some(owner) = app.get_webview_window(DETAILS) else { return Ok(()) };
    let Some(window) = app.get_webview_window(WINDOW) else { return Ok(()) };
    if !owner.is_visible()? || owner.is_minimized()? { return Ok(()); }
    if let Some(bounds) = placement::for_quota_popup(&owner, top, height)?
    {
        set_bounds(&window, &bounds, true)?;
        if focus { activate(&window); }
    }
    Ok(())
}

fn dismiss(app: &AppHandle, id: Option<u32>)
{
    let memory = app.state::<WindowMemory>();
    let (selected, ready) = {
        let mut popup = memory.quota_popup.lock().expect("quota popup");
        let Some(target) = &popup.target else { return };
        let selected = target.selection.id;
        if id.is_some_and(|id| id != selected) { return; }
        popup.target = None;
        popup.height = None;
        (selected, popup.ready)
    };
    if let Some(window) = app.get_webview_window(WINDOW) { hide(&window); }
    if ready { let _ = app.emit_to(WINDOW, SELECTION, Option::<Selection>::None); }
    interaction(app, selected, "close");
}

pub fn destroy(app: &AppHandle)
{
    *app.state::<WindowMemory>().quota_popup.lock().expect("quota popup") = Popup::default();
    if let Some(window) = app.get_webview_window(WINDOW) { let _ = window.destroy(); }
}

pub fn on_event(app: &AppHandle, event: &WindowEvent)
{
    match event
    {
        WindowEvent::CloseRequested { api, .. } =>
        {
            api.prevent_close();
            dismiss(app, None);
        }
        WindowEvent::Focused(false) =>
        {
            if let Some(owner) = app.get_webview_window(DETAILS) { crate::panel_blur(&owner.as_ref().window()); }
        }
        _ => {}
    }
}

pub fn is_visible(app: &AppHandle) -> bool
{
    app.get_webview_window(WINDOW).is_some_and(|window| window.is_visible().unwrap_or(false))
}

pub fn is_focused(app: &AppHandle) -> bool
{
    let Some(window) = app.get_webview_window(WINDOW) else { return false };
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
        window.hwnd().is_ok_and(|hwnd| hwnd.0 == unsafe { GetForegroundWindow() })
    }
    #[cfg(not(windows))]
    window.is_focused().unwrap_or(false)
}

fn log_error(app: &AppHandle, error: &tauri::Error)
{
    if let Some(log) = app.try_state::<Arc<AppLog>>() { log.write(format_args!("quota popup result=failed error={error}")); }
}

fn set_bounds(window: &WebviewWindow, bounds: &placement::Placement, show: bool) -> tauri::Result<()>
{
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW};
        // SWP_SHOWWINDOW keeps the existing foreground window active on the hover path.
        let mut flags = SWP_NOACTIVATE | SWP_NOZORDER;
        if show { flags |= SWP_SHOWWINDOW; }
        let hwnd = window.hwnd()?.0;
        let success = unsafe { SetWindowPos(hwnd, std::ptr::null_mut(), bounds.position.x, bounds.position.y,
            bounds.size.width as i32, bounds.size.height as i32, flags) };
        if success == 0 { return Err(std::io::Error::last_os_error().into()); }
    }
    #[cfg(not(windows))]
    {
        window.set_position(bounds.position)?;
        window.set_size(bounds.size)?;
        if show { window.show()?; }
    }
    Ok(())
}

fn hide(window: &WebviewWindow)
{
    let app = window.app_handle();
    if is_focused(app)
    {
        if let Some(owner) = app.get_webview_window(DETAILS) { activate(&owner); }
    }
    #[cfg(windows)]
    if let Ok(hwnd) = window.hwnd()
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
        unsafe { ShowWindow(hwnd.0, SW_HIDE); }
    }
    #[cfg(not(windows))]
    let _ = window.hide();
}

fn activate(window: &WebviewWindow)
{
    #[cfg(windows)]
    if let Ok(hwnd) = window.hwnd()
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow;
        unsafe { SetForegroundWindow(hwnd.0); }
        let _ = window.as_ref().set_focus();
    }
    #[cfg(not(windows))]
    let _ = window.set_focus();
}
