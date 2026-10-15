// The tray's "Other system tray icons" list shows the exe's FileDescription.
fn main() {
    let mut res = winresource::WindowsResource::new();
    res.set("FileDescription", "Glint");
    res.set("ProductName", "Glint");
    res.set("InternalName", "Glint");
    res.set("OriginalFilename", "glint.exe");
    res.compile().unwrap();
}
