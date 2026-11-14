//! Monitores e reserva de espaço na tela.
//!
//! Reservar espaço: com o painel encostado no topo ou na base, o sistema tira essa faixa da
//! área útil do monitor, e janelas maximizadas param antes do painel.
//! - Windows: o painel vira uma "AppBar" (a mesma API da barra de tarefas).
//! - Linux/X11: janela do tipo Dock com `_NET_WM_STRUT_PARTIAL` (como painéis e docks).
//! - Wayland nativo e macOS: não suportado (o painel só fica por cima).

use winit::event_loop::ActiveEventLoop;
use winit::monitor::MonitorHandle;
use winit::window::Window;

use crate::state::MonitorInfo;

/// Monitores em ordem estável para o usuário: da esquerda para a direita, depois de cima para baixo.
pub fn sorted_monitors(el: &ActiveEventLoop) -> Vec<MonitorHandle> {
    let mut list: Vec<MonitorHandle> = el.available_monitors().collect();
    list.sort_by_key(|m| (m.position().x, m.position().y));
    list
}

/// Número (1, 2...) do monitor na lista ordenada.
pub fn number_of(list: &[MonitorHandle], mon: &MonitorHandle) -> Option<usize> {
    list.iter().position(|m| m == mon).map(|i| i + 1)
}

pub fn describe(el: &ActiveEventLoop, current: Option<&MonitorHandle>) -> Vec<MonitorInfo> {
    let primary = el.primary_monitor();
    sorted_monitors(el)
        .iter()
        .enumerate()
        .map(|(i, m)| MonitorInfo {
            numero: i + 1,
            largura: m.size().width,
            altura: m.size().height,
            x: m.position().x,
            y: m.position().y,
            principal: primary.as_ref() == Some(m),
            atual: current == Some(m),
        })
        .collect()
}

/// Estado da reserva de espaço (uma por janela).
#[derive(Default)]
pub struct Reserve {
    #[cfg(windows)]
    registered: bool,
    /// Última reserva aplicada: (x, y, largura, altura do monitor, topo?, altura da faixa).
    last: Option<(i32, i32, u32, u32, bool, u32)>,
    /// Y onde a janela deve ficar, calculado na última reserva.
    last_y: Option<i32>,
}

impl Reserve {
    /// Reserva a faixa do topo (`top`) ou da base do monitor e devolve o Y em que a janela deve ficar.
    pub fn apply(&mut self, window: &Window, mon: &MonitorHandle, top: bool, height: u32) -> Option<i32> {
        let (p, s) = (mon.position(), mon.size());
        let key = (p.x, p.y, s.width, s.height, top, height);
        if self.last == Some(key) {
            return self.last_y;
        }
        let y = imp::apply(self, window, mon, top, height)?;
        self.last = Some(key);
        self.last_y = Some(y);
        Some(y)
    }

    /// Devolve a faixa ao sistema.
    pub fn release(&mut self, window: &Window) {
        if self.last.take().is_some() || self.is_registered() {
            imp::release(self, window);
        }
        self.last_y = None;
    }

    fn is_registered(&self) -> bool {
        #[cfg(windows)]
        return self.registered;
        #[cfg(not(windows))]
        false
    }
}

#[cfg(windows)]
mod imp {
    use super::Reserve;
    use windows_sys::Win32::Foundation::{HWND, POINT, RECT};
    use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint};
    use windows_sys::Win32::UI::Shell::{
        ABE_BOTTOM, ABE_TOP, ABM_NEW, ABM_QUERYPOS, ABM_REMOVE, ABM_SETPOS, APPBARDATA, SHAppBarMessage,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::WM_USER;
    use winit::monitor::MonitorHandle;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::Window;

    fn hwnd(window: &Window) -> Option<HWND> {
        match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(w) => Some(w.hwnd.get() as HWND),
            _ => None,
        }
    }

    fn data(hwnd: HWND) -> APPBARDATA {
        let mut d: APPBARDATA = unsafe { std::mem::zeroed() };
        d.cbSize = std::mem::size_of::<APPBARDATA>() as u32;
        d.hWnd = hwnd;
        d
    }

    fn monitor_rect(mon: &MonitorHandle) -> Option<RECT> {
        let (p, s) = (mon.position(), mon.size());
        let center = POINT { x: p.x + s.width as i32 / 2, y: p.y + s.height as i32 / 2 };
        unsafe {
            let hmon = MonitorFromPoint(center, MONITOR_DEFAULTTONEAREST);
            let mut info: MONITORINFO = std::mem::zeroed();
            info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
            (GetMonitorInfoW(hmon, &mut info) != 0).then_some(info.rcMonitor)
        }
    }

    pub fn apply(r: &mut Reserve, window: &Window, mon: &MonitorHandle, top: bool, height: u32) -> Option<i32> {
        let hwnd = hwnd(window)?;
        let mut d = data(hwnd);
        if !r.registered {
            // A mensagem de aviso é obrigatória no registro; os avisos (ABN_*) são ignorados.
            d.uCallbackMessage = WM_USER + 0x4C;
            if unsafe { SHAppBarMessage(ABM_NEW, &mut d) } == 0 {
                return None;
            }
            r.registered = true;
        }
        let h = height as i32;
        d.uEdge = if top { ABE_TOP } else { ABE_BOTTOM };
        d.rc = monitor_rect(mon)?;
        let fit = |rc: &mut RECT| {
            if top {
                rc.bottom = rc.top + h;
            } else {
                rc.top = rc.bottom - h;
            }
        };
        fit(&mut d.rc);
        // O sistema ajusta o retângulo para não sobrepor outras barras (a barra de tarefas, por exemplo).
        unsafe { SHAppBarMessage(ABM_QUERYPOS, &mut d) };
        fit(&mut d.rc);
        unsafe { SHAppBarMessage(ABM_SETPOS, &mut d) };
        Some(d.rc.top)
    }

    pub fn release(r: &mut Reserve, window: &Window) {
        if let Some(hwnd) = hwnd(window) {
            let mut d = data(hwnd);
            unsafe { SHAppBarMessage(ABM_REMOVE, &mut d) };
        }
        r.registered = false;
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::Reserve;
    use winit::monitor::MonitorHandle;
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::Window;
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, PropMode};
    use x11rb::wrapper::ConnectionExt as _;

    fn x11_window(window: &Window) -> Option<u32> {
        match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Xlib(h) => Some(h.window as u32),
            RawWindowHandle::Xcb(h) => Some(h.window.get()),
            _ => None, // Wayland nativo: não há como reservar espaço
        }
    }

    /// Grava (ou apaga, com `None`) _NET_WM_STRUT_PARTIAL e _NET_WM_STRUT na janela.
    fn set_strut(window: u32, values: Option<[u32; 12]>) -> Result<(), String> {
        let (conn, _) = x11rb::connect(None).map_err(|e| format!("conexão X11: {e}"))?;
        let atom = |name: &[u8]| -> Result<u32, String> {
            Ok(conn.intern_atom(false, name).map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?.atom)
        };
        let partial = atom(b"_NET_WM_STRUT_PARTIAL")?;
        let strut = atom(b"_NET_WM_STRUT")?;
        let err = |e: x11rb::errors::ConnectionError| e.to_string();
        match values {
            Some(v) => {
                conn.change_property32(PropMode::REPLACE, window, partial, AtomEnum::CARDINAL, &v).map_err(err)?;
                conn.change_property32(PropMode::REPLACE, window, strut, AtomEnum::CARDINAL, &v[..4]).map_err(err)?;
            }
            None => {
                conn.delete_property(window, partial).map_err(err)?;
                conn.delete_property(window, strut).map_err(err)?;
            }
        }
        conn.flush().map_err(err)?;
        // Espera o servidor processar antes de fechar a conexão.
        conn.get_input_focus().map_err(err)?.reply().map_err(|e| e.to_string())?;
        Ok(())
    }

    fn root_height() -> Option<u32> {
        let (conn, n) = x11rb::connect(None).ok()?;
        Some(conn.setup().roots[n].height_in_pixels as u32)
    }

    pub fn apply(_r: &mut Reserve, window: &Window, mon: &MonitorHandle, top: bool, height: u32) -> Option<i32> {
        let Some(id) = x11_window(window) else {
            eprintln!("painel-led: reservar espaço só funciona no X11 (inclusive XWayland)");
            return None;
        };
        let (p, s) = (mon.position(), mon.size());
        let (x0, x1) = (p.x.max(0) as u32, (p.x + s.width as i32 - 1).max(0) as u32);
        // Valores contados a partir das bordas da tela inteira (todos os monitores).
        let mut v = [0u32; 12];
        let y = if top {
            v[2] = p.y.max(0) as u32 + height;
            v[8] = x0;
            v[9] = x1;
            p.y
        } else {
            let bottom = p.y + s.height as i32;
            v[3] = root_height()?.saturating_sub(bottom.max(0) as u32) + height;
            v[10] = x0;
            v[11] = x1;
            bottom - height as i32
        };
        if let Err(e) = set_strut(id, Some(v)) {
            eprintln!("painel-led: não consegui reservar espaço: {e}");
            return None;
        }
        Some(y)
    }

    pub fn release(_r: &mut Reserve, window: &Window) {
        if let Some(id) = x11_window(window) {
            let _ = set_strut(id, None);
        }
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
mod imp {
    use super::Reserve;
    use winit::monitor::MonitorHandle;
    use winit::window::Window;

    pub fn apply(_r: &mut Reserve, _w: &Window, _m: &MonitorHandle, _top: bool, _h: u32) -> Option<i32> {
        None
    }

    pub fn release(_r: &mut Reserve, _w: &Window) {}
}
