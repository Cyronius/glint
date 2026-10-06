//! GDI drawing with a double buffer.
//!
//! The plan rules out Direct2D and Direct3D on purpose. A Direct2D render
//! target creates a D3D device, which costs RAM and keeps the GPU awake. Five
//! rows and five sparklines do not need a GPU.
//!
//! The renderer owns one memory device context and one compatible bitmap. It
//! re-creates them only when the size or the DPI changes, so a redraw
//! allocates nothing.

use windows::core::PCWSTR;
use windows::Win32::Foundation::{COLORREF, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreatePen, CreateSolidBrush,
    DeleteDC, DeleteObject, DrawTextW, FillRect, GetDC, Polygon, Polyline, ReleaseDC, RoundRect,
    SelectObject, SetBkMode, SetTextColor, CLEARTYPE_QUALITY, DEFAULT_CHARSET, DT_LEFT,
    DT_RIGHT, DT_SINGLELINE, DT_VCENTER, FF_DONTCARE, FW_NORMAL, FW_SEMIBOLD, HBITMAP,
    HDC, HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS, PS_SOLID, SRCCOPY, TRANSPARENT, VARIABLE_PITCH,
};

use crate::sampler::{History, Metrics};

// ------------------------------------------------------------ the palette ----

/// Build a `COLORREF` from red, green and blue. Win32 orders the bytes
/// as blue, green, red, which is the reverse of the usual hex notation.
const fn rgb(red: u8, green: u8, blue: u8) -> COLORREF {
    COLORREF((red as u32) | ((green as u32) << 8) | ((blue as u32) << 16))
}

const BACKGROUND: COLORREF = rgb(0x1C, 0x1C, 0x1E);
const TEXT: COLORREF = rgb(0xF2, 0xF2, 0xF7);
const TEXT_DIM: COLORREF = rgb(0x8E, 0x8E, 0x93);
const SEPARATOR: COLORREF = rgb(0x38, 0x38, 0x3A);

const CPU_HUE: COLORREF = rgb(0x4C, 0x9A, 0xFF);
const MEMORY_HUE: COLORREF = rgb(0xA7, 0x8B, 0xFA);
const GPU_HUE: COLORREF = rgb(0x34, 0xD3, 0x99);
const NPU_HUE: COLORREF = rgb(0x22, 0xD3, 0xEE);
const DISK_HUE: COLORREF = rgb(0x94, 0xA3, 0xB8);

/// GLINT-ALERT-COLOR thresholds.
const AMBER: COLORREF = rgb(0xF5, 0x9E, 0x0B);
const RED: COLORREF = rgb(0xEF, 0x44, 0x44);
pub const AMBER_AT: f32 = 80.0;
pub const RED_AT: f32 = 95.0;

/// The colour a row takes at `percent` (GLINT-ALERT-COLOR).
pub fn alert_color(percent: f32, hue: COLORREF) -> COLORREF {
    if percent >= RED_AT {
        RED
    } else if percent >= AMBER_AT {
        AMBER
    } else {
        hue
    }
}

fn channels(color: COLORREF) -> (u32, u32, u32) {
    (color.0 & 0xFF, (color.0 >> 8) & 0xFF, (color.0 >> 16) & 0xFF)
}

/// Mix `color` into `onto` at `numerator/denominator`.
///
/// GDI has no alpha blend for these shapes, and it needs none: the background
/// is a known solid colour, so a pre-mixed shade is exactly what an alpha
/// blend would produce.
fn mix(color: COLORREF, onto: COLORREF, numerator: u32, denominator: u32) -> COLORREF {
    let (r1, g1, b1) = channels(color);
    let (r2, g2, b2) = channels(onto);
    let blend = |a: u32, b: u32| {
        ((a * numerator + b * (denominator - numerator)) / denominator).min(255) as u8
    };
    rgb(blend(r1, r2), blend(g1, g2), blend(b1, b2))
}

// ------------------------------------------------------------- the layout ----

/// Design sizes in pixels at 96 DPI. `Layout` scales them.
mod design {
    pub const WIDTH: i32 = 300;
    pub const PAD: i32 = 14;
    pub const HEADER: i32 = 24;
    pub const ROW: i32 = 26;
    pub const DETAIL: i32 = 15;
    pub const SPARK_WIDTH: i32 = 104;
    pub const SPARK_HEIGHT: i32 = 18;
    pub const SEPARATOR: i32 = 9;
    pub const VOLUME: i32 = 21;
    pub const BUTTON: i32 = 20;
    pub const LABEL_WIDTH: i32 = 40;
    pub const VALUE_WIDTH: i32 = 46;
}

/// Which rows the window shows, and how tall that makes it.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub dpi: u32,
    pub width: i32,
    pub height: i32,
    /// The minimize button, which hides the window (GLINT-MINIMIZE).
    pub minimize_rect: RECT,
    /// The bottom of the header strip. A drag starts above this line.
    pub header_bottom: i32,
    /// The "Disk space" title above the drive list.
    disk_title_rect: RECT,
    volumes: usize,
}

fn scale(value: i32, dpi: u32) -> i32 {
    (value * dpi as i32) / 96
}

impl Layout {
    pub fn new(dpi: u32, has_npu: bool, volumes: usize) -> Self {
        let pad = scale(design::PAD, dpi);
        let width = scale(design::WIDTH, dpi);

        // CPU and memory each carry a detail line. The plain rows are GPU,
        // the NPU when there is one, and disk activity.
        let detail_rows = 2;
        let plain_rows = 1 + usize::from(has_npu) + 1;
        let mut height = pad + scale(design::HEADER, dpi);
        height += detail_rows * (scale(design::ROW, dpi) + scale(design::DETAIL, dpi));
        height += plain_rows as i32 * scale(design::ROW, dpi);
        height += scale(design::SEPARATOR, dpi);
        let title_top = height;
        height += scale(design::ROW, dpi);
        height += volumes as i32 * scale(design::VOLUME, dpi);
        height += pad;

        // The minimize button sits at the far right as Windows places it.
        let button = scale(design::BUTTON, dpi);
        let header_top = pad;
        let minimize_right = width - pad;
        Self {
            dpi,
            width,
            height,
            minimize_rect: RECT {
                left: minimize_right - button,
                top: header_top,
                right: minimize_right,
                bottom: header_top + button,
            },
            header_bottom: header_top + button,
            disk_title_rect: RECT {
                left: pad,
                top: title_top,
                right: width - pad,
                bottom: title_top + scale(design::ROW, dpi),
            },
            volumes,
        }
    }

    fn scale(&self, value: i32) -> i32 {
        scale(value, self.dpi)
    }
}

pub fn contains(rect: &RECT, point: POINT) -> bool {
    point.x >= rect.left && point.x < rect.right && point.y >= rect.top && point.y < rect.bottom
}

// ----------------------------------------------------------- the renderer ----

/// One row of the panel.
struct Row<'a> {
    label: &'a str,
    percent: f32,
    hue: COLORREF,
    history: &'a History,
    detail: Option<String>,
}

pub struct Renderer {
    memory_dc: HDC,
    bitmap: HBITMAP,
    previous_bitmap: HGDIOBJ,
    size: (i32, i32),
    dpi: u32,
    font_label: HFONT,
    font_value: HFONT,
    font_detail: HFONT,
    /// Scratch for `DrawTextW`, which needs UTF-16.
    scratch: Vec<u16>,
    /// Scratch for sparkline points.
    points: Vec<POINT>,
}

impl Renderer {
    pub fn new() -> Self {
        Self {
            memory_dc: HDC::default(),
            bitmap: HBITMAP::default(),
            previous_bitmap: HGDIOBJ::default(),
            size: (0, 0),
            dpi: 0,
            font_label: HFONT::default(),
            font_value: HFONT::default(),
            font_detail: HFONT::default(),
            scratch: Vec::with_capacity(64),
            points: Vec::with_capacity(crate::sampler::HISTORY + 2),
        }
    }

    /// Make sure the buffer and the fonts match the current size and DPI.
    fn prepare(&mut self, layout: &Layout) {
        if self.size == (layout.width, layout.height) && self.dpi == layout.dpi {
            return;
        }
        self.release_buffer();
        let screen = unsafe { GetDC(None) };
        self.memory_dc = unsafe { CreateCompatibleDC(Some(screen)) };
        self.bitmap = unsafe { CreateCompatibleBitmap(screen, layout.width, layout.height) };
        unsafe { ReleaseDC(None, screen) };
        self.previous_bitmap = unsafe { SelectObject(self.memory_dc, self.bitmap.into()) };
        self.size = (layout.width, layout.height);

        if self.dpi != layout.dpi {
            self.release_fonts();
            self.font_label = font(scale(12, layout.dpi), FW_SEMIBOLD.0 as i32);
            self.font_value = font(scale(15, layout.dpi), FW_SEMIBOLD.0 as i32);
            self.font_detail = font(scale(11, layout.dpi), FW_NORMAL.0 as i32);
            self.dpi = layout.dpi;
        }
    }

    fn release_buffer(&mut self) {
        unsafe {
            if !self.memory_dc.is_invalid() {
                if !self.previous_bitmap.is_invalid() {
                    SelectObject(self.memory_dc, self.previous_bitmap);
                }
                let _ = DeleteDC(self.memory_dc);
            }
            if !self.bitmap.is_invalid() {
                let _ = DeleteObject(self.bitmap.into());
            }
        }
        self.memory_dc = HDC::default();
        self.bitmap = HBITMAP::default();
        self.previous_bitmap = HGDIOBJ::default();
        self.size = (0, 0);
    }

    fn release_fonts(&mut self) {
        for handle in [self.font_label, self.font_value, self.font_detail] {
            if !handle.is_invalid() {
                unsafe {
                    let _ = DeleteObject(handle.into());
                }
            }
        }
        self.font_label = HFONT::default();
        self.font_value = HFONT::default();
        self.font_detail = HFONT::default();
    }

    /// Draw the whole panel into the buffer, then copy it to the window in one
    /// `BitBlt`. One copy means no flicker, so `WM_ERASEBKGND` can do nothing.
    pub fn paint(
        &mut self,
        target: HDC,
        layout: &Layout,
        metrics: &Metrics,
        history: &crate::sampler::Histories,
    ) {
        self.prepare(layout);
        if self.memory_dc.is_invalid() {
            return;
        }
        let dc = self.memory_dc;
        unsafe { SetBkMode(dc, TRANSPARENT) };

        let whole = RECT {
            left: 0,
            top: 0,
            right: layout.width,
            bottom: layout.height,
        };
        self.fill(&whole, BACKGROUND);

        self.draw_header(layout);
        let mut y = layout.scale(design::PAD) + layout.scale(design::HEADER);

        let rows = [
            Row {
                label: "CPU",
                percent: metrics.cpu,
                hue: CPU_HUE,
                history: &history.cpu,
                // GLINT-CPU-BOOST: the clock and the ratio to the base clock.
                detail: Some(format!(
                    "{:.2} GHz  ({:.2}x base)",
                    metrics.cpu_ghz, metrics.cpu_ratio
                )),
            },
            Row {
                label: "MEM",
                percent: metrics.memory,
                hue: MEMORY_HUE,
                history: &history.memory,
                detail: Some(format!(
                    "{:.1} / {:.1} GiB",
                    gib(metrics.memory_used_bytes),
                    gib(metrics.memory_total_bytes)
                )),
            },
        ];
        for row in &rows {
            y = self.draw_row(layout, row, y);
        }

        if let Some(gpu) = metrics.gpu {
            y = self.draw_row(
                layout,
                &Row {
                    label: "GPU",
                    percent: gpu,
                    hue: GPU_HUE,
                    history: &history.gpu,
                    detail: None,
                },
                y,
            );
        }
        // GLINT-NPU-OPTIONAL: no NPU, no row, and the window is shorter.
        if let Some(npu) = metrics.npu {
            y = self.draw_row(
                layout,
                &Row {
                    label: "NPU",
                    percent: npu,
                    hue: NPU_HUE,
                    history: &history.npu,
                    detail: None,
                },
                y,
            );
        }
        y = self.draw_row(
            layout,
            &Row {
                label: "DISK",
                percent: metrics.disk,
                hue: DISK_HUE,
                history: &history.disk,
                detail: None,
            },
            y,
        );

        // The separator sits in the middle of its band.
        let band = layout.scale(design::SEPARATOR);
        let line = RECT {
            left: layout.scale(design::PAD),
            top: y + band / 2,
            right: layout.width - layout.scale(design::PAD),
            bottom: y + band / 2 + 1,
        };
        self.fill(&line, SEPARATOR);

        self.draw_disk_section(layout, metrics);

        unsafe {
            let _ = BitBlt(
                target,
                0,
                0,
                layout.width,
                layout.height,
                Some(dc),
                0,
                0,
                SRCCOPY,
            );
        }
    }

    fn draw_header(&mut self, layout: &Layout) {
        let pad = layout.scale(design::PAD);
        let header = RECT {
            left: pad,
            top: pad,
            right: layout.minimize_rect.left - pad / 2,
            bottom: pad + layout.scale(design::HEADER),
        };
        self.text("Glint", &header, TEXT_DIM, self.font_label, DT_LEFT);
        self.draw_minimize(layout);
    }

    /// A single bar. Clicking it hides the window (GLINT-MINIMIZE).
    fn draw_minimize(&mut self, layout: &Layout) {
        let rect = layout.minimize_rect;
        self.rounded(&rect, BACKGROUND, SEPARATOR);
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        let bar = RECT {
            left: rect.left + width / 4,
            top: rect.top + height / 2,
            right: rect.right - width / 4,
            bottom: rect.top + height / 2 + (height / 10).max(1),
        };
        self.fill(&bar, TEXT_DIM);
    }

    /// Draw one resource row and return the y of the next row.
    fn draw_row(&mut self, layout: &Layout, row: &Row<'_>, top: i32) -> i32 {
        let pad = layout.scale(design::PAD);
        let row_height = layout.scale(design::ROW);
        let color = alert_color(row.percent, row.hue);

        let label = RECT {
            left: pad,
            top,
            right: pad + layout.scale(design::LABEL_WIDTH),
            bottom: top + row_height,
        };
        self.text(row.label, &label, row.hue, self.font_label, DT_LEFT);

        let value = RECT {
            left: label.right,
            top,
            right: label.right + layout.scale(design::VALUE_WIDTH),
            bottom: top + row_height,
        };
        let percent = format!("{:.0}%", row.percent);
        self.text(&percent, &value, color, self.font_value, DT_RIGHT);

        let spark_height = layout.scale(design::SPARK_HEIGHT);
        let spark = RECT {
            left: layout.width - pad - layout.scale(design::SPARK_WIDTH),
            top: top + (row_height - spark_height) / 2,
            right: layout.width - pad,
            bottom: top + (row_height - spark_height) / 2 + spark_height,
        };
        self.draw_sparkline(&spark, row.history, color);

        let mut next = top + row_height;
        if let Some(detail) = &row.detail {
            let detail_height = layout.scale(design::DETAIL);
            let box_ = RECT {
                left: pad,
                top: next - layout.scale(2),
                right: layout.width - pad,
                bottom: next - layout.scale(2) + detail_height,
            };
            self.text(detail, &box_, TEXT_DIM, self.font_detail, DT_LEFT);
            next += detail_height;
        }
        next
    }

    /// A filled area chart with a bright top line.
    ///
    /// The ceiling is a fixed 100%, and every value is already clamped, so the
    /// shape stays comparable from one minute to the next.
    fn draw_sparkline(&mut self, rect: &RECT, history: &History, color: COLORREF) {
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if width <= 2 || height <= 2 {
            return;
        }

        // A baseline, so an idle row still reads as a row.
        let baseline = RECT {
            left: rect.left,
            top: rect.bottom - 1,
            right: rect.right,
            bottom: rect.bottom,
        };
        self.fill(&baseline, SEPARATOR);
        if history.is_empty() {
            return;
        }

        let count = history.len();
        let step = if count > 1 {
            width as f32 / (crate::sampler::HISTORY - 1) as f32
        } else {
            0.0
        };
        // Anchor the newest sample at the right edge, so the line grows
        // leftward until a full minute of history exists.
        let first_x = rect.right as f32 - step * (count - 1) as f32;

        self.points.clear();
        for (index, value) in history.iter().enumerate() {
            let x = (first_x + step * index as f32).round() as i32;
            let span = (height - 2) as f32;
            let y = rect.bottom - 1 - ((value.clamp(0.0, 100.0) / 100.0) * span).round() as i32;
            self.points.push(POINT { x, y });
        }

        if self.points.len() >= 2 {
            let fill = mix(color, BACKGROUND, 1, 4);
            let brush = unsafe { CreateSolidBrush(fill) };
            let pen = unsafe { CreatePen(PS_SOLID, 1, fill) };
            // Close the shape along the baseline to fill under the line.
            let last = *self.points.last().unwrap();
            let first = self.points[0];
            self.points.push(POINT {
                x: last.x,
                y: rect.bottom - 1,
            });
            self.points.push(POINT {
                x: first.x,
                y: rect.bottom - 1,
            });
            unsafe {
                let old_brush = SelectObject(self.memory_dc, brush.into());
                let old_pen = SelectObject(self.memory_dc, pen.into());
                let _ = Polygon(self.memory_dc, &self.points);
                SelectObject(self.memory_dc, old_brush);
                SelectObject(self.memory_dc, old_pen);
                let _ = DeleteObject(brush.into());
                let _ = DeleteObject(pen.into());
            }
            self.points.truncate(self.points.len() - 2);

            let pen = unsafe { CreatePen(PS_SOLID, layout_pen_width(height), color) };
            unsafe {
                let old_pen = SelectObject(self.memory_dc, pen.into());
                let _ = Polyline(self.memory_dc, &self.points);
                SelectObject(self.memory_dc, old_pen);
                let _ = DeleteObject(pen.into());
            }
        }
    }

    fn draw_disk_section(&mut self, layout: &Layout, metrics: &Metrics) {
        let title = layout.disk_title_rect;
        self.text("Disk space", &title, TEXT_DIM, self.font_label, DT_LEFT);

        let pad = layout.scale(design::PAD);
        let height = layout.scale(design::VOLUME);
        for (index, volume) in metrics.volumes.iter().take(layout.volumes).enumerate() {
            let top = title.bottom + index as i32 * height;
            let label = RECT {
                left: pad,
                top,
                right: pad + layout.scale(30),
                bottom: top + height,
            };
            self.text(&volume.label, &label, TEXT, self.font_detail, DT_LEFT);

            let percent = volume.percent();
            let color = alert_color(percent, DISK_HUE);
            let text = RECT {
                left: label.right,
                top,
                right: label.right + layout.scale(design::VALUE_WIDTH),
                bottom: top + height,
            };
            self.text(
                &format!("{percent:.0}%"),
                &text,
                color,
                self.font_detail,
                DT_RIGHT,
            );

            let bar_height = layout.scale(6).max(3);
            let track = RECT {
                left: layout.width - pad - layout.scale(design::SPARK_WIDTH),
                top: top + (height - bar_height) / 2,
                right: layout.width - pad,
                bottom: top + (height - bar_height) / 2 + bar_height,
            };
            self.fill(&track, SEPARATOR);
            let span = track.right - track.left;
            let filled = RECT {
                right: track.left + (span as f32 * percent / 100.0).round() as i32,
                ..track
            };
            self.fill(&filled, color);
        }
    }

    // ------------------------------------------------------------ helpers ----

    fn fill(&self, rect: &RECT, color: COLORREF) {
        unsafe {
            let brush = CreateSolidBrush(color);
            FillRect(self.memory_dc, rect, brush);
            let _ = DeleteObject(brush.into());
        }
    }

    fn rounded(&self, rect: &RECT, fill: COLORREF, border: COLORREF) {
        let radius = ((rect.bottom - rect.top) / 3).max(2);
        unsafe {
            let brush = CreateSolidBrush(fill);
            let pen = CreatePen(PS_SOLID, 1, border);
            let old_brush = SelectObject(self.memory_dc, brush.into());
            let old_pen = SelectObject(self.memory_dc, pen.into());
            let _ = RoundRect(
                self.memory_dc,
                rect.left,
                rect.top,
                rect.right,
                rect.bottom,
                radius,
                radius,
            );
            SelectObject(self.memory_dc, old_brush);
            SelectObject(self.memory_dc, old_pen);
            let _ = DeleteObject(brush.into());
            let _ = DeleteObject(pen.into());
        }
    }

    fn text(
        &mut self,
        text: &str,
        rect: &RECT,
        color: COLORREF,
        font: HFONT,
        align: windows::Win32::Graphics::Gdi::DRAW_TEXT_FORMAT,
    ) {
        self.scratch.clear();
        self.scratch.extend(text.encode_utf16());
        if self.scratch.is_empty() {
            return;
        }
        let mut box_ = *rect;
        unsafe {
            SetTextColor(self.memory_dc, color);
            let old = SelectObject(self.memory_dc, font.into());
            DrawTextW(
                self.memory_dc,
                &mut self.scratch,
                &mut box_,
                align | DT_SINGLELINE | DT_VCENTER,
            );
            SelectObject(self.memory_dc, old);
        }
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        self.release_buffer();
        self.release_fonts();
    }
}

impl Default for Renderer {
    fn default() -> Self {
        Self::new()
    }
}

fn layout_pen_width(height: i32) -> i32 {
    if height >= 24 {
        2
    } else {
        1
    }
}

fn gib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0 * 1024.0)
}

fn font(height: i32, weight: i32) -> HFONT {
    let face: Vec<u16> = "Segoe UI".encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        CreateFontW(
            -height,
            0,
            0,
            0,
            weight,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            windows::Win32::Graphics::Gdi::CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            (VARIABLE_PITCH.0 | FF_DONTCARE.0) as u32,
            PCWSTR(face.as_ptr()),
        )
    }
}
