fn main() {
    // 内置输入框的右键菜单（Cut/Copy/Paste/Select All）来自 Slint std-widgets，
    // 打包中文翻译后运行时选中 zh_CN，避免系统控件显示英文。
    let config = slint_build::CompilerConfiguration::new()
        .with_bundled_translations("ui/lang")
        .with_default_translation_context(slint_build::DefaultTranslationContext::None);
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint 界面编译失败");
    println!("cargo:rerun-if-changed=ui/app.slint");
    println!("cargo:rerun-if-changed=ui/assets/tray-icon.png");
    println!("cargo:rerun-if-changed=ui/lang");
}
