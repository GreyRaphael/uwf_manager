/// UWF 筛选器状态（对应 WMI `UWF_Filter`，单实例、无会话分裂）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FilterState {
    /// 当前会话是否已启用
    pub current_enabled: bool,
    /// 下次启动是否启用（Enable/Disable 均修改此侧，重启生效）
    pub next_enabled: bool,
}
