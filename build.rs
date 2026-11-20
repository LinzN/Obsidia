//! Embeds the application icon into the Windows executable.

fn main() {
    println!("cargo:rerun-if-changed=assets/obsidia.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/obsidia.ico");
        res.set("ProductName", "Obsidia");
        res.set("CompanyName", "MirraNET");
        res.set("LegalCopyright", "MirraNET - https://mirranet.de");
        res.set("FileDescription", "Obsidia - iPhone/iPad firmware tool");
        if let Err(e) = res.compile() {
            println!("cargo:warning=failed to embed Windows icon: {e}");
        }
    }
}
