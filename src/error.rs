//! 业务错误类型：全部 Display 为中文，面向最终用户可读。

use thiserror::Error;

pub type Result<T> = std::result::Result<T, UwfError>;

// 部分变体由阶段 2（InvalidPath）与阶段 3（NotInstalled/WmiMethod/Wmi）接线，届时移除该 allow
#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum UwfError {
    /// UWF 功能未安装或未启用（触发 D6 引导页）
    #[error("UWF 功能未安装或未启用。请在管理员 PowerShell 中执行：Enable-WindowsOptionalFeature -Online -FeatureName Client-UnifiedWriteFilter")]
    NotInstalled,

    /// WMI 方法返回非零 ReturnValue（uwfmgr 风格十六进制错误码）
    #[error("WMI 方法 {method} 调用失败，错误码: 0x{code:08X}")]
    WmiMethod { method: &'static str, code: u32 },

    /// R1：筛选器当前启用时 Overlay 类型/最大容量锁定
    #[error("筛选器当前已启用，{what}不可修改（仅预警/临界阈值可改）")]
    FilterCurrentlyEnabled { what: &'static str },

    #[error("未找到卷 {0} 对应的 UWF 实例")]
    VolumeNotFound(String),

    #[error("无效的路径: {0}")]
    InvalidPath(String),

    #[error("文件操作失败: {0}")]
    Io(#[from] std::io::Error),

    #[cfg(windows)]
    #[error("WMI 错误: {0}")]
    Wmi(Box<wmi::WMIError>),
}

#[cfg(windows)]
impl From<wmi::WMIError> for UwfError {
    fn from(e: wmi::WMIError) -> Self {
        UwfError::Wmi(Box::new(e))
    }
}
