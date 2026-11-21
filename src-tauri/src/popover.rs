use std::{mem, time::{Duration, Instant}};
use tauri::PhysicalPosition;

const TRAY_PRESS_WINDOW: Duration = Duration::from_millis(250);
const SAME_PRESS_DISTANCE: f64 = 8.0;

#[cfg(windows)]
pub(crate) fn cursor_outside_tray(rect: &tauri::Rect) -> bool
{
    use windows_sys::Win32::{Foundation::POINT, UI::WindowsAndMessaging::GetCursorPos};
    let mut cursor = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut cursor) } == 0 { return false; }
    // Native tray events supply physical screen coordinates, including on secondary displays.
    let origin = rect.position.to_physical::<i32>(1.0);
    let size = rect.size.to_physical::<u32>(1.0);
    !(origin.x..origin.x + size.width as i32).contains(&cursor.x) ||
        !(origin.y..origin.y + size.height as i32).contains(&cursor.y)
}

#[derive(Default)]
pub struct Popover
{
    activated: bool,
    closing_press: bool,
    last_blur: Option<(Instant, PhysicalPosition<f64>)>,
}

impl Popover
{
    pub fn opening(&mut self)
    {
        *self = Self::default();
    }

    pub fn focus(&mut self)
    {
        self.activated = true;
    }

    pub fn closing(&mut self)
    {
        self.activated = false;
    }

    pub fn blur(&mut self, point: Option<PhysicalPosition<f64>>, now: Instant) -> bool
    {
        if !mem::take(&mut self.activated) { return false; }
        self.last_blur = point.map(|point| (now, point));
        true
    }

    pub fn tray_down(&mut self, visible: bool, point: PhysicalPosition<f64>, now: Instant)
    {
        self.closing_press = visible || self.take_matching_blur(point, now);
    }

    pub fn tray_up(&mut self, visible: bool, point: PhysicalPosition<f64>, now: Instant) -> bool
    {
        let closes = mem::take(&mut self.closing_press);
        let blurred = self.take_matching_blur(point, now);
        visible || closes || blurred
    }

    fn take_matching_blur(&mut self, point: PhysicalPosition<f64>, now: Instant) -> bool
    {
        // Windows can deliver activation loss before the notification area's press.
        self.last_blur.take().is_some_and(|(when, previous)| {
            now.saturating_duration_since(when) <= TRAY_PRESS_WINDOW
                && (point.x - previous.x).abs().max((point.y - previous.y).abs()) <= SAME_PRESS_DISTANCE
        })
    }
}

#[cfg(test)]
mod tests
{
    use super::*;

    #[test]
    fn outside_activation_closes_and_a_tray_press_does_not_reopen_the_same_click()
    {
        let mut popover = Popover::default();
        let tray = PhysicalPosition::new(1700.0, 1060.0);
        let outside = PhysicalPosition::new(1110.0, 420.0);
        let now = Instant::now();
        let later = now + Duration::from_millis(60);
        assert!(!popover.blur(Some(outside), now));
        popover.focus();
        assert!(popover.blur(Some(tray), now));
        popover.tray_down(false, tray, later);
        assert!(popover.tray_up(false, tray, later));
        popover.opening();
        popover.focus();
        popover.tray_down(true, tray, now);
        assert!(popover.blur(Some(tray), later));
        assert!(popover.tray_up(false, tray, later));
        popover.opening();
        popover.focus();
        assert!(popover.blur(Some(outside), now));
        popover.tray_down(false, tray, later);
        assert!(!popover.tray_up(false, tray, later));
    }
}
