use tauri::{AppHandle, LogicalSize, PhysicalPosition, PhysicalSize, Rect};

#[derive(Clone, Copy)]
struct Area
{
    x: i32,
    y: i32,
    right: i32,
    bottom: i32,
}

impl Area
{
    fn from_parts(position: PhysicalPosition<i32>, size: PhysicalSize<u32>) -> Self
    {
        Self { x: position.x, y: position.y, right: position.x + size.width as i32,
            bottom: position.y + size.height as i32 }
    }

    fn center(self) -> PhysicalPosition<i32>
    {
        PhysicalPosition::new((self.x + self.right) / 2, (self.y + self.bottom) / 2)
    }
}

pub struct Placement
{
    pub position: PhysicalPosition<i32>,
    pub size: PhysicalSize<u32>,
    pub logical_size: LogicalSize<f64>,
}

fn place(screen: Area, work: Area, icon: Option<Area>, size: PhysicalSize<u32>, gap: i32) -> PhysicalPosition<i32>
{
    let width = size.width as i32;
    let height = size.height as i32;
    let mut x = work.right - width - gap;
    let mut y = work.bottom - height - gap;
    if let Some(icon) = icon
    {
        let center = icon.center();
        let left = (work.x - screen.x).max(work.x - center.x);
        let top = (work.y - screen.y).max(work.y - center.y);
        let right = (screen.right - work.right).max(center.x - work.right);
        let bottom = (screen.bottom - work.bottom).max(center.y - work.bottom);
        // The monitor work area leaves room for a taskbar on any edge.
        if bottom >= left.max(right).max(top)
        {
            x = center.x - width / 2;
        }
        else if top >= left.max(right)
        {
            x = center.x - width / 2;
            y = work.y + gap;
        }
        else
        {
            if left > right { x = work.x + gap; }
            y = center.y - height / 2;
        }
    }
    let min_x = work.x + gap;
    let min_y = work.y + gap;
    PhysicalPosition::new(x.clamp(min_x, (work.right - gap - width).max(min_x)),
        y.clamp(min_y, (work.bottom - gap - height).max(min_y)))
}

pub fn for_tray(app: &AppHandle, rect: Option<Rect>, preferred: LogicalSize<f64>) -> Option<Placement>
{
    // The tray implementation returns physical screen coordinates on Windows.
    let icon = rect.and_then(|rect| {
        let size: PhysicalSize<u32> = rect.size.to_physical(1.0);
        (size.width > 0 && size.height > 0).then(|| Area::from_parts(rect.position.to_physical(1.0), size))
    });
    let picked = icon.and_then(|area| {
        let point = area.center();
        app.monitor_from_point(point.x as f64, point.y as f64).ok().flatten()
    });
    let monitor = picked.or_else(|| app.primary_monitor().ok().flatten())?;
    let scale = monitor.scale_factor();
    let gap = (8.0 * scale).round() as i32;
    let screen = Area::from_parts(*monitor.position(), *monitor.size());
    let work = Area::from_parts(monitor.work_area().position, monitor.work_area().size);
    let wanted: PhysicalSize<u32> = preferred.to_physical(scale);
    let size = PhysicalSize::new(
        wanted.width.min((work.right - work.x - 2 * gap).max(1) as u32),
        wanted.height.min((work.bottom - work.y - 2 * gap).max(1) as u32),
    );
    Some(Placement { position: place(screen, work, icon, size, gap), size, logical_size: size.to_logical(scale) })
}

#[cfg(test)]
mod tests
{
    use super::*;

    #[test]
    fn opens_next_to_tray_and_clamps_within_work_area()
    {
        let screen = Area { x: -1920, y: 0, right: 0, bottom: 1080 };
        let size = PhysicalSize::new(597, 995);
        let bottom = Area { bottom: 1020, ..screen };
        let bottom_icon = Area { x: -126, right: -102, y: 1038, bottom: 1062 };
        let at = place(screen, bottom, Some(bottom_icon), size, 12);
        assert_eq!(at, PhysicalPosition::new(-609, 13));
        let top = Area { y: 60, ..screen };
        let top_icon = Area { y: 18, bottom: 42, ..bottom_icon };
        assert_eq!(place(screen, top, Some(top_icon), size, 12).y, 72);
        let left = Area { x: -1860, ..screen };
        let left_icon = Area { x: -1908, right: -1884, y: 990, bottom: 1014 };
        assert_eq!(place(screen, left, Some(left_icon), size, 12).x, -1848);
        let right = Area { right: -60, ..screen };
        let right_icon = Area { x: -42, right: -18, y: 990, bottom: 1014 };
        assert_eq!(place(screen, right, Some(right_icon), size, 12).x, -669);
    }
}
