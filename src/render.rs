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
use windows::Win32::Foundation::{COLORREF, POINT, RECT, SIZE};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreatePen, CreateSolidBrush,
    DeleteDC, DeleteObject, DrawTextW, FillRect, GetDC, GetTextExtentPoint32W, Polygon,
    Polyline, ReleaseDC, RoundRect,
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
const TEXT_DIM: COLORREF = rgb(0x8E, 0x8E, 0x93);
const SEPARATOR: COLORREF = rgb(0x38, 0x38, 0x3A);

pub const CPU_HUE: COLORREF = rgb(0x4C, 0x9A, 0xFF);
pub const MEMORY_HUE: COLORREF = rgb(0xA7, 0x8B, 0xFA);
pub const GPU_HUE: COLORREF = rgb(0x34, 0xD3, 0x99);
pub const NPU_HUE: COLORREF = rgb(0x22, 0xD3, 0xEE);
pub const DISK_HUE: COLORREF = rgb(0x94, 0xA3, 0xB8);

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

/// The red, green and blue of a `COLORREF`, for code that builds pixels by
/// hand rather than asking GDI to draw.
pub fn rgb_parts(color: COLORREF) -> (u8, u8, u8) {
    let (r, g, b) = channels(color);
    (r as u8, g as u8, b as u8)
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
    pub const BUTTON: i32 = 20;
    pub const LABEL_WIDTH: i32 = 40;
    pub const VALUE_WIDTH: i32 = 46;
    /// After the "Disk space" label, before the first drive.
    pub const LABEL_GAP: i32 = 12;
    /// Between one drive's percentage and the next drive's letter.
    pub const TOKEN_GAP: i32 = 12;
    /// Between a drive letter and its percentage.
    pub const PAIR_GAP: i32 = 5;
}

/// Which measurements the window draws, after the user's choice has been
/// resolved against the hardware that exists (GLINT-KPI-TOGGLE).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shown {
    pub cpu: bool,
    pub memory: bool,
    pub gpu: bool,
    pub npu: bool,
    pub disk_activity: bool,
    pub disk_space: bool,
}

impl Shown {
    pub fn resolve(
        show: &crate::config::Show,
        has_gpu: bool,
        has_npu: bool,
        has_volumes: bool,
    ) -> Self {
        Self {
            cpu: show.cpu,
            memory: show.memory,
            gpu: show.gpu && has_gpu,
            npu: show.npu && has_npu,
            disk_activity: show.disk_activity,
            disk_space: show.disk_space && has_volumes,
        }
    }

    pub fn any(&self) -> bool {
        self.cpu || self.memory || self.gpu || self.npu || self.disk_activity || self.disk_space
    }
}

/// Which rows the window shows, and how large that makes it (GLINT-FIT).
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub dpi: u32,
    pub width: i32,
    pub height: i32,
    /// The minimize button, which hides the window (GLINT-MINIMIZE).
    pub minimize_rect: RECT,
    /// The bottom of the header strip. A drag starts above this line.
    pub header_bottom: i32,
    pub shown: Shown,
}

fn scale(value: i32, dpi: u32) -> i32 {
    (value * dpi as i32) / 96
}

impl Layout {
    /// `disk_line_width` is the measured width of the drive line, which is the
    /// one piece of content that can need more than the natural width.
    pub fn new(dpi: u32, shown: Shown, disk_line_width: i32) -> Self {
        let pad = scale(design::PAD, dpi);
        let row = scale(design::ROW, dpi);
        let detail = scale(design::DETAIL, dpi);

        let mut width = scale(design::WIDTH, dpi);
        let mut height = pad + scale(design::HEADER, dpi);
        // CPU and memory each carry a detail line; the rest are plain rows.
        if shown.cpu {
            height += row + detail;
        }
        if shown.memory {
            height += row + detail;
        }
        for present in [shown.gpu, shown.npu, shown.disk_activity] {
            if present {
                height += row;
            }
        }
        if shown.disk_space {
            height += row;
            width = width.max(disk_line_width + pad * 2);
        }
        // With nothing selected the window still says how to get a row back.
        if !shown.any() {
            height += row;
        }
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
            shown,
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
    /// Tracked apart from `dpi`: the fonts are built before the back buffer,
    /// so the window can measure the disk line before it knows its own width.
    font_dpi: u32,
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
            font_dpi: 0,
            font_label: HFONT::default(),
            font_value: HFONT::default(),
            font_detail: HFONT::default(),
            scratch: Vec::with_capacity(64),
            points: Vec::with_capacity(crate::sampler::HISTORY + 2),
        }
    }

    /// Build the three fonts for `dpi`, independently of the back buffer.
    fn ensure_fonts(&mut self, dpi: u32) {
        if self.font_dpi == dpi && !self.font_label.is_invalid() {
            return;
        }
        self.release_fonts();
        self.font_label = font(scale(12, dpi), FW_SEMIBOLD.0 as i32);
        self.font_value = font(scale(15, dpi), FW_SEMIBOLD.0 as i32);
        self.font_detail = font(scale(11, dpi), FW_NORMAL.0 as i32);
        self.font_dpi = dpi;
    }

    /// The width the drive line wants, so the window can widen to fit it
    /// (GLINT-DISK-LINE). Percentages are measured as `100%` whatever they
    /// read, so the window does not resize as a drive fills.
    pub fn disk_line_width(&mut self, dpi: u32, volumes: &[crate::sampler::Volume]) -> i32 {
        if volumes.is_empty() {
            return 0;
        }
        self.ensure_fonts(dpi);
        let screen = unsafe { GetDC(None) };
        let font = self.font_label;
        let mut total = text_width(screen, font, "Disk space", &mut self.scratch);
        total += scale(design::LABEL_GAP, dpi);
        let percent = text_width(screen, font, "100%", &mut self.scratch);
        for (index, volume) in volumes.iter().enumerate() {
            if index > 0 {
                total += scale(design::TOKEN_GAP, dpi);
            }
            total += text_width(screen, font, &volume.label, &mut self.scratch);
            total += scale(design::PAIR_GAP, dpi) + percent;
        }
        unsafe { ReleaseDC(None, screen) };
        total
    }

    /// Make sure the buffer and the fonts match the current size and DPI.
    fn prepare(&mut self, layout: &Layout) {
        self.ensure_fonts(layout.dpi);
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
        self.dpi = layout.dpi;
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
        self.font_dpi = 0;
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
        let shown = layout.shown;

        if shown.cpu {
            y = self.draw_row(
                layout,
                &Row {
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
                y,
            );
        }
        if shown.memory {
            y = self.draw_row(
                layout,
                &Row {
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
                y,
            );
        }
        if shown.gpu {
            y = self.draw_row(
                layout,
                &Row {
                    label: "GPU",
                    percent: metrics.gpu.unwrap_or(0.0),
                    hue: GPU_HUE,
                    history: &history.gpu,
                    detail: None,
                },
                y,
            );
        }
        // GLINT-NPU-OPTIONAL: no NPU, no row, and the window is shorter.
        if shown.npu {
            y = self.draw_row(
                layout,
                &Row {
                    label: "NPU",
                    percent: metrics.npu.unwrap_or(0.0),
                    hue: NPU_HUE,
                    history: &history.npu,
                    detail: None,
                },
                y,
            );
        }
        if shown.disk_activity {
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
        }
        if shown.disk_space {
            self.draw_disk_line(layout, metrics, y);
        }
        if !shown.any() {
            let prompt = RECT {
                left: layout.scale(design::PAD),
                top: y,
                right: layout.width - layout.scale(design::PAD),
                bottom: y + layout.scale(design::ROW),
            };
            let font = self.font_detail;
            self.text(
                "Right click to choose measurements",
                &prompt,
                TEXT_DIM,
                font,
                DT_LEFT,
            );
        }

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

    /// Every drive on one line: `Disk space   C: 95%   Z: 41%`
    /// (GLINT-DISK-LINE). No separator above it, no bars, and the same font
    /// size as the rest of the body.
    fn draw_disk_line(&mut self, layout: &Layout, metrics: &Metrics, top: i32) {
        let height = layout.scale(design::ROW);
        let font = self.font_label;
        let dc = self.memory_dc;
        let mut x = layout.scale(design::PAD);

        let label_width = text_width(dc, font, "Disk space", &mut self.scratch);
        let label = RECT {
            left: x,
            top,
            right: x + label_width,
            bottom: top + height,
        };
        self.text("Disk space", &label, TEXT_DIM, font, DT_LEFT);
        x = label.right + layout.scale(design::LABEL_GAP);

        // A fixed percentage box keeps the columns still as the numbers move.
        let percent_width = text_width(dc, font, "100%", &mut self.scratch);
        for (index, volume) in metrics.volumes.iter().enumerate() {
            if index > 0 {
                x += layout.scale(design::TOKEN_GAP);
            }
            let drive_width = text_width(dc, font, &volume.label, &mut self.scratch);
            let drive = RECT {
                left: x,
                top,
                right: x + drive_width,
                bottom: top + height,
            };
            self.text(&volume.label, &drive, DISK_HUE, font, DT_LEFT);
            x = drive.right + layout.scale(design::PAIR_GAP);

            let percent = volume.percent();
            let box_ = RECT {
                left: x,
                top,
                right: x + percent_width,
                bottom: top + height,
            };
            self.text(
                &format!("{percent:.0}%"),
                &box_,
                alert_color(percent, DISK_HUE),
                font,
                DT_RIGHT,
            );
            x = box_.right;
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

/// The width `text` needs in `font`, for laying the disk line out by hand.
fn text_width(dc: HDC, font: HFONT, text: &str, scratch: &mut Vec<u16>) -> i32 {
    scratch.clear();
    scratch.extend(text.encode_utf16());
    let mut size = SIZE::default();
    unsafe {
        let old = SelectObject(dc, font.into());
        let _ = GetTextExtentPoint32W(dc, scratch, &mut size);
        SelectObject(dc, old);
    }
    size.cx
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
