fn main() {
    if cfg!(target_os = "windows") {
        winres::WindowsResource::new()
            .set_icon("icon.ico") // Optional: add an icon
            .compile()
            .unwrap();
    }
}