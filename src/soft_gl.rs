//! Windows 软件 OpenGL 兜底(服务器 / RDP / 无显卡驱动环境)
//!
//! 现象:这类 Windows 只有 OpenGL 1.1(Microsoft GDI Generic 软件实现),而 eframe 的
//! glow 后端要求 3.2+,于是 run_native 直接返回 Error::NoGlutinConfigs
//! (eframe/src/native/glow_integration.rs:1123),窗口根本建不出来;打包版又是 GUI
//! 子系统(没有控制台、stdout 是黑洞),现场只看到"双击没反应"。
//!
//! 做法:exe 同级 mesa/ 目录放一份 Mesa3D(llvmpipe) 软件 OpenGL,硬件 GL 起不来时用
//! 子进程 + GLUTIN_WGL_OPENGL_DLL 重启自己。glutin 支持该变量按绝对路径加载 OpenGL
//! 实现(glutin-0.32.3/src/api/wgl/display.rs:44),因此不用换渲染框架、不用 wgpu、
//! 不引入任何新依赖。
//!
//! 现场开关(环境变量 FINDANY_GL):
//!   · 不设置(默认) → 先按硬件 GL 跑,失败自动改用 mesa/ 软件渲染重启
//!   · hw           → 禁用兜底,只走硬件 GL
//!   · mesa         → 强制软件渲染(兜底重启时由本模块注入,并作为防递归标记)
//!
//! 仅 Windows 编译;Linux 行为不变。

use std::path::PathBuf;
use std::process::Command;

/// 现场开关:hw / mesa / 未设置
const ENV_GL: &str = "FINDANY_GL";
/// exe 同级的软件 OpenGL 目录与库名(Mesa3D for Windows 的 libgl-gdi 目标)
const MESA_DIR: &str = "mesa";
const MESA_DLL: &str = "opengl32.dll";
/// glutin 读的变量:按路径加载 OpenGL 实现(默认 opengl32.dll)
const ENV_GLUTIN_DLL: &str = "GLUTIN_WGL_OPENGL_DLL";

fn mode() -> Option<String> {
    std::env::var(ENV_GL).ok()
}

fn exe_path() -> Option<PathBuf> {
    std::env::current_exe().ok()
}

/// exe 同级的 mesa/opengl32.dll,不存在则不兜底(未随包分发时零行为变化)
fn mesa_dll() -> Option<PathBuf> {
    let dll = exe_path()?.parent()?.join(MESA_DIR).join(MESA_DLL);
    dll.is_file().then_some(dll)
}

/// 硬件 GL 启动失败后:带软件 GL 环境重新拉起自己。
///
/// 返回 true = 子进程已拉起,调用方应立刻返回(不要再报错/重试);
/// 返回 false = 没条件兜底(已在兜底中 / 设为 hw / 没有 mesa 库),调用方按原逻辑处理。
pub fn relaunch_with_mesa() -> bool {
    match mode().as_deref() {
        // 已经在软件渲染里还失败 → 不再递归重启
        Some("mesa") => return false,
        // 显式要求只用硬件 GL
        Some("hw") => return false,
        _ => {}
    }
    let Some(dll) = mesa_dll() else {
        return false;
    };
    let Some(exe) = exe_path() else {
        return false;
    };
    let Some(dir) = dll.parent() else {
        return false;
    };
    // Mesa 的伴随库(libgallium_wgl.dll 等)靠 PATH 解析:LoadLibrary 的搜索顺序是
    // exe 目录 → System32 → PATH,必须把 mesa 目录插到最前面才找得到。
    let path = match std::env::var_os("PATH") {
        Some(old) => {
            let mut v = dir.as_os_str().to_os_string();
            v.push(";");
            v.push(old);
            v
        }
        None => dir.as_os_str().to_os_string(),
    };
    Command::new(exe)
        .args(std::env::args_os().skip(1))
        .env(ENV_GL, "mesa")
        .env(ENV_GLUTIN_DLL, &dll)
        .env("PATH", path)
        // 明确软件光栅(Mesa 默认即 llvmpipe,写死免被现场环境变量改坏)
        .env("GALLIUM_DRIVER", "llvmpipe")
        .spawn()
        .is_ok()
}
