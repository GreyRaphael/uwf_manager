/// Overlay 类型（对应 `UWF_OverlayConfig.Type`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayType {
    /// 0 = 内存
    Ram,
    /// 1 = 磁盘
    Disk,
}

impl OverlayType {
    // as_u32/from_u32 由阶段 2 设置弹窗与阶段 3 WMI 映射使用，届时移除该 allow
    #[allow(dead_code)]
    pub fn as_u32(self) -> u32 {
        match self {
            OverlayType::Ram => 0,
            OverlayType::Disk => 1,
        }
    }

    #[allow(dead_code)]
    pub fn from_u32(value: u32) -> Self {
        match value {
            0 => OverlayType::Ram,
            _ => OverlayType::Disk,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            OverlayType::Ram => "内存(RAM)",
            OverlayType::Disk => "磁盘(Disk)",
        }
    }
}

/// Overlay 配置（对应 `UWF_OverlayConfig`，按当前/下次会话各一份）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayConfig {
    pub current_session: bool,
    pub overlay_type: OverlayType,
    pub max_size_mb: u32,
}

/// Overlay 运行时状态（对应 `UWF_Overlay`，2 秒轮询目标）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayRuntime {
    pub available_space_mb: u32,
    pub consumption_mb: u32,
    pub warning_threshold_mb: u32,
    pub critical_threshold_mb: u32,
}

/// 用量等级：Gauge 配色依据（绿/黄/红）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsageLevel {
    Normal,
    Warning,
    Critical,
}

impl OverlayRuntime {
    pub fn usage_level(&self) -> UsageLevel {
        if self.consumption_mb >= self.critical_threshold_mb {
            UsageLevel::Critical
        } else if self.consumption_mb >= self.warning_threshold_mb {
            UsageLevel::Warning
        } else {
            UsageLevel::Normal
        }
    }

    /// R7：用量百分比以 `OverlayConsumption / MaximumSize` 计算（0..=100），
    /// 不用 `MaxSize - AvailableSpace` 推算（磁盘型 Overlay 下不准）。
    pub fn usage_percent(&self, max_size_mb: u32) -> u16 {
        if max_size_mb == 0 {
            return 0;
        }
        ((self.consumption_mb as u64 * 100) / max_size_mb as u64).min(100) as u16
    }
}
