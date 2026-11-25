fn main() {
    // Embed the Cargo package version into `anterm-updater.exe`'s PE
    // VS_FIXEDFILEINFO / ProductVersion resource on Windows so
    // `anterm-updater verify-portable` and `verify_installed_payload` can
    // verify the staged updater binary's version.
    #[cfg(windows)]
    {
        let res = winresource::WindowsResource::new();
        if let Err(e) = res.compile() {
            println!("cargo:warning=failed to embed Windows version resource: {e}");
        }
    }
}
