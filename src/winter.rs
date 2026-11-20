#![allow(unsafe_op_in_unsafe_fn)]
use std::time::Instant;
use windows_sys::Win32::{Foundation::POINT, Graphics::Gdi::*};
const W: usize = 176;
const H: usize = 112;
fn rgb(r: u32, g: u32, b: u32) -> u32 {
    r | (g << 8) | (b << 16)
}
pub struct Winter {
    pixels: Vec<u32>,
    elapsed: f32,
    since: Instant,
    running: bool,
}
impl Winter {
    pub fn new() -> Self {
        Self {
            pixels: vec![0; W * H],
            elapsed: 0.0,
            since: Instant::now(),
            running: false,
        }
    }
    pub fn running(&mut self, on: bool) {
        if self.running == on {
            return;
        }
        if self.running {
            self.elapsed += self.since.elapsed().as_secs_f32();
        }
        self.since = Instant::now();
        self.running = on;
    }
    fn time(&self) -> f32 {
        self.elapsed
            + if self.running {
                self.since.elapsed().as_secs_f32()
            } else {
                0.0
            }
    }
    pub unsafe fn draw(&mut self, dc: HDC, width: i32, height: i32, count: u16) {
        let t = self.time();
        for x in 0..W {
            let u = x as f32 / W as f32;
            let a = 0.19 + 0.10 * (u * 5.0 + t * 0.08).sin() + 0.04 * (u * 11.0 - t * 0.11).sin();
            let b = 0.34 + 0.12 * (u * 4.5 - t * 0.06).sin();
            for y in 0..H {
                let v = y as f32 / H as f32;
                let glow = (1.0 - ((v - a) / 0.16).abs()).max(0.0).powi(2) * (0.4 + 0.6 * u);
                let violet = (1.0 - ((v - b) / 0.22).abs()).max(0.0).powi(2) * u;
                let r = (8.0 + glow * 6.0 + violet * 15.0) as u32;
                let g = (19.0 + glow * 32.0 + violet * 7.0) as u32;
                let bl = (22.0 + glow * 21.0 + violet * 23.0) as u32;
                self.pixels[y * W + x] = (r << 16) | (g << 8) | bl;
            }
        }
        let mut info: BITMAPINFO = std::mem::zeroed();
        info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        info.bmiHeader.biWidth = W as i32;
        info.bmiHeader.biHeight = -(H as i32);
        info.bmiHeader.biPlanes = 1;
        info.bmiHeader.biBitCount = 32;
        info.bmiHeader.biCompression = BI_RGB;
        let old = SetStretchBltMode(dc, HALFTONE);
        SetBrushOrgEx(dc, 0, 0, std::ptr::null_mut());
        StretchDIBits(
            dc,
            0,
            0,
            width,
            height,
            0,
            0,
            W as i32,
            H as i32,
            self.pixels.as_ptr() as _,
            &info,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
        SetStretchBltMode(dc, old);
        let colors = [rgb(102, 133, 137), rgb(164, 192, 194), rgb(220, 239, 235)];
        let brushes = colors.map(|c| CreateSolidBrush(c));
        let original = SelectObject(dc, GetStockObject(NULL_PEN));
        for i in 0..usize::from(count.min(240)) {
            let depth = (i % 3) as f32;
            let seed = i as f32 * 2.39996;
            let x = 210.0
                + ((seed * 137.0 + t * (2.0 + depth) + 8.0 * (t * 0.3 + seed).sin())
                    .rem_euclid((width - 210).max(1) as f32));
            let y = (seed * 91.0 + t * (8.0 + depth * 6.0)).rem_euclid(height.max(1) as f32);
            let size = [1, 2, 2][i % 3];
            let old = SelectObject(dc, brushes[i % 3]);
            Ellipse(
                dc,
                x as i32,
                y as i32,
                x as i32 + size + 1,
                y as i32 + size + 1,
            );
            SelectObject(dc, old);
        }
        SelectObject(dc, original);
        for brush in brushes {
            DeleteObject(brush);
        }
    }
    pub unsafe fn lights(&self, dc: HDC, width: i32) {
        let t = self.time();
        let pen = CreatePen(PS_SOLID, 1, rgb(72, 93, 75));
        let old = SelectObject(dc, pen);
        MoveToEx(dc, 230, 111, std::ptr::null_mut());
        LineTo(dc, width - 28, 111);
        SelectObject(dc, old);
        DeleteObject(pen);
        let colors = [
            (255.0, 205.0, 104.0),
            (255.0, 113.0, 121.0),
            (110.0, 239.0, 187.0),
            (142.0, 187.0, 255.0),
        ];
        let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
        for i in 0..16 {
            let x = 244 + (width - 286).max(1) * i / 15;
            let (r, g, b) = colors[i as usize % colors.len()];
            // Slow waves of light; a few GDI circles avoid blur buffers or GPU surfaces.
            let pulse = 0.68 + 0.32 * (t * 1.4 - i as f32 * 0.62).sin();
            for (radius, alpha) in [(10, 0.10), (7, 0.23), (5, 0.48), (3, 1.0)] {
                let level = alpha * pulse;
                let color = rgb(
                    (8.0 + (r - 8.0) * level) as u32,
                    (19.0 + (g - 19.0) * level) as u32,
                    (22.0 + (b - 22.0) * level) as u32,
                );
                let brush = CreateSolidBrush(color);
                let old = SelectObject(dc, brush);
                Ellipse(
                    dc,
                    x - radius,
                    113 - radius,
                    x + radius + 1,
                    113 + radius + 1,
                );
                SelectObject(dc, old);
                DeleteObject(brush);
            }
            let highlight = CreateSolidBrush(rgb(247, 243, 215));
            let old = SelectObject(dc, highlight);
            Ellipse(dc, x - 1, 111, x + 1, 113);
            SelectObject(dc, old);
            DeleteObject(highlight);
        }
        SelectObject(dc, old_pen);
    }
    pub unsafe fn tree(dc: HDC, x: i32, y: i32) {
        let old_pen = SelectObject(dc, GetStockObject(NULL_PEN));
        let green = CreateSolidBrush(rgb(42, 99, 77));
        let gold = CreateSolidBrush(rgb(217, 180, 102));
        let red = CreateSolidBrush(rgb(190, 94, 96));
        let old_brush = SelectObject(dc, green);
        for (offset, half, height) in [(0, 17, 28), (17, 25, 31), (35, 33, 34)] {
            let points = [
                POINT { x, y: y + offset },
                POINT {
                    x: x - half,
                    y: y + offset + height,
                },
                POINT {
                    x: x + half,
                    y: y + offset + height,
                },
            ];
            Polygon(dc, points.as_ptr(), 3);
        }
        SelectObject(dc, gold);
        Rectangle(dc, x - 4, y + 68, x + 5, y + 77);
        let star = [
            POINT { x, y: y - 9 },
            POINT { x: x + 3, y: y - 3 },
            POINT { x: x + 9, y: y - 2 },
            POINT { x: x + 4, y: y + 2 },
            POINT { x: x + 5, y: y + 9 },
            POINT { x, y: y + 5 },
            POINT { x: x - 5, y: y + 9 },
            POINT { x: x - 4, y: y + 2 },
            POINT { x: x - 9, y: y - 2 },
            POINT { x: x - 3, y: y - 3 },
        ];
        Polygon(dc, star.as_ptr(), star.len() as i32);
        for (i, (dx, dy)) in [(-7, 25), (9, 40), (-16, 55), (16, 62), (-2, 59)]
            .iter()
            .enumerate()
        {
            SelectObject(dc, if i % 2 == 0 { gold } else { red });
            Ellipse(dc, x + dx - 2, y + dy - 2, x + dx + 3, y + dy + 3);
        }
        SelectObject(dc, old_brush);
        SelectObject(dc, old_pen);
        for b in [green, gold, red] {
            DeleteObject(b);
        }
    }
}
pub struct Surface {
    pub dc: HDC,
    bitmap: HBITMAP,
    old: HGDIOBJ,
    pub width: i32,
    pub height: i32,
}
impl Surface {
    pub unsafe fn new(dc: HDC, width: i32, height: i32) -> Option<Self> {
        let mem = CreateCompatibleDC(dc);
        if mem.is_null() {
            return None;
        }
        let bitmap = CreateCompatibleBitmap(dc, width.max(1), height.max(1));
        if bitmap.is_null() {
            DeleteDC(mem);
            return None;
        }
        let old = SelectObject(mem, bitmap);
        Some(Self {
            dc: mem,
            bitmap,
            old,
            width,
            height,
        })
    }
}
impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            DeleteObject(self.bitmap);
            DeleteDC(self.dc);
        }
    }
}
