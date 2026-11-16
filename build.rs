//! Embeds the app icon (drawn in `src/icon.rs`) and version info into the exe, so the
//! downloaded `comprimo.exe` already looks like comprimo in Explorer and Task Manager.

#[path = "src/icon.rs"]
mod icon;

fn main() {
    println!("cargo:rerun-if-changed=src/icon.rs");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let ico = std::path::PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("comprimo.ico");
    std::fs::write(&ico, icon::ico_bytes()).unwrap();
    let mut res = winresource::WindowsResource::new();
    res.set_icon(ico.to_str().unwrap());
    res.set("FileDescription", "comprimo");
    res.set("ProductName", "comprimo");
    res.set("LegalCopyright", "MIT license");
    if let Err(e) = res.compile() {
        // Not fatal: the app works without it, it just has the generic exe icon.
        println!("cargo:warning=couldn't embed the icon: {e}");
    }
}
