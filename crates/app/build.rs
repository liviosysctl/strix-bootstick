//! Compiles the window (`ui/`) with its German texts (`translations/`), and
//! on Windows puts the icon and the manifest into the `.exe`. The manifest
//! asks for administrator rights, so Windows shows its prompt once at the
//! start: writing to a whole disk and setting a firmware variable need them.

use slint_build::{CompilerConfiguration, DefaultTranslationContext};

fn main() {
    let config = CompilerConfiguration::new()
        .with_bundled_translations("translations")
        .with_default_translation_context(DefaultTranslationContext::None);
    slint_build::compile_with_config("ui/app.slint", config).expect("compile the window");

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rerun-if-changed=windows/app.rc");
        println!("cargo:rerun-if-changed=windows/app.manifest");
        println!("cargo:rerun-if-changed=icons/icon.ico");
        embed_resource::compile("windows/app.rc", embed_resource::NONE)
            .manifest_required()
            .expect("embed the icon and the Windows manifest");
    }
}
