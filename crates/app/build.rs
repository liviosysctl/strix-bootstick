//! Embeds the pages, the icon and the Windows manifest. The manifest asks for
//! administrator rights, so Windows shows its prompt once at the start:
//! writing to a whole disk and setting a firmware variable need them.

fn main() {
    // The window only exists on Windows, see Cargo.toml
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let windows =
        tauri_build::WindowsAttributes::new().app_manifest(include_str!("windows/app.manifest"));
    tauri_build::try_build(tauri_build::Attributes::new().windows_attributes(windows))
        .expect("embed the pages and the Windows manifest");
}
