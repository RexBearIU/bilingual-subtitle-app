fn main() {
    // tauri-build embeds the Windows icon from tauri.conf.json, but only
    // declares a rerun trigger for the config file itself — not for the icon
    // files it names. Replace the artwork and cargo sees nothing to do, so the
    // build succeeds and silently ships the previous icon. That happened once;
    // the only symptom was the old icon still being there after a full
    // reinstall, with nothing in the build output to suggest why.
    println!("cargo:rerun-if-changed=icons");

    tauri_build::build()
}
