fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        // Spout's optional error-dialog path imports TaskDialogIndirect. On systems
        // without comctl32 v6 activation, resolving it at process load prevents the
        // headless receiver from starting although capture never opens that dialog.
        println!("cargo:rustc-link-arg=/DELAYLOAD:comctl32.dll");
        println!("cargo:rustc-link-lib=delayimp");
    }
}
