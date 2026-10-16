/// 文件排除项（对应 `UWF_Volume.GetExclusions` 返回的 `ExcludedFiles`）。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct FileExclusion {
    /// 盘符，如 "C:"
    pub drive_letter: String,
    /// 相对路径（R6：保留前导 `\`，如 `\Windows\Temp`）
    pub file_rel_path: String,
}

impl FileExclusion {
    pub fn new(drive_letter: String, file_rel_path: String) -> Self {
        Self { drive_letter, file_rel_path }
    }

    /// 展示用绝对路径，如 `C:\Windows\Temp`
    pub fn display(&self) -> String {
        format!("{}{}", self.drive_letter, self.file_rel_path)
    }
}
