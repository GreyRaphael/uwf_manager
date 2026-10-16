/// 卷保护信息（R2：`UWF_Volume` 当前/下次两次查询按盘符合并）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeInfo {
    /// 盘符，如 "C:"
    pub drive_letter: String,
    pub current_protected: bool,
    pub next_protected: bool,
}
