//! Windows resources for `kuula.exe`: the icon from `assets/branding`.
//! Other targets have no resource section and nothing to do.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=../../assets/branding/kuula.ico");
    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target != "windows" {
        return;
    }
    let icon =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/branding/kuula.ico");
    let mut res = tauri_winres::WindowsResource::new();
    res.set_icon(icon.to_str().expect("icon path is UTF-8"));
    res.set("ProductName", "Kuula");
    res.set("FileDescription", "Kuula fantasy console");
    if let Err(e) = res.compile() {
        // A missing rc.exe must not hide the build; the icon is cosmetic.
        println!("cargo:warning=icon resource not embedded: {e}");
    }
}
