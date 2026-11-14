#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agenda;
mod autostart;
mod claude;
mod config;
mod font;
mod integrations;
mod render;
mod screen;
mod sources;
mod state;
mod tray;

use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};
use tray_icon::menu::MenuEvent;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::monitor::MonitorHandle;
use winit::window::{Window, WindowId, WindowLevel};

use config::{Config, Effect, PanelCfg, Position, Width};
use render::{Col, Composer};
use state::{Message, Rgb, Shared, State};
use tray::Tray;

type Surface = softbuffer::Surface<Rc<Window>, Rc<Window>>;

const BG: Rgb = Rgb(8, 8, 8);
const OFF: Rgb = Rgb(26, 26, 26);
const FRAME: Rgb = Rgb(35, 35, 35);

// ---------------------------------------------------------------- geometria

#[derive(Clone, Copy, PartialEq, Debug)]
struct Geometry {
    size: PhysicalSize<u32>,
    pos: PhysicalPosition<i32>,
    cols: usize,
    rows: usize,
    pitch: u32,
    origin: (u32, u32),
    scale: f64,
}

/// Área útil do monitor (sem a barra de tarefas no Windows).
fn work_area(mon: &MonitorHandle) -> (i32, i32, u32, u32) {
    let (p, s) = (mon.position(), mon.size());
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::Graphics::Gdi::{
            GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint,
        };
        let center = POINT { x: p.x + s.width as i32 / 2, y: p.y + s.height as i32 / 2 };
        let hmon = MonitorFromPoint(center, MONITOR_DEFAULTTONEAREST);
        let mut info: MONITORINFO = std::mem::zeroed();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(hmon, &mut info) != 0 {
            let r = info.rcWork;
            return (r.left, r.top, (r.right - r.left) as u32, (r.bottom - r.top) as u32);
        }
    }
    (p.x, p.y, s.width, s.height)
}

fn geometry(cfg: &PanelCfg, mon: &MonitorHandle, free_pos: Option<PhysicalPosition<i32>>) -> Geometry {
    let scale = mon.scale_factor();
    let pitch = (cfg.tamanho_led.clamp(3.0, 60.0) * scale).round() as u32;
    let pad = (pitch / 2).max(2);
    let (wx, wy, ww, wh) = work_area(mon);
    let width = match cfg.width() {
        Width::Percent(p) => ww as f64 * p / 100.0,
        Width::Pixels(px) => px * scale,
    }
    .min(ww as f64) as u32;
    let cols = ((width.saturating_sub(2 * pad)) / pitch).max(10) as usize;
    let rows = cfg.lines() * 8 + 1;
    let width = width.max(cols as u32 * pitch + 2 * pad);
    let height = rows as u32 * pitch + 2 * pad;
    let origin = ((width - cols as u32 * pitch) / 2, pad);
    let centered_x = wx + (ww as i32 - width as i32) / 2;
    let pos = match cfg.posicao {
        Position::Topo => PhysicalPosition::new(centered_x, wy),
        Position::Base => PhysicalPosition::new(centered_x, wy + wh as i32 - height as i32),
        Position::Livre => {
            // Mantém onde o usuário deixou, mas sempre inteiro dentro da área útil do monitor.
            let p = free_pos.unwrap_or(PhysicalPosition::new(centered_x, wy + 40));
            let max_x = (wx + ww as i32 - width as i32).max(wx);
            let max_y = (wy + wh as i32 - height as i32).max(wy);
            PhysicalPosition::new(p.x.clamp(wx, max_x), p.y.clamp(wy, max_y))
        }
    };
    Geometry { size: PhysicalSize::new(width, height), pos, cols, rows, pitch, origin, scale }
}

#[derive(Serialize, Deserialize)]
struct SavedPos {
    x: i32,
    y: i32,
}

fn pos_path() -> std::path::PathBuf {
    config::dir().join("janela.json")
}

fn load_pos() -> Option<PhysicalPosition<i32>> {
    let p: SavedPos = serde_json::from_str(&std::fs::read_to_string(pos_path()).ok()?).ok()?;
    Some(PhysicalPosition::new(p.x, p.y))
}

fn save_pos(p: PhysicalPosition<i32>) {
    let _ = std::fs::create_dir_all(config::dir());
    let _ = std::fs::write(pos_path(), serde_json::to_string(&SavedPos { x: p.x, y: p.y }).unwrap());
}

fn contains(m: &MonitorHandle, p: PhysicalPosition<i32>) -> bool {
    let (mp, ms) = (m.position(), m.size());
    p.x >= mp.x && p.y >= mp.y && p.x < mp.x + ms.width as i32 && p.y < mp.y + ms.height as i32
}

fn monitor_at(el: &ActiveEventLoop, p: PhysicalPosition<i32>) -> Option<MonitorHandle> {
    el.available_monitors().find(|m| contains(m, p))
}

// ---------------------------------------------------------------- desenho em software

/// Máscaras pré-calculadas de um LED (aceso: núcleo + brilho; apagado).
struct Sprites {
    core: Vec<f32>,
    glow: Vec<f32>,
    off: Vec<f32>,
}

impl Sprites {
    fn new(pitch: u32) -> Self {
        let p = pitch as f32;
        let (r_on, r_off, r_glow) = (p * 0.40, p * 0.30, p * 0.50);
        let n = (pitch * pitch) as usize;
        let (mut core, mut glow, mut off) = (vec![0.0; n], vec![0.0; n], vec![0.0; n]);
        for y in 0..pitch {
            for x in 0..pitch {
                let dx = x as f32 + 0.5 - p / 2.0;
                let dy = y as f32 + 0.5 - p / 2.0;
                let d = (dx * dx + dy * dy).sqrt();
                let i = (y * pitch + x) as usize;
                core[i] = (r_on - d + 0.5).clamp(0.0, 1.0);
                off[i] = (r_off - d + 0.5).clamp(0.0, 1.0);
                glow[i] = if d > r_on {
                    (1.0 - (d - r_on) / (r_glow - r_on + 1.0)).clamp(0.0, 1.0).powi(2) * 0.30
                } else {
                    0.0
                };
            }
        }
        Self { core, glow, off }
    }
}

#[inline]
fn pack(r: f32, g: f32, b: f32) -> u32 {
    ((r.min(255.0) as u32) << 16) | ((g.min(255.0) as u32) << 8) | (b.min(255.0) as u32)
}

fn draw(
    buf: &mut [u32],
    g: &Geometry,
    sp: &Sprites,
    lines: &[Vec<Col>],
    border: Option<Rgb>,
    hover_close: Option<bool>,
    brightness: f32,
) {
    let w = g.size.width as usize;
    let h = g.size.height as usize;
    buf.fill(pack(BG.0 as f32, BG.1 as f32, BG.2 as f32));
    let p = g.pitch as usize;

    for row in 0..g.rows {
        // Linha 0 = margem; depois blocos de 8 (7 de texto + 1 de espaço).
        let (li, bit) = if row == 0 { (usize::MAX, 7) } else { ((row - 1) / 8, (row - 1) % 8) };
        for x in 0..g.cols {
            let lit = if bit < 7 {
                lines
                    .get(li)
                    .and_then(|l| l.get(x))
                    .filter(|(bits, _)| bits & (1 << bit) != 0)
                    .map(|(_, c)| *c)
            } else {
                None
            };
            let x0 = g.origin.0 as usize + x * p;
            let y0 = g.origin.1 as usize + row * p;
            for dy in 0..p {
                let start = (y0 + dy) * w + x0;
                let line = &mut buf[start..start + p];
                for (dx, px) in line.iter_mut().enumerate() {
                    let i = dy * p + dx;
                    match lit {
                        Some(c) => {
                            let (a, gl) = (sp.core[i], sp.glow[i]);
                            if a == 0.0 && gl == 0.0 {
                                continue;
                            }
                            let mix = |bgc: u8, cc: u8| {
                                let cc = cc as f32 * brightness;
                                bgc as f32 * (1.0 - a) + cc * a + cc * gl * (1.0 - a)
                            };
                            *px = pack(mix(BG.0, c.0), mix(BG.1, c.1), mix(BG.2, c.2));
                        }
                        None => {
                            let a = sp.off[i];
                            if a == 0.0 {
                                continue;
                            }
                            let mix = |bgc: u8, cc: u8| bgc as f32 * (1.0 - a) + cc as f32 * a;
                            *px = pack(mix(BG.0, OFF.0), mix(BG.1, OFF.1), mix(BG.2, OFF.2));
                        }
                    }
                }
            }
        }
    }

    // Moldura (acende na cor da mensagem ativa).
    let (c, thick) = match border {
        Some(c) => (c, (2.0 * g.scale).round().max(1.0) as usize),
        None => (FRAME, 1),
    };
    let col = pack(c.0 as f32, c.1 as f32, c.2 as f32);
    for t in 0..thick {
        buf[t * w..(t + 1) * w].fill(col);
        buf[(h - 1 - t) * w..(h - t) * w].fill(col);
        for y in 0..h {
            buf[y * w + t] = col;
            buf[y * w + w - 1 - t] = col;
        }
    }

    // Botão fechar (só com o mouse em cima do painel).
    if let Some(hot) = hover_close {
        let (cx, cy, r) = close_button(g);
        let fg = if hot { Rgb(255, 80, 70) } else { Rgb(130, 130, 130) };
        let arm = r * 0.45;
        let stroke = (0.8 * g.scale as f32).max(0.8);
        let (x_lo, x_hi) = ((cx - r).max(0.0) as usize, ((cx + r) as usize).min(w - 1));
        let (y_lo, y_hi) = ((cy - r).max(0.0) as usize, ((cy + r) as usize).min(h - 1));
        for y in y_lo..=y_hi {
            for x in x_lo..=x_hi {
                let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                let d = (dx * dx + dy * dy).sqrt();
                let disc = (r - d + 0.5).clamp(0.0, 1.0);
                if disc == 0.0 {
                    continue;
                }
                let on_x = dx.abs() <= arm && dy.abs() <= arm;
                let line_d = (dx - dy).abs().min((dx + dy).abs()) / std::f32::consts::SQRT_2;
                let x_a = if on_x { (stroke - line_d + 0.5).clamp(0.0, 1.0) } else { 0.0 };
                let ch = |f: u8| 20.0 * (1.0 - x_a) + f as f32 * x_a;
                let old = buf[y * w + x];
                let ob = |s: u32| ((old >> s) & 0xFF) as f32;
                buf[y * w + x] = pack(
                    ob(16) * (1.0 - disc) + ch(fg.0) * disc,
                    ob(8) * (1.0 - disc) + ch(fg.1) * disc,
                    ob(0) * (1.0 - disc) + ch(fg.2) * disc,
                );
            }
        }
    }
}

/// Centro e raio do botão fechar, em pixels físicos.
fn close_button(g: &Geometry) -> (f32, f32, f32) {
    let r = (7.0 * g.scale) as f32;
    let m = (11.0 * g.scale) as f32;
    (g.size.width as f32 - m, m.min(g.size.height as f32 / 2.0), r)
}

// ---------------------------------------------------------------- bandeja e tela de configurações

enum UserEvent {
    Wake,
    Menu(MenuEvent),
}

/// Abre a tela de configurações (servida pelo próprio painel) numa janela de app do Edge,
/// ou no navegador padrão.
fn open_settings() {
    let url = format!("http://{}/config", sources::HTTP_ADDR);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const NO_WINDOW: u32 = 0x0800_0000;
        let edge = [
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
            r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
        ]
        .into_iter()
        .find(|p| std::path::Path::new(p).exists());
        let opened = edge.is_some_and(|e| {
            std::process::Command::new(e)
                .arg(format!("--app={url}"))
                .arg("--window-size=1000,900")
                .spawn()
                .is_ok()
        });
        if !opened {
            let _ = std::process::Command::new("cmd")
                .args(["/C", "start", "", &url])
                .creation_flags(NO_WINDOW)
                .spawn();
        }
    }
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(&url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let browsers = ["google-chrome", "chromium", "chromium-browser", "microsoft-edge", "brave-browser"];
        let opened = browsers.iter().any(|b| {
            std::process::Command::new(b)
                .arg(format!("--app={url}"))
                .arg("--window-size=1000,900")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .is_ok()
        });
        if !opened {
            let _ = std::process::Command::new("xdg-open").arg(&url).spawn();
        }
    }
}

// ---------------------------------------------------------------- app

type Drawn = (Vec<Vec<Col>>, Option<Rgb>, Option<bool>);

struct App {
    cfg: Config,
    cfg_mtime: Option<SystemTime>,
    cfg_checked: Instant,
    state: Shared,
    composer: Composer,
    window: Option<Rc<Window>>,
    surface: Option<Surface>,
    geom: Option<Geometry>,
    sprites: Sprites,
    cursor: Option<PhysicalPosition<f64>>,
    last_click: Option<Instant>,
    next_frame: Instant,
    pending_snap: Option<Instant>,
    last_drawn: Option<Drawn>,
    tray: Option<Tray>,
    visible: bool,
    /// Faixa da tela reservada (posição topo/base com "reservar espaço").
    reserve: screen::Reserve,
    /// Se a janela atual foi criada como Dock (Linux, para reservar espaço).
    window_dock: bool,
}

impl App {
    fn set_visible(&mut self, el: &ActiveEventLoop, visible: bool) {
        self.visible = visible;
        if let Some(w) = self.window.clone() {
            if !visible {
                self.reserve.release(&w);
            }
            w.set_visible(visible);
        }
        if let Some(t) = &self.tray {
            t.set_visible(visible);
        }
        if visible {
            self.relayout(el);
        }
    }

    fn clear_messages(&mut self) {
        self.composer.skip_message();
        let mut s = self.state.lock().unwrap();
        s.messages.clear();
        s.scheduled.clear();
        drop(s);
        self.redraw_now();
    }

    fn on_menu(&mut self, el: &ActiveEventLoop, ev: MenuEvent) {
        let Some(t) = &self.tray else { return };
        let ids = t.ids.clone();
        let id = ev.id();
        if id == &ids.settings {
            open_settings();
        } else if id == &ids.toggle {
            self.set_visible(el, !self.visible);
        } else if id == &ids.clear {
            self.clear_messages();
        } else if id == &ids.file {
            config::open_in_editor();
        } else if id == &ids.autostart {
            // Inverte o estado real (a marcação do menu é só reflexo dele).
            let want = !autostart::is_enabled();
            if let Err(e) = autostart::set(want) {
                self.state.lock().unwrap().push_message(Message {
                    text: format!("Não consegui alterar a inicialização: {e}"),
                    color: Rgb::RED,
                    effect: Effect::Piscar,
                    repeat: 2,
                    seconds: 8.0,
                });
                self.redraw_now();
            }
            if let Some(t) = &self.tray {
                t.set_autostart(autostart::is_enabled());
            }
        } else if id == &ids.quit {
            el.exit();
        }
    }

    /// `None` = mouse fora do painel; `Some(true)` = em cima do botão fechar.
    fn hover_close(&self) -> Option<bool> {
        let (g, c) = (self.geom?, self.cursor?);
        let (cx, cy, r) = close_button(&g);
        let d = ((c.x as f32 - cx).powi(2) + (c.y as f32 - cy).powi(2)).sqrt();
        Some(d <= r + 2.0)
    }

    fn redraw_now(&mut self) {
        self.next_frame = Instant::now();
        self.last_drawn = None;
        if let Some(w) = &self.window {
            w.request_redraw();
        }
    }

    fn resize_surface(&mut self) {
        if let (Some(g), Some(s)) = (self.geom, &mut self.surface) {
            let _ = s.resize(NonZeroU32::new(g.size.width).unwrap(), NonZeroU32::new(g.size.height).unwrap());
        }
    }

    /// Monitor onde o painel deve ficar: o escolhido na configuração ou, no automático,
    /// onde a janela está (ou estava na última execução).
    fn target_monitor(&self, el: &ActiveEventLoop) -> Option<MonitorHandle> {
        let list = screen::sorted_monitors(el);
        let n = self.cfg.painel.monitor;
        if n > 0 {
            if let Some(m) = list.get(n - 1) {
                return Some(m.clone());
            }
        }
        let window = self.window.as_ref();
        let current = window.and_then(|w| w.outer_position().ok());
        window
            .and_then(|w| w.current_monitor())
            .or_else(|| current.and_then(|p| monitor_at(el, p)))
            .or_else(|| load_pos().and_then(|p| monitor_at(el, p)))
            .or_else(|| el.primary_monitor())
            .or_else(|| list.first().cloned())
    }

    fn publish_monitors(&self, el: &ActiveEventLoop) {
        let current = self.window.as_ref().and_then(|w| w.current_monitor());
        let list = screen::describe(el, current.as_ref());
        let mut s = self.state.lock().unwrap();
        if s.monitors != list {
            s.monitors = list;
        }
    }

    /// Recalcula tamanho/posição conforme a configuração e o monitor alvo.
    fn relayout(&mut self, el: &ActiveEventLoop) {
        let Some(window) = self.window.clone() else { return };
        let Some(mon) = self.target_monitor(el) else { return };
        // Posição livre: mantém onde está se continuar no monitor alvo.
        let keep = window
            .outer_position()
            .ok()
            .filter(|p| contains(&mon, *p))
            .or_else(|| load_pos().filter(|p| contains(&mon, *p)));
        let mut g = geometry(&self.cfg.painel, &mon, keep);
        if self.cfg.painel.reserves_space() && self.visible {
            let top = self.cfg.painel.posicao == Position::Topo;
            if let Some(y) = self.reserve.apply(&window, &mon, top, g.size.height) {
                g.pos.y = y;
            }
        } else {
            self.reserve.release(&window);
        }
        let current = window.outer_position().ok();
        if self.geom.map(|o| o.pitch) != Some(g.pitch) {
            self.sprites = Sprites::new(g.pitch);
        }
        self.geom = Some(g);
        if window.inner_size() != g.size {
            let _ = window.request_inner_size(g.size);
        }
        if current != Some(g.pos) {
            window.set_outer_position(g.pos);
        }
        self.resize_surface();
        self.redraw_now();
        self.publish_monitors(el);
    }

    /// Cria a janela do painel (também usada para recriá-la no Linux ao ligar/desligar a reserva).
    fn create_window(&mut self, el: &ActiveEventLoop) {
        let Some(mon) = self.target_monitor(el) else {
            el.exit();
            return;
        };
        let saved = load_pos().filter(|p| contains(&mon, *p));
        let g = geometry(&self.cfg.painel, &mon, saved);
        let dock = self.cfg.painel.reserves_space();

        let attrs = Window::default_attributes()
            .with_title("Painel LED")
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_inner_size(g.size)
            .with_position(g.pos);
        #[cfg(windows)]
        let attrs = {
            use winit::platform::windows::WindowAttributesExtWindows;
            attrs.with_skip_taskbar(true)
        };
        #[cfg(target_os = "linux")]
        let attrs = {
            use winit::platform::x11::{WindowAttributesExtX11, WindowType};
            // "Dock" para reservar espaço; "Utility" fica fora da barra de tarefas no GNOME e no KDE.
            // O WSLg (e algum gerenciador de janelas que trate mal esses tipos) esconde a janela:
            // nesses casos usa uma janela comum.
            let normal = std::env::var_os("WSL_DISTRO_NAME").is_some()
                || std::env::var_os("PAINEL_LED_JANELA_NORMAL").is_some();
            let kind = if normal {
                WindowType::Normal
            } else if dock {
                WindowType::Dock
            } else {
                WindowType::Utility
            };
            attrs.with_x11_window_type(vec![kind]).with_name("painel-led", "painel-led")
        };
        let window = match el.create_window(attrs) {
            Ok(w) => Rc::new(w),
            Err(_) => {
                el.exit();
                return;
            }
        };
        round_corners(&window);
        let ctx = softbuffer::Context::new(window.clone()).expect("softbuffer");
        let surface = Surface::new(&ctx, window.clone()).expect("surface");
        self.sprites = Sprites::new(g.pitch);
        self.geom = Some(g);
        self.surface = Some(surface);
        self.window = Some(window);
        self.window_dock = dock;
        self.relayout(el);
    }

    fn check_config(&mut self, el: &ActiveEventLoop) {
        self.cfg_checked = Instant::now();
        // Mantém a marcação da bandeja em sincronia (a tela de configurações também pode mudar isso).
        if let Some(t) = &self.tray {
            t.set_autostart(autostart::is_enabled());
        }
        // Monitor conectado/desconectado: refaz o layout.
        let before = self.state.lock().unwrap().monitors.len();
        self.publish_monitors(el);
        if self.state.lock().unwrap().monitors.len() != before && before > 0 {
            self.relayout(el);
        }
        let m = config::modified();
        if m == self.cfg_mtime {
            return;
        }
        self.cfg_mtime = m;
        match config::load() {
            Ok(new) => {
                let relayout = format!("{:?}", new.painel) != format!("{:?}", self.cfg.painel);
                {
                    let mut st = self.state.lock().unwrap();
                    st.agenda_cfg = new.agenda.clone();
                    st.claude_cfg = new.claude.clone();
                }
                self.cfg = new;
                self.composer.reset();
                if cfg!(target_os = "linux") && self.cfg.painel.reserves_space() != self.window_dock {
                    // No X11, o tipo da janela (Dock) só vale na criação: recria a janela.
                    if let Some(w) = self.window.take() {
                        self.reserve.release(&w);
                    }
                    self.surface = None;
                    self.create_window(el);
                } else if relayout {
                    self.relayout(el);
                } else {
                    self.redraw_now();
                }
            }
            Err(e) => {
                self.state.lock().unwrap().push_message(config_error(&e));
                self.redraw_now();
            }
        }
    }

    fn paint(&mut self) {
        let Some(g) = self.geom else { return };
        let hover = self.hover_close();
        let frame = {
            let mut s = self.state.lock().unwrap();
            self.composer.compose(&self.cfg, &mut s, g.cols)
        };
        self.next_frame = Instant::now() + frame.next;
        let sig: Drawn = (frame.lines, frame.border, hover);
        if self.last_drawn.as_ref() == Some(&sig) {
            return; // nada mudou: não redesenha
        }
        let Some(surface) = self.surface.as_mut() else { return };
        if surface.window().inner_size() != g.size {
            return; // aguardando o redimensionamento terminar
        }
        let Ok(mut buf) = surface.buffer_mut() else { return };
        let brightness = self.cfg.painel.brilho.clamp(10, 100) as f32 / 100.0;
        draw(&mut buf, &g, &self.sprites, &sig.0, sig.1, sig.2, brightness);
        let _ = buf.present();
        self.last_drawn = Some(sig);
    }
}

fn config_error(e: &str) -> Message {
    Message {
        text: format!("ERRO NA CONFIG: {e}"),
        color: Rgb::RED,
        effect: Effect::Piscar,
        repeat: 2,
        seconds: 10.0,
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        self.create_window(el);
        if self.window.is_some() {
            self.tray = Tray::new(autostart::is_enabled());
        }
    }

    fn exiting(&mut self, _el: &ActiveEventLoop) {
        // Devolve a faixa reservada antes de fechar.
        if let Some(w) = &self.window {
            self.reserve.release(w);
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => el.exit(),
            WindowEvent::RedrawRequested => self.paint(),
            WindowEvent::Resized(_) => {
                self.resize_surface();
                self.redraw_now();
            }
            WindowEvent::ScaleFactorChanged { .. } => self.relayout(el),
            WindowEvent::Moved(p) => {
                if self.geom.map(|g| g.pos) != Some(p) {
                    self.pending_snap = Some(Instant::now());
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let before = self.hover_close();
                self.cursor = Some(position);
                if before != self.hover_close() {
                    self.redraw_now();
                }
            }
            WindowEvent::CursorLeft { .. } => {
                self.cursor = None;
                self.redraw_now();
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button, .. } => match button {
                MouseButton::Left => {
                    if self.hover_close() == Some(true) {
                        el.exit();
                        return;
                    }
                    // Duplo clique: descarta a mensagem atual e a fila.
                    if self.last_click.is_some_and(|t| t.elapsed() < Duration::from_millis(350)) {
                        self.last_click = None;
                        self.clear_messages();
                        return;
                    }
                    self.last_click = Some(Instant::now());
                    if let Some(w) = &self.window {
                        let _ = w.drag_window();
                    }
                }
                MouseButton::Right => open_settings(),
                _ => {}
            },
            _ => {}
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Wake => {
                self.next_frame = Instant::now();
                if let Some(w) = &self.window {
                    w.request_redraw();
                }
            }
            UserEvent::Menu(ev) => self.on_menu(el, ev),
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if self.cfg_checked.elapsed() >= Duration::from_secs(1) {
            self.check_config(el);
        }
        // Depois de arrastar: ajusta ao monitor onde o painel caiu (largura, encaixe) e salva a posição.
        if let Some(t) = self.pending_snap {
            if t.elapsed() >= Duration::from_millis(400) {
                self.pending_snap = None;
                if self.cfg.painel.monitor > 0 {
                    let landed = self.window.as_ref().and_then(|w| w.current_monitor());
                    let list = screen::sorted_monitors(el);
                    if let Some(n) = landed.and_then(|m| screen::number_of(&list, &m)) {
                        if n != self.cfg.painel.monitor {
                            self.cfg.painel.monitor = n;
                            let _ = config::save(&self.cfg);
                            self.cfg_mtime = config::modified();
                        }
                    }
                }
                self.relayout(el);
                if let Some(g) = self.geom {
                    save_pos(g.pos);
                }
            }
        }
        let now = Instant::now();
        if now >= self.next_frame {
            if let Some(w) = &self.window {
                w.request_redraw();
            }
            self.next_frame = now + Duration::from_millis(1000);
        }
        let mut wake = self.next_frame.min(self.cfg_checked + Duration::from_secs(1));
        if let Some(t) = self.pending_snap {
            wake = wake.min(t + Duration::from_millis(400));
        }
        el.set_control_flow(ControlFlow::WaitUntil(wake));
    }
}

/// Cantos arredondados no Windows 11.
fn round_corners(window: &Window) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Graphics::Dwm::{
            DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
        };
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(h) = window.window_handle() {
            if let RawWindowHandle::Win32(w) = h.as_raw() {
                let pref = DWMWCP_ROUND;
                unsafe {
                    DwmSetWindowAttribute(
                        w.hwnd.get() as _,
                        DWMWA_WINDOW_CORNER_PREFERENCE as _,
                        &pref as *const _ as *const _,
                        std::mem::size_of_val(&pref) as u32,
                    );
                }
            }
        }
    }
    #[cfg(not(windows))]
    let _ = window;
}

fn main() {
    // Porta ocupada = outra instância já está rodando.
    let Some(server) = sources::bind_http() else {
        eprintln!("painel-led: {} já em uso (outra instância aberta?)", sources::HTTP_ADDR);
        return;
    };

    config::ensure_file();
    let state: Shared = Arc::new(Mutex::new(State::default()));
    let cfg = match config::load() {
        Ok(c) => c,
        Err(e) => {
            state.lock().unwrap().push_message(config_error(&e));
            Config::default()
        }
    };
    {
        let mut st = state.lock().unwrap();
        st.agenda_cfg = cfg.agenda.clone();
        st.claude_cfg = cfg.claude.clone();
        claude::load_limits(&mut st);
    }

    let mut builder = EventLoop::<UserEvent>::with_user_event();
    #[cfg(target_os = "linux")]
    {
        use winit::platform::x11::EventLoopBuilderExtX11;
        let force_wayland = std::env::var_os("PAINEL_LED_WAYLAND").is_some();
        if !force_wayland && std::env::var_os("DISPLAY").is_some() {
            builder.with_x11();
        }
    }
    let event_loop = builder.build().expect("event loop");
    let proxy = Mutex::new(event_loop.create_proxy());
    let wake: state::Wake = Arc::new(move || {
        let _ = proxy.lock().unwrap().send_event(UserEvent::Wake);
    });
    let menu_proxy = Mutex::new(event_loop.create_proxy());
    MenuEvent::set_event_handler(Some(move |ev| {
        let _ = menu_proxy.lock().unwrap().send_event(UserEvent::Menu(ev));
    }));

    sources::spawn_system(state.clone());
    sources::spawn_ping(state.clone(), wake.clone());
    sources::spawn_http(server, state.clone(), wake.clone(), sources::new_token());
    agenda::spawn(state.clone(), wake);

    let mut app = App {
        cfg,
        cfg_mtime: config::modified(),
        cfg_checked: Instant::now(),
        state,
        composer: Composer::new(),
        window: None,
        surface: None,
        geom: None,
        sprites: Sprites::new(8),
        cursor: None,
        last_click: None,
        next_frame: Instant::now(),
        pending_snap: None,
        last_drawn: None,
        tray: None,
        reserve: screen::Reserve::default(),
        window_dock: false,
        visible: true,
    };
    let _ = event_loop.run_app(&mut app);
}
