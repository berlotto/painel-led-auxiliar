//! Embute o ícone e as informações de versão nos executáveis do Windows.
fn main() {
    println!("cargo:rerun-if-changed=assets/painel-led.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let mut res = winresource::WindowsResource::new();
    res.set_icon("assets/painel-led.ico");
    res.set("FileDescription", "Painel LED");
    res.set("ProductName", "Painel LED");
    if let Err(e) = res.compile() {
        // Sem o compilador de recursos (rc.exe), segue sem ícone em vez de falhar.
        println!("cargo:warning=ícone não embutido: {e}");
    }
}
