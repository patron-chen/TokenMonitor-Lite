use serde::Serialize;
use tauri::WebviewWindow;

#[derive(Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Backdrop
{
    #[default]
    None,
    System,
    Accent,
}

#[cfg(windows)]
mod windows
{
    use std::{cell::RefCell, ffi::c_void, mem::size_of};

    use ::windows::{
        core::{Interface, Result},
        System::{DispatcherQueue, DispatcherQueueController},
        UI::Composition::{CompositionRoundedRectangleGeometry, Compositor, Desktop::DesktopWindowTarget, SpriteVisual},
        Win32::System::WinRT::{
            Composition::ICompositorDesktopInterop, CreateDispatcherQueueController, DispatcherQueueOptions,
            DQTAT_COM_NONE, DQTYPE_THREAD_CURRENT,
        },
    };
    use tauri::PhysicalSize;
    use windows_sys::{
        core::BOOL,
        Win32::{
            Foundation::HWND,
            Graphics::{
                Dwm::{
                    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMSBT_TRANSIENTWINDOW,
                    DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_SYSTEMBACKDROP_TYPE,
                    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
                },
                Gdi::{CreateRoundRectRgn, DeleteObject, SetWindowRgn},
            },
            System::LibraryLoader::{GetModuleHandleA, GetProcAddress},
            UI::Controls::MARGINS,
        },
    };

    use super::{Backdrop, WebviewWindow};

    const WCA_ACCENT_POLICY: u32 = 19;
    const ACCENT_DISABLED: u32 = 0;
    const ACCENT_ENABLE_HOSTBACKDROP: u32 = 5;
    const CORNER_RADIUS: f64 = 8.0; // Matches the Windows shell radius in styles.css.

    struct Glass
    {
        _compositor: Compositor,
        target: DesktopWindowTarget,
        visual: SpriteVisual,
        geometry: CompositionRoundedRectangleGeometry,
    }

    thread_local!
    {
        static DISPATCHER: RefCell<Option<DispatcherQueueController>> = const { RefCell::new(None) };
        static NATIVE_GLASS: RefCell<Option<Glass>> = const { RefCell::new(None) };
    }

    pub fn apply(window: &WebviewWindow) -> Backdrop
    {
        let Ok(handle) = window.hwnd() else { return Backdrop::None };
        let hwnd = handle.0;
        let glass = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
        // Frameless hosts need the DWM surface to extend underneath WebView2.
        unsafe {
            if DwmExtendFrameIntoClientArea(hwnd, &glass) < 0 { return Backdrop::None; }
            if attribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &DWMSBT_TRANSIENTWINDOW)
            {
                attribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &1_u32);
                attribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &DWMWCP_ROUND);
                attribute(hwnd, DWMWA_BORDER_COLOR, &DWMWA_COLOR_NONE);
                return Backdrop::System;
            }
            if let (Ok(size), Ok(scale)) = (window.outer_size(), window.scale_factor())
            {
                if set_accent(hwnd, ACCENT_ENABLE_HOSTBACKDROP)
                {
                    match Glass::new(hwnd, size, scale)
                    {
                        Ok(glass) =>
                        {
                            NATIVE_GLASS.with_borrow_mut(|active| *active = Some(glass));
                            round(hwnd, size, scale);
                            return Backdrop::Accent;
                        }
                        Err(error) => eprintln!("Could not create clipped window backdrop: {error}"),
                    }
                    set_accent(hwnd, ACCENT_DISABLED);
                }
            }
            DwmExtendFrameIntoClientArea(hwnd, &MARGINS::default());
        }
        Backdrop::None
    }

    unsafe fn attribute<T>(hwnd: HWND, key: i32, value: &T) -> bool
    {
        DwmSetWindowAttribute(hwnd, key as u32, (value as *const T).cast(), size_of::<T>() as u32) >= 0
    }

    #[repr(C)]
    struct AccentPolicy
    {
        state: u32,
        flags: u32,
        color: u32,
        animation: u32,
    }

    #[repr(C)]
    struct CompositionAttribute
    {
        kind: u32,
        data: *mut c_void,
        size: usize,
    }

    unsafe fn set_accent(hwnd: HWND, state: u32) -> bool
    {
        // Windows 10 enables the host brush through the accent policy.
        type SetAttribute = unsafe extern "system" fn(HWND, *mut CompositionAttribute) -> BOOL;
        let user32 = GetModuleHandleA(c"user32.dll".as_ptr().cast());
        if user32.is_null() { return false; }
        let Some(symbol) = GetProcAddress(user32, c"SetWindowCompositionAttribute".as_ptr().cast()) else { return false };
        let set_attribute: SetAttribute = std::mem::transmute(symbol);
        let mut policy = AccentPolicy { state, flags: 0, color: 0, animation: 0 };
        let mut data = CompositionAttribute {
            kind: WCA_ACCENT_POLICY, data: (&mut policy as *mut AccentPolicy).cast(), size: size_of::<AccentPolicy>(),
        };
        set_attribute(hwnd, &mut data) != 0
    }

    impl Glass
    {
        fn new(hwnd: HWND, size: PhysicalSize<u32>, scale: f64) -> Result<Self>
        {
            if DispatcherQueue::GetForCurrentThread().is_err()
            {
                // Attach composition to the existing UI thread's message loop.
                let options = DispatcherQueueOptions {
                    dwSize: size_of::<DispatcherQueueOptions>() as u32,
                    threadType: DQTYPE_THREAD_CURRENT,
                    apartmentType: DQTAT_COM_NONE,
                };
                let controller = unsafe { CreateDispatcherQueueController(options)? };
                DISPATCHER.with_borrow_mut(|queue| *queue = Some(controller));
            }
            let compositor = Compositor::new()?;
            let interop = compositor.cast::<ICompositorDesktopInterop>()?;
            let target = unsafe { interop.CreateDesktopWindowTarget(::windows::Win32::Foundation::HWND(hwnd), false)? };
            let visual = compositor.CreateSpriteVisual()?;
            let geometry = compositor.CreateRoundedRectangleGeometry()?;
            // The HWND region clips input and content. The brush needs its own
            // clip because the legacy DWM accent effect fills the whole rectangle.
            let clip = compositor.CreateGeometricClipWithGeometry(&geometry)?;
            visual.SetClip(&clip)?;
            visual.SetBrush(&compositor.CreateHostBackdropBrush()?)?;
            let glass = Self { _compositor: compositor, target, visual, geometry };
            glass.fit(size, scale)?;
            glass.target.SetRoot(&glass.visual)?;
            Ok(glass)
        }

        fn fit(&self, size: PhysicalSize<u32>, scale: f64) -> Result<()>
        {
            let mut extent = self.geometry.Size()?;
            extent.X = size.width as f32;
            extent.Y = size.height as f32;
            self.geometry.SetSize(extent)?;
            let mut radius = self.geometry.CornerRadius()?;
            radius.X = (CORNER_RADIUS * scale) as f32;
            radius.Y = radius.X;
            self.geometry.SetCornerRadius(radius)?;
            self.visual.SetSize(extent)
        }
    }

    pub fn resize(window: &tauri::Window)
    {
        if let (Ok(handle), Ok(size), Ok(scale)) = (window.hwnd(), window.outer_size(), window.scale_factor())
        {
            NATIVE_GLASS.with_borrow(|glass| {
                if let Some(glass) = glass { let _ = glass.fit(size, scale); }
            });
            unsafe { round(handle.0, size, scale); }
        }
    }

    pub fn release()
    {
        NATIVE_GLASS.with_borrow_mut(|glass| *glass = None);
    }

    unsafe fn round(hwnd: HWND, size: PhysicalSize<u32>, scale: f64)
    {
        if size.width == 0 || size.height == 0 { return; }
        let diameter = (2.0 * CORNER_RADIUS * scale).round() as i32;
        let region = CreateRoundRectRgn(0, 0, size.width as i32, size.height as i32, diameter, diameter);
        if !region.is_null() && SetWindowRgn(hwnd, region, 1) == 0
        {
            DeleteObject(region);
        }
    }
}

#[cfg(windows)]
pub use windows::{apply, release, resize};

#[cfg(not(windows))]
pub fn apply(_: &WebviewWindow) -> Backdrop { Backdrop::None }
