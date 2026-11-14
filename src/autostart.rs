//! "Iniciar com o sistema".
//! Windows: valor em HKCU\Software\Microsoft\Windows\CurrentVersion\Run (sem precisar de administrador).
//! Linux: ~/.config/autostart/painel-led.desktop. macOS: ~/Library/LaunchAgents/com.painel-led.plist.

fn exe() -> Result<String, String> {
    std::env::current_exe().map(|p| p.display().to_string()).map_err(|e| e.to_string())
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::ERROR_SUCCESS;
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ, RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW,
    };

    const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
    const NAME: &str = "PainelLED";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn is_enabled() -> bool {
        let (key, name) = (wide(KEY), wide(NAME));
        let mut size = 0u32;
        let r = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        r == ERROR_SUCCESS
    }

    pub fn set(enabled: bool) -> Result<(), String> {
        let (key, name) = (wide(KEY), wide(NAME));
        let r = if enabled {
            let value = wide(&format!("\"{}\"", super::exe()?));
            unsafe {
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    key.as_ptr(),
                    name.as_ptr(),
                    REG_SZ,
                    value.as_ptr().cast(),
                    (value.len() * 2) as u32,
                )
            }
        } else {
            if !is_enabled() {
                return Ok(());
            }
            unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr()) }
        };
        if r == ERROR_SUCCESS { Ok(()) } else { Err(format!("erro {r} ao gravar no registro")) }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    fn path() -> Option<std::path::PathBuf> {
        Some(dirs::config_dir()?.join("autostart").join("painel-led.desktop"))
    }

    pub fn is_enabled() -> bool {
        path().is_some_and(|p| p.exists())
    }

    pub fn set(enabled: bool) -> Result<(), String> {
        let p = path().ok_or("pasta de configuração não encontrada")?;
        if enabled {
            std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
            let desktop = format!(
                "[Desktop Entry]\nType=Application\nName=Painel LED\nExec=\"{}\"\nX-GNOME-Autostart-enabled=true\n",
                super::exe()?
            );
            std::fs::write(p, desktop).map_err(|e| e.to_string())
        } else if p.exists() {
            std::fs::remove_file(p).map_err(|e| e.to_string())
        } else {
            Ok(())
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    fn path() -> Option<std::path::PathBuf> {
        Some(dirs::home_dir()?.join("Library/LaunchAgents/com.painel-led.plist"))
    }

    pub fn is_enabled() -> bool {
        path().is_some_and(|p| p.exists())
    }

    pub fn set(enabled: bool) -> Result<(), String> {
        let p = path().ok_or("pasta home não encontrada")?;
        if enabled {
            std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
            let plist = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>com.painel-led</string>
  <key>ProgramArguments</key><array><string>{}</string></array>
  <key>RunAtLoad</key><true/>
</dict></plist>
"#,
                super::exe()?
            );
            std::fs::write(p, plist).map_err(|e| e.to_string())
        } else if p.exists() {
            std::fs::remove_file(p).map_err(|e| e.to_string())
        } else {
            Ok(())
        }
    }
}

pub use imp::{is_enabled, set};
