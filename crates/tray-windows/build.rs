fn main() {
    slint_build::compile("ui/app.slint").expect("Slint 界面编译失败");
    println!("cargo:rerun-if-changed=ui/app.slint");
    println!("cargo:rerun-if-changed=ui/assets/tray-icon.png");
}
