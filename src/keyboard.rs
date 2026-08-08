/// Whether the given virtual key (e.g. `VK_CONTROL`) is currently held down. Shared by
/// `App::on_key_press` and `ImageViewer::on_key_press` — `OnKeyPress`'s `EventData` carries
/// no modifier state, so both read it directly via `GetKeyState`.
pub fn key_down(vk: winapi::ctypes::c_int) -> bool {
    unsafe { winapi::um::winuser::GetKeyState(vk) & 0x8000u16 as i16 != 0 }
}
