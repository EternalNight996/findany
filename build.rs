//! 嵌入 Windows 图标到 .exe 资源(资源管理器/任务栏显示产品图标)。
//! 注:运行时窗口图标由 src/ui/app.rs 的 load_window_icon() 读 assets/icon.png 设置(独立路径)。

fn main() {
  // **必须按「目标平台」判断,不能用 #[cfg(windows)]**:
  // #[cfg(windows)] 判断的是 build script 的编译宿主;交叉编译 Linux 时宿主仍是 Windows,
  // 会误走 winres 分支去找 rc.exe 而失败。运行时读 CARGO_CFG_TARGET_OS 才是目标平台。
  #[cfg(windows)]
  {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
      // rc.exe 由 vcvars 提供;失败只告警不阻断(退回默认图标,功能不受影响)。
      if let Err(e) = winres::WindowsResource::new().set_icon("assets/icon.ico").compile() {
        println!("cargo:warning=嵌入 assets/icon.ico 失败: {e}(exe 将使用默认图标)");
      }
    }
  }
}
