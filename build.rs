use embed_manifest::{embed_manifest, new_manifest};

fn main() {
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        // NWG's control subclassing (e.g. GetWindowSubclass/SetWindowSubclass) requires
        // Common Controls v6, which Windows only loads when the exe carries a manifest
        // asking for it — without this, the app fails to start with an
        // "Entry Point Not Found" error.
        embed_manifest(new_manifest("Tischer.PhotoMatic")).expect("unable to embed manifest");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
