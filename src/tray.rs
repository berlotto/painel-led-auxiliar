//! Ícone na bandeja do sistema.
//! Windows/macOS: o menu vive na thread da janela.
//! Linux: o tray-icon precisa de GTK, então o menu vive numa thread própria rodando `gtk::main`;
//! a janela só guarda os ids dos itens e manda comandos por um canal.

use tray_icon::menu::{CheckMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder};

#[cfg(windows)]
const AUTOSTART_LABEL: &str = "Iniciar com o Windows";
#[cfg(not(windows))]
const AUTOSTART_LABEL: &str = "Iniciar com o sistema";

/// Ícone gerado em código: 4x4 LEDs âmbar formando uma moldura.
pub fn icon_rgba(size: u32) -> Vec<u8> {
    const LIT: [[u8; 4]; 4] = [[1, 1, 1, 1], [1, 0, 0, 1], [1, 0, 0, 1], [1, 1, 1, 1]];
    let s = size as f32;
    let mut px = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // Fundo com cantos arredondados.
            let r = s * 0.22;
            let cx = fx.clamp(r, s - r);
            let cy = fy.clamp(r, s - r);
            let bg_a = (r - ((fx - cx).powi(2) + (fy - cy).powi(2)).sqrt() + 0.5).clamp(0.0, 1.0);
            let mut c = [20.0, 20.0, 20.0];
            let mut a = bg_a;
            // LEDs.
            let cell = s / 4.0;
            let (gx, gy) = (((fx / cell) as usize).min(3), ((fy / cell) as usize).min(3));
            let (lx, ly) = (gx as f32 * cell + cell / 2.0, gy as f32 * cell + cell / 2.0);
            let d = ((fx - lx).powi(2) + (fy - ly).powi(2)).sqrt();
            let dot = (cell * 0.36 - d + 0.5).clamp(0.0, 1.0);
            let led = if LIT[gy][gx] == 1 { [255.0, 176.0, 0.0] } else { [60.0, 60.0, 60.0] };
            for i in 0..3 {
                c[i] = c[i] * (1.0 - dot) + led[i] * dot;
            }
            a = a.max(dot * bg_a);
            let o = ((y * size + x) * 4) as usize;
            px[o..o + 4].copy_from_slice(&[c[0] as u8, c[1] as u8, c[2] as u8, (a * 255.0) as u8]);
        }
    }
    px
}

/// Ids dos itens do menu (comparados com os eventos de `MenuEvent`).
#[derive(Clone)]
pub struct Ids {
    pub settings: MenuId,
    pub toggle: MenuId,
    pub clear: MenuId,
    pub file: MenuId,
    pub autostart: MenuId,
    pub quit: MenuId,
}

struct Items {
    _icon: TrayIcon,
    toggle: MenuItem,
    autostart: CheckMenuItem,
}

impl Items {
    fn set_visible(&self, visible: bool) {
        self.toggle.set_text(if visible { "Ocultar painel" } else { "Mostrar painel" });
    }
    fn set_autostart(&self, on: bool) {
        if self.autostart.is_checked() != on {
            self.autostart.set_checked(on);
        }
    }
}

fn build(autostart_on: bool) -> Option<(Items, Ids)> {
    let settings = MenuItem::new("Configurações…", true, None);
    let toggle = MenuItem::new("Ocultar painel", true, None);
    let clear = MenuItem::new("Limpar mensagens", true, None);
    let file = MenuItem::new("Editar arquivo de configuração", true, None);
    let autostart = CheckMenuItem::new(AUTOSTART_LABEL, true, autostart_on, None);
    let quit = MenuItem::new("Sair", true, None);
    let menu = Menu::new();
    menu.append_items(&[
        &settings,
        &toggle,
        &clear,
        &PredefinedMenuItem::separator(),
        &autostart,
        &file,
        &PredefinedMenuItem::separator(),
        &quit,
    ])
    .ok()?;
    let icon = tray_icon::Icon::from_rgba(icon_rgba(32), 32, 32).ok()?;
    let tray = TrayIconBuilder::new()
        .with_tooltip("Painel LED")
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(true)
        .build()
        .ok()?;
    let ids = Ids {
        settings: settings.id().clone(),
        toggle: toggle.id().clone(),
        clear: clear.id().clone(),
        file: file.id().clone(),
        autostart: autostart.id().clone(),
        quit: quit.id().clone(),
    };
    Some((Items { _icon: tray, toggle, autostart }, ids))
}

pub struct Tray {
    pub ids: Ids,
    #[cfg(not(target_os = "linux"))]
    items: Items,
    #[cfg(target_os = "linux")]
    tx: std::sync::mpsc::Sender<Cmd>,
}

#[cfg(target_os = "linux")]
enum Cmd {
    Visible(bool),
    Autostart(bool),
}

impl Tray {
    #[cfg(not(target_os = "linux"))]
    pub fn new(autostart_on: bool) -> Option<Tray> {
        let (items, ids) = build(autostart_on)?;
        Some(Tray { ids, items })
    }

    /// Sobe uma thread com GTK para o ícone. Sem GTK ou sem bandeja no ambiente, devolve `None`
    /// e o painel funciona sem o ícone (as configurações abrem pelo clique direito).
    #[cfg(target_os = "linux")]
    pub fn new(autostart_on: bool) -> Option<Tray> {
        use std::sync::mpsc;
        let (ids_tx, ids_rx) = mpsc::channel::<Option<Ids>>();
        let (tx, rx) = mpsc::channel::<Cmd>();
        std::thread::spawn(move || {
            if gtk::init().is_err() {
                let _ = ids_tx.send(None);
                return;
            }
            let Some((items, ids)) = build(autostart_on) else {
                let _ = ids_tx.send(None);
                return;
            };
            let _ = ids_tx.send(Some(ids));
            gtk::glib::timeout_add_local(std::time::Duration::from_millis(200), move || {
                loop {
                    match rx.try_recv() {
                        Ok(Cmd::Visible(v)) => items.set_visible(v),
                        Ok(Cmd::Autostart(on)) => items.set_autostart(on),
                        Err(mpsc::TryRecvError::Empty) => return gtk::glib::ControlFlow::Continue,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            gtk::main_quit();
                            return gtk::glib::ControlFlow::Break;
                        }
                    }
                }
            });
            gtk::main();
        });
        let ids = ids_rx.recv().ok()??;
        Some(Tray { ids, tx })
    }

    pub fn set_visible(&self, visible: bool) {
        #[cfg(not(target_os = "linux"))]
        self.items.set_visible(visible);
        #[cfg(target_os = "linux")]
        let _ = self.tx.send(Cmd::Visible(visible));
    }

    pub fn set_autostart(&self, on: bool) {
        #[cfg(not(target_os = "linux"))]
        self.items.set_autostart(on);
        #[cfg(target_os = "linux")]
        let _ = self.tx.send(Cmd::Autostart(on));
    }
}
