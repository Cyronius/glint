//! The tray icon, drawn from live metrics.
//!
//! The icon is the only glint surface that is always on screen, so it carries
//! one micro-bar per measurement. Pixels go straight into a 32-bit DIB, which
//! keeps the alpha channel exact and involves no font or shape call: at the
//! 16 to 24 pixels Windows gives a tray icon, GDI's own rounding is coarser
//! than writing the bytes.

use windows::Win32::Foundation::COLORREF;
use windows::Win32::Graphics::Gdi::{
    CreateBitmap, CreateDIBSection, DeleteObject, GetDC, ReleaseDC, BITMAPINFO, BITMAPINFOHEADER,
    BI_RGB, DIB_RGB_COLORS, HBITMAP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, GetSystemMetrics, HICON, ICONINFO, SM_CXSMICON,
};

use crate::render::{
    alert_color, rgb_parts, Shown, CPU_HUE, DISK_HUE, GPU_HUE, MEMORY_HUE, NPU_HUE,
};
use crate::sampler::Metrics;

/// The accent blue of the fallback logo. It reads on a light taskbar and on a
/// dark one.
const LOGO: (u8, u8, u8) = (0x4C, 0x9A, 0xFF);

/// The unfilled part of a bar. Mid grey at partial alpha sits visibly on both
/// a light and a dark taskbar, which a single opaque shade cannot.
const TRACK: (u8, u8, u8) = (0x8A, 0x8A, 0x8A);
/// The track alternates between two shades per row. There are no gap pixels
/// anywhere — bars use their whole pitch and touch — so this banding is the
/// only thing keeping five idle strips from reading as one grey slab.
const TRACK_ALPHA: [u8; 2] = [0xB0, 0x82];

/// A 32-bit top-down DIB that pixels can be written into directly.
struct Canvas {
    size: i32,
    bitmap: HBITMAP,
    pixels: *mut u32,
}

impl Canvas {
    fn new(size: i32) -> Option<Self> {
        let header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                // Negative height makes the rows top down, which is easier to
                // reason about than the default bottom-up order.
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels: *mut core::ffi::c_void = std::ptr::null_mut();
        let screen = unsafe { GetDC(None) };
        let bitmap = unsafe {
            CreateDIBSection(
                Some(screen),
                &raw const header,
                DIB_RGB_COLORS,
                &raw mut pixels,
                None,
                0,
            )
        };
        unsafe { ReleaseDC(None, screen) };
        let bitmap = bitmap.ok()?;
        if pixels.is_null() {
            unsafe {
                let _ = DeleteObject(bitmap.into());
            }
            return None;
        }
        let mut canvas = Self {
            size,
            bitmap,
            pixels: pixels.cast::<u32>(),
        };
        canvas.buffer().fill(0);
        Some(canvas)
    }

    fn buffer(&mut self) -> &mut [u32] {
        let count = (self.size * self.size) as usize;
        unsafe { std::slice::from_raw_parts_mut(self.pixels, count) }
    }

    /// Write one premultiplied BGRA pixel.
    fn put(&mut self, x: i32, y: i32, (r, g, b): (u8, u8, u8), alpha: u8) {
        if x < 0 || y < 0 || x >= self.size || y >= self.size {
            return;
        }
        let a = u32::from(alpha);
        let pre = |c: u8| (u32::from(c) * a) / 255;
        let value = (a << 24) | (pre(r) << 16) | (pre(g) << 8) | pre(b);
        let offset = (y * self.size + x) as usize;
        self.buffer()[offset] = value;
    }

    fn rect(&mut self, x0: i32, y0: i32, width: i32, height: i32, color: (u8, u8, u8), alpha: u8) {
        for y in y0..(y0 + height) {
            for x in x0..(x0 + width) {
                self.put(x, y, color, alpha);
            }
        }
    }

    /// Turn the pixels into an icon. A 1-bit mask of zeros means "use the
    /// alpha channel".
    fn into_icon(self) -> HICON {
        let mask: HBITMAP = unsafe { CreateBitmap(self.size, self.size, 1, 1, None) };
        let info = ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: self.bitmap,
        };
        let icon = unsafe { CreateIconIndirect(&info) };
        unsafe {
            let _ = DeleteObject(self.bitmap.into());
            let _ = DeleteObject(mask.into());
        }
        std::mem::forget(self);
        icon.unwrap_or_default()
    }
}

impl Drop for Canvas {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.bitmap.into());
        }
    }
}

/// The pixel grid the icon is drawn on.
///
/// This is the size the shell actually displays, measured rather than
/// assumed: a full-bleed probe icon rendered exactly 16 x 16 on this machine
/// at 100% scaling. Supplying larger art does not buy resolution — the shell
/// downscales it and the bars come back blurred — so the icon is drawn
/// native and crisp, and the bar tips are antialiased by hand instead.
fn icon_size() -> i32 {
    unsafe { GetSystemMetrics(SM_CXSMICON) }.clamp(16, 64)
}

/// The bars the icon carries, top to bottom, in the order the window draws
/// its rows.
///
/// Disk space is left out on purpose: it is per drive, so it has no single
/// value a bar could show.
fn bars(metrics: &Metrics, shown: Shown) -> Vec<(&'static str, f32, COLORREF)> {
    let mut bars = Vec::with_capacity(5);
    if shown.cpu {
        bars.push(("CPU", metrics.cpu, CPU_HUE));
    }
    if shown.memory {
        bars.push(("MEM", metrics.memory, MEMORY_HUE));
    }
    if shown.gpu {
        bars.push(("GPU", metrics.gpu.unwrap_or(0.0), GPU_HUE));
    }
    if shown.npu {
        bars.push(("NPU", metrics.npu.unwrap_or(0.0), NPU_HUE));
    }
    if shown.disk_activity {
        bars.push(("DISK", metrics.disk, DISK_HUE));
    }
    bars
}

/// The same list the icon draws, for the hover text.
///
/// The tooltip reads top to bottom in the order the strips are stacked, which
/// is the only thing naming them: the icon has no room for letters. Both come
/// from this one list so they cannot drift apart.
pub fn readings(metrics: &Metrics, shown: Shown) -> Vec<(&'static str, f32)> {
    bars(metrics, shown)
        .into_iter()
        .map(|(label, percent, _)| (label, percent))
        .collect()
}

/// The pixel height of each bar: what the icon would actually look like.
///
/// The caller compares this with the last one drawn and skips the rebuild
/// when it has not moved. Quantising at the pixel rather than the percent is
/// what makes an idle machine free: a CPU jittering between 0% and 2% maps to
/// the same zero-height bar, so nothing is redrawn.
pub fn signature(metrics: &Metrics, shown: Shown) -> Vec<i32> {
    let size = icon_size();
    bars(metrics, shown)
        .iter()
        .map(|(_, percent, _)| ((percent.clamp(0.0, 100.0) / 100.0) * size as f32).round() as i32)
        .collect()
}

/// Draw the tray icon for the current reading.
///
/// One horizontal strip per measurement, filling left to right. A length
/// along the full width reads far better at 16 px than the height of a
/// 2-pixel-wide column, which is what this replaced.
///
/// Falls back to the static logo when no measurement is selected, so the icon
/// never becomes an empty square the user cannot find.
pub fn metric_icon(metrics: &Metrics, shown: Shown) -> HICON {
    let bars = bars(metrics, shown);
    if bars.is_empty() {
        return logo_icon();
    }
    let size = icon_size();
    let Some(mut canvas) = Canvas::new(size) else {
        return HICON::default();
    };

    let count = bars.len() as i32;
    let pitch = (size / count).max(1);
    let top = (size - pitch * count) / 2;

    for (index, (_, percent, hue)) in bars.iter().enumerate() {
        let y = top + index as i32 * pitch;
        canvas.rect(0, y, size, pitch, TRACK, TRACK_ALPHA[index % 2]);

        let color = rgb_parts(alert_color(*percent, *hue));
        let exact = (percent.clamp(0.0, 100.0) / 100.0) * size as f32;
        let whole = exact.floor() as i32;
        if whole > 0 {
            canvas.rect(0, y, whole, pitch, color, 0xFF);
        }
        // Antialias the tip by hand. Sixteen whole pixels is a coarse scale
        // for a percentage, and a part-lit end pixel reads as a half step
        // without costing any width.
        let fraction = exact - whole as f32;
        if whole < size && fraction > 0.05 {
            canvas.rect(whole, y, 1, pitch, color, (fraction * 255.0) as u8);
        }
    }
    canvas.into_icon()
}

/// Four bars of rising height: the mark, for when there is nothing to show.
pub fn logo_icon() -> HICON {
    let size = icon_size();
    let Some(mut canvas) = Canvas::new(size) else {
        return HICON::default();
    };
    let count = 4;
    let gap = (size / 12).max(1);
    let width = ((size - gap * (count - 1)) / count).max(1);
    let start = (size - (width * count + gap * (count - 1))) / 2;
    for index in 0..count {
        let height = (size * (index + 2)) / (count + 1);
        canvas.rect(
            start + index * (width + gap),
            size - height,
            width,
            height,
            LOGO,
            0xFF,
        );
    }
    canvas.into_icon()
}
