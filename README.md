# UWF 管理器 (UWF Manager)

Windows 统一写入筛选器（Unified Write Filter, UWF）的现代桌面管理工具 —— 由原 C# WinForms 项目 [UWFPRO](file:///home/gewei/UWFPRO) 以 Rust（Edition 2024）与 **`egui` / `eframe` `0.37`** 重构，单可执行文件、无 .NET 与异步运行时依赖。

> 历史 TUI 阶段的 WMI 接口调研与 R1–R7 业务规则定义见归档文档 [PLAN.md](PLAN.md)。

## 功能亮点

- **现代 Fluent 亮色桌面仪表盘**：
  - **顶部全局命令栏**：位于右侧工作区上方（执行中进度指示、`提交操作…`、`电源 (重启/关机)`、`日志` 与 `帮助 (F1)`），让左侧控制栏向上延展至顶部。
  - **左侧控制与监控边栏**：**筛选器控制卡**（启用/停用总开关 + 重置配置）、**写入缓存 (Overlay) 卡**（顶部实时用量进度条 + 健康/预警/临界三态色标 + `内存 RAM / 磁盘 Disk` 分段切换 + 最大容量/预警/严重阈值表单）与左下角**系统环境卡**（展示当前 Windows 版本及企业版/教育版支持提示）。
  - **右侧宽幅工作区**：**受保护分区表**（展示盘符、当前/重启后保护状态与每行一键 Toggle Switch 开关，自动处理新分区首次保护的 `UWF_Volume` 实例注册与同盘符多实例消歧）与**排除项规则列表**（直接管理重启后配置，展示类型徽章与「已生效 / 待重启生效」标签、一键撤销修改回到当前生效状态，以及系统原生文件/文件夹选择器和 UTF-8 `.txt` 导入导出）。
  - **底部紧凑状态栏与操作日志弹窗**：底部单行展示最新操作状态与生效提示；点击顶部「日志」按钮弹出操作记录窗口，可倒序浏览与一键复制完整执行日志。
- **跨平台可开发预览**：Windows 实机走 WMI（`wmi` crate，serde 化）；Linux / macOS 自动走 `MockUwfBackend` 仿真数据，`cargo run` 或 `--mock` 即可启动桌面窗交互预览全部流程。

## 构建

### 环境要求

| 用途 | 要求 |
| :--- | :--- |
| 任意平台开发预览 | Rust 1.99+（stable） |
| Windows 本机构建 | VS Build Tools / Windows SDK（提供 `rc.exe` 供 `embed-resource` 编译清单与图标） |
| Linux 交叉 check / build | `llvm-rc`（或 `x86_64-w64-mingw32-windres`）；构建 exe 另需 `x86_64-w64-mingw32-gcc` |

### Linux / macOS 开发预览（Mock 数据）

```bash
cargo run            # 非 Windows 平台自动使用 Mock 后端
cargo run -- --mock  # 任意平台强制 Mock
```

> Windows 注意：Release 构建启用了 `windows_subsystem = "windows"` 以隐藏控制台黑框，`--mock` 仅跳过运行时权限检测，**不会改变内嵌的 `requireAdministrator` 清单**。

### Windows 本机构建

```powershell
cargo build --release
# 产物 target\release\uwf_manager.exe 已内嵌 UAC 清单与应用图标，无控制台窗口
```

### Linux 交叉编译（Windows 目标）

```bash
rustup target add x86_64-pc-windows-gnu
cargo check --target x86_64-pc-windows-gnu
cargo build --release --target x86_64-pc-windows-gnu
```

## 使用与交互说明

### 权限与未安装引导

- Release 产物内嵌 `requireAdministrator` UAC 清单；若以无管理员权限方式运行非 `--mock` 构建，程序会在启动图形界面前弹出系统错误对话框提示并退出（退出码 2）。
- 若当前 Windows 未启用 UWF 可选功能，界面将展示引导卡片，提供一键复制的管理员 PowerShell 启用命令：

```powershell
Enable-WindowsOptionalFeature -Online -FeatureName Client-UnifiedWriteFilter
```

执行并重启系统后，点击引导页中的「重新检测」或按 `[F5]` 即可进入主界面。

### 快捷键

| 按键 | 说明 |
| :--- | :--- |
| `F5` | 刷新全部 UWF 状态 |
| `F1` | 打开/关闭使用说明弹窗 |
| `Esc` | 关闭当前弹窗 |
| `Delete` | 无文本聚焦时移除当前选中的排除项 |
| `Enter` | 确认输入/确认弹窗 |

### 关键业务规则

- **R1**：筛选器**当前会话已启用**时，缓存模式与最大容量锁定（界面禁用并提示，后端同步校验）；警告/严重阈值随时可调。
- **R2 / R4**：筛选器开关、缓存配置、卷保护切换及排除项增删均作用于**下次会话**，重启后生效；排除项行内标注「已生效 / 待重启生效」，支持一键「撤销修改」恢复为当前生效规则。
- **R3**：文件修改提交与文件删除提交（两步安全确认：先物理删除，再 WMI 固化删除）立即生效；删除提交强制两次交互确认。
- **R5**：注册表提交按最后一个 `\` 拆分为键路径与值名称。
- **导入格式**：`.txt` 每行一条，`HK` 前缀 → 注册表排除，其余 → 文件/目录绝对路径排除。

## 开发与验证

```bash
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked   # 包含模型、WMI 消歧逻辑、状态机及 egui_kittest 无头 UI 交互测试
cargo build --locked
```

架构：UI 线程运行 `eframe` / `egui`（含非阻塞 `rfd` 原生文件对话框后台线程）⇄ `std::sync::mpsc` ⇄ 后端 actor 线程（独占 `dyn UwfBackend`，天然满足 COM 套间约束）；后端回包通过 `egui::Context::request_repaint` 立即唤醒重绘。运行时 `unsafe` 仅 `backend/windows/elevate.rs` 一处（模块内显式 allow）。
