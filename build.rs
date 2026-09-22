//! 嵌入 Windows 图标到 .exe 资源(资源管理器/任务栏显示产品图标)。
//! 注:运行时窗口图标由 src/ui/app.rs 的 theme::install_fonts / ViewportIcon 设置(独立路径)。

#[cfg(windows)]
fn main() {
    winres::WindowsResource::new()
        .set_icon("assets/icon.ico")
        .compile()
        .expect("嵌入 assets/icon.ico 失败(检查图标文件存在且是合法 .ico)");
}

#[cfg(not(windows))]
fn main() {}
