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
            Foundation::{HWND, LPARAM, LRESULT, WPARAM},
            Graphics::{
                Dwm::{
                    DwmExtendFrameIntoClientArea, DwmSetWindowAttribute, DWMSBT_TRANSIENTWINDOW,
                    DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_SYSTEMBACKDROP_TYPE,
                    DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
                },
                Gdi::{CreateRoundRectRgn, DeleteObject, SetWindowRgn},
            },
            System::LibraryLoader::{GetModuleHandleA, GetProcAddress},
            UI::{
                Controls::MARGINS,
                Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
                WindowsAndMessaging::{
                    GetWindowLongW, SetWindowLongW, SetWindowPos, GWL_STYLE, STYLESTRUCT,
                    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
                    WM_NCDESTROY, WM_STYLECHANGING, WS_CAPTION, WS_MAXIMIZEBOX, WS_MINIMIZEBOX, WS_SYSMENU,
                },
            },
        },
    };

    use super::{Backdrop, WebviewWindow};

    const WCA_ACCENT_POLICY: u32 = 19;
    const ACCENT_DISABLED: u32 = 0;
    const ACCENT_ENABLE_HOSTBACKDROP: u32 = 5;
    const CORNER_RADIUS: f64 = 8.0; // Matches the Windows shell radius in styles.css.
    const CAPTION_STYLES: u32 = WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX;

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
        let bounds = window.outer_size().ok().zip(window.scale_factor().ok());
        unsafe
        {
            // Windows 11 supports this preference even on builds without system backdrop materials.
            let native_corners = attribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &DWMWCP_ROUND);
            apply_backdrop(hwnd, native_corners, bounds)
        }
    }

    unsafe fn apply_backdrop(hwnd: HWND, native_corners: bool, bounds: Option<(PhysicalSize<u32>, f64)>) -> Backdrop
    {
        if native_corners
        {
            // Only Windows 11 exposes the unwanted caption. Preserve Tao's frame styles on Windows 10.
            remove_native_caption(hwnd);
        }
        let glass = MARGINS { cxLeftWidth: -1, cxRightWidth: -1, cyTopHeight: -1, cyBottomHeight: -1 };
        let extended = DwmExtendFrameIntoClientArea(hwnd, &glass) >= 0;
        if native_corners && extended && attribute(hwnd, DWMWA_SYSTEMBACKDROP_TYPE, &DWMSBT_TRANSIENTWINDOW)
        {
            attribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &1_u32);
            attribute(hwnd, DWMWA_BORDER_COLOR, &DWMWA_COLOR_NONE);
            return Backdrop::System;
        }
        apply_legacy(hwnd, bounds, extended)
    }

    unsafe fn apply_legacy(hwnd: HWND, bounds: Option<(PhysicalSize<u32>, f64)>, extended: bool) -> Backdrop
    {
        if let Some((size, scale)) = bounds
        {
            // Clip the HWND even when the legacy brush is unavailable and WebView2 paints the fallback.
            round(hwnd, size, scale);
            if extended && set_accent(hwnd, ACCENT_ENABLE_HOSTBACKDROP)
            {
                match Glass::new(hwnd, size, scale)
                {
                    Ok(glass) =>
                    {
                        NATIVE_GLASS.with_borrow_mut(|active| *active = Some(glass));
                        return Backdrop::Accent;
                    }
                    Err(error) => eprintln!("Could not create clipped window backdrop: {error}"),
                }
                set_accent(hwnd, ACCENT_DISABLED);
            }
        }
        if extended
        {
            DwmExtendFrameIntoClientArea(hwnd, &MARGINS::default());
        }
        Backdrop::None
    }

    unsafe fn remove_native_caption(hwnd: HWND)
    {
        // Tao keeps the caption styles on undecorated windows and reapplies them when showing the host.
        SetWindowSubclass(hwnd, Some(without_caption), 0, 0);
        let style = GetWindowLongW(hwnd, GWL_STYLE) as u32;
        SetWindowLongW(hwnd, GWL_STYLE, (style & !CAPTION_STYLES) as i32);
        SetWindowPos(hwnd, std::ptr::null_mut(), 0, 0, 0, 0,
            SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER);
    }

    unsafe extern "system" fn without_caption(
        hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM, id: usize, _: usize,
    ) -> LRESULT
    {
        if message == WM_STYLECHANGING && wparam as i32 == GWL_STYLE
        {
            (*(lparam as *mut STYLESTRUCT)).styleNew &= !CAPTION_STYLES;
        }
        else if message == WM_NCDESTROY
        {
            RemoveWindowSubclass(hwnd, Some(without_caption), id);
        }
        DefSubclassProc(hwnd, message, wparam, lparam)
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
        // GDI's rounded region otherwise drops the last row and column of the WebView.
        let region = CreateRoundRectRgn(0, 0, size.width as i32 + 1, size.height as i32 + 1, diameter, diameter);
        if !region.is_null() && SetWindowRgn(hwnd, region, 1) == 0
        {
            DeleteObject(region);
        }
    }

    #[cfg(test)]
    mod tests
    {
        use super::*;
        use std::ptr::{null, null_mut};
        use windows_sys::Win32::{
            Foundation::RECT,
            Graphics::Gdi::{CreateRectRgn, GetWindowRgn, GetWindowRgnBox, PtInRegion, COMPLEXREGION},
            UI::WindowsAndMessaging::{CreateWindowExW, DestroyWindow, WS_POPUP, WS_THICKFRAME},
        };

        struct TestWindow(HWND);

        impl TestWindow
        {
            fn new() -> Self
            {
                let hwnd = unsafe { CreateWindowExW(0, windows_sys::w!("STATIC"), null(),
                    WS_POPUP | WS_THICKFRAME | CAPTION_STYLES, 0, 0, 320, 200,
                    null_mut(), null_mut(), null_mut(), null()) };
                assert!(!hwnd.is_null());
                Self(hwnd)
            }
        }

        impl Drop for TestWindow
        {
            fn drop(&mut self)
            {
                release();
                unsafe { DestroyWindow(self.0); }
            }
        }

        #[test]
        fn legacy_window_keeps_its_frame_styles_and_has_a_rounded_region()
        {
            let window = TestWindow::new();
            unsafe
            {
                let material = apply_backdrop(window.0, false, Some((PhysicalSize::new(320, 200), 1.0)));
                assert!(material != Backdrop::System);
                assert_eq!(GetWindowLongW(window.0, GWL_STYLE) as u32 & CAPTION_STYLES, CAPTION_STYLES);
                assert_eq!(GetWindowRgnBox(window.0, &mut RECT::default()), COMPLEXREGION);
            }
        }

        #[test]
        fn gradient_fallback_clips_all_four_corners_at_the_current_size_and_dpi()
        {
            let window = TestWindow::new();
            for (width, height, scale) in [(320, 200, 1.0), (480, 300, 1.5)]
            {
                unsafe
                {
                    assert_ne!(SetWindowPos(window.0, null_mut(), 0, 0, width, height,
                        SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOZORDER), 0);
                    let bounds = (PhysicalSize::new(width as u32, height as u32), scale);
                    assert!(apply_legacy(window.0, Some(bounds), false) == Backdrop::None);
                    let region = CreateRectRgn(0, 0, 0, 0);
                    assert!(!region.is_null());
                    let kind = GetWindowRgn(window.0, region);
                    let right = width - 1;
                    let bottom = height - 1;
                    let visible = [(0, 0), (right, 0), (0, bottom), (right, bottom),
                        (0, height / 2), (right, height / 2), (width / 2, 0), (width / 2, bottom), (4, 1)]
                        .map(|(x, y)| PtInRegion(region, x, y) != 0);
                    DeleteObject(region);
                    assert_eq!(kind, COMPLEXREGION);
                    assert_eq!(visible, [false, false, false, false, true, true, true, true, scale == 1.0]);
                }
            }
        }

        #[test]
        fn native_corners_keep_caption_buttons_hidden_when_tao_reapplies_styles()
        {
            let window = TestWindow::new();
            unsafe
            {
                let material = apply_backdrop(window.0, true, Some((PhysicalSize::new(320, 200), 1.0)));
                let style = GetWindowLongW(window.0, GWL_STYLE) as u32;
                assert_eq!(style & CAPTION_STYLES, 0);
                SetWindowLongW(window.0, GWL_STYLE, (style | CAPTION_STYLES) as i32);
                assert_eq!(GetWindowLongW(window.0, GWL_STYLE) as u32 & (CAPTION_STYLES | WS_THICKFRAME), WS_THICKFRAME);
                if material == Backdrop::System
                {
                    assert_eq!(GetWindowRgnBox(window.0, &mut RECT::default()), 0);
                }
            }
        }
    }
}

#[cfg(windows)]
pub use windows::{apply, release, resize};

#[cfg(not(windows))]
pub fn apply(_: &WebviewWindow) -> Backdrop { Backdrop::None }
