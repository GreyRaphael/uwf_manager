//! 构建脚本：仅 Windows target 下注入 UAC 清单与应用图标。
//!
//! embed-resource 对 `*-pc-windows-gnu` 默认只探测 `x86_64-w64-mingw32-windres`；
//! 无 mingw 工具链的环境（如 CentOS，官方仓库已不提供 mingw64-*）退回 PATH 上的
//! `llvm-rc`（embed-resource 官方支持的 `RC` 环境变量覆盖口）。

fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    if std::env::var_os("RC").is_none()
        && std::env::var_os("RC_x86_64_pc_windows_gnu").is_none()
        && !runnable("x86_64-w64-mingw32-windres")
        && runnable("llvm-rc")
    {
        // SAFETY: 构建脚本主函数单线程，且在任何库读取这些变量之前设置
        unsafe {
            std::env::set_var("RC", "llvm-rc");
            // llvm-rc 路线的 .rc 预处理经 cc-rs 调用 C 预处理器（纯宏展开，
            // 无目标代码生成），无交叉 gcc 时用宿主编译器即可
            if std::env::var_os("CC_x86_64_pc_windows_gnu").is_none()
                && !runnable("x86_64-w64-mingw32-gcc")
                && let Some(host_cc) = ["cc", "gcc", "clang"].into_iter().find(|c| runnable(c))
            {
                std::env::set_var("CC_x86_64_pc_windows_gnu", host_cc);
            }
        }
    }

    // UAC 清单为硬性需求（D5）：缺失即失去自动提权，编译期直接失败
    embed_resource::compile("uwf_manager.rc", embed_resource::NONE)
        .manifest_required()
        .expect("编译 Windows 资源（UAC 清单/图标）失败：请安装 mingw windres 或 llvm-rc");
}

/// 目标程序是否可在 PATH 上执行（spawn 成功即视为可用）
fn runnable(program: &str) -> bool {
    use std::process::{Command, Stdio};
    Command::new(program)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|mut child| {
            let _ = child.kill();
            let _ = child.wait();
        })
        .is_ok()
}
