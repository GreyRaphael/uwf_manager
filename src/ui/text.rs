//! 界面中文文案与共享标签常量。

pub const LOADING: &str = "正在读取 UWF 系统状态…";
pub const LOAD_FAILED: &str = "读取状态失败，请点击下方「重新加载」或按 [F5] 重试";
pub const BACKEND_LOST: &str = "后台服务线程已意外断开，请重启本程序";
pub const READY_EMPTY: &str = "当前暂无排除规则";

pub const NOTICE_SHORT: &str = "保护开关、缓存模式/容量、分区保护与排除列表均在重启计算机后生效";
pub const OVERLAY_LOCK_HINT: &str =
    "写保护当前已开启：模式与最大容量已锁定（停用保护并重启后可修改）";

pub fn app_title() -> String {
    format!("UWF 管理器 v{}", env!("CARGO_PKG_VERSION"))
}

pub fn protected_zh(value: bool) -> &'static str {
    if value { "保护" } else { "不保护" }
}

// ── UWF 未安装引导页 ──
pub const NI_TITLE: &str = "未检测到 UWF 功能";
pub const NI_LEAD: &str = "当前系统尚未安装或启用 Windows 统一写入筛选器（Unified Write Filter）。";
pub const NI_REASON_TITLE: &str = "常见原因";
pub const NI_REASON_1: &str =
    "• 系统尚未开启 UWF 可选功能（仅 Windows 企业版 / 教育版 / IoT 提供）";
pub const NI_REASON_2: &str = "• WMI 命名空间 root\\standardcimv2\\embedded 暂不可用";
pub const NI_ENABLE_TITLE: &str = "启用步骤（请在管理员 PowerShell 中运行以下命令并重启系统）";
pub const NI_FOOTER: &str = "安装并重启完成后，点击「重新检测」或按 [F5] 刷新状态。";
