use std::sync::atomic::{AtomicUsize, Ordering};

use native_windows_gui as nwg;
use winapi::shared::windef::RECT;
use winapi::um::wingdi::{CreateSolidBrush, RGB};
use winapi::um::winuser::{FillRect, GetClientRect, WM_ERASEBKGND};

/// `bind_raw_event_handler` requires an id greater than `0xFFFF` (smaller ones are reserved
/// by NWG); each call to `paint` needs its own so multiple frames can each have a handler.
static NEXT_HANDLER_ID: AtomicUsize = AtomicUsize::new(0x1_0000);

/// Paints `frame`'s background with a solid color.
///
/// `nwg::Frame` is NWG's own `"NWG_FRAME"` window class, registered once for every Frame
/// instance with no background brush, and — unlike standard controls (Label, ImageFrame,
/// etc.) — it never sends `WM_CTLCOLORSTATIC` to its parent, so there's no builder option for
/// this. Answering `WM_ERASEBKGND` on the frame's own handle is the only way to color it.
///
/// The bound handler (and the brush it uses) is intentionally never released: the frames this
/// is called on live for the lifetime of the process, the same as `App` itself.
pub fn paint(frame: &nwg::Frame, color: [u8; 3]) {
    let brush = unsafe { CreateSolidBrush(RGB(color[0], color[1], color[2])) };
    let handler_id = NEXT_HANDLER_ID.fetch_add(1, Ordering::Relaxed);

    nwg::bind_raw_event_handler(&frame.handle, handler_id, move |hwnd, msg, wparam, _lparam| {
        if msg != WM_ERASEBKGND {
            return None;
        }

        unsafe {
            let mut rect: RECT = std::mem::zeroed();
            GetClientRect(hwnd, &mut rect);
            FillRect(wparam as _, &rect, brush);
        }
        Some(1)
    })
    .expect("Failed to bind panel background paint handler");
}
