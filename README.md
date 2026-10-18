# UWF Manager

Windows 统一写入筛选器（UWF）的终端用户界面（TUI）管理工具 —— 由 C# WinForms 项目
[UWFPRO](file:///home/gewei/UWFPRO) 以 Rust（Edition 2024）重写，单可执行文件、
零运行时依赖（无 .NET / 无异步运行时）。

设计与实施细节见 [PLAN.md](PLAN.md)。

## 功能

- UWF 筛选器：启用 / 禁用 / 重置全部配置 / 重启 / 关机
- Overlay 缓存：类型与上限配置、预警/临界阈值、**每 2 秒**动态用量监测（Gauge 绿→黄→红）
- 卷保护：逐卷保护切换（当前 / 下次双状态）
- 排除项：文件 / 注册表双栏管理（当前 / 下次双视图）、**txt 列表导入导出**
- 提交工具：文件提交、文件删除提交（先删文件再固化）、注册表提交
- 跨平台可开发：Windows 走 WMI（`wmi` crate，serde 化）；Linux/macOS 走 Mock 仿真数据，
  `cargo run` 即可预览全部界面（含模拟重启后"下次→当前"生效演示）

## 构建

### 依赖

| 用途 | 要求 |
| :--- | :--- |
| 任意平台开发预览 | Rust 1.99+（stable） |
| Windows 本机构建 | 无需额外组件（embed-resource 使用工具链自带资源编译器） |
| Linux 交叉 check / build | `llvm-rc`（或 `x86_64-w64-mingw32-windres`）；构建 exe 另需 `x86_64-w64-mingw32-gcc` 链接器 |

### Linux / macOS 开发预览（Mock 数据）

```bash
cargo run          # 非 Windows 平台自动使用 Mock 后端
cargo run -- --mock  # 任意平台强制 Mock（Windows 上免管理员预览）
```

### Windows 本机构建

```powershell
cargo build --release
# 产物 target\release\uwf_manager.exe 已内嵌 requireAdministrator UAC 清单与图标
```

### Linux 交叉编译

```bash
rustup target add x86_64-pc-windows-gnu

# 编译验证（资源脚本经 build.rs 实际编译，需资源编译器）：
#   ① CentOS/RHEL：sudo dnf install llvm        （提供 /usr/bin/llvm-rc）
#   ② Debian/Ubuntu：sudo apt install llvm      （同上）或 binutils-mingw-w64-x86-64
cargo check --target x86_64-pc-windows-gnu

# 构建 exe（另需 mingw 链接器）：
#   Debian/Ubuntu：sudo apt install gcc-mingw-w64-x86-64
cargo build --release --target x86_64-pc-windows-gnu
```

> build.rs 资源编译器回退链：`RC`/`RC_<target>` 环境变量 → `x86_64-w64-mingw32-windres`
> → PATH 上的 `llvm-rc`（无交叉 gcc 时 .rc 预处理回落宿主编译器，纯宏展开无副作用）。
> UAC 清单为硬性需求，资源编译失败即构建失败。

## 使用

### 权限（UAC）

- 正式版 exe 内嵌 `requireAdministrator` 清单：从非提权终端启动时，Windows 弹 UAC
  确认后进程在**新控制台窗口**中运行（Windows 机制使然，非缺陷）。
- 运行时另有 `TokenElevation` 兜底检测：无清单的 dev 构建以非管理员启动时，
  打印中文引导并以退出码 2 退出；`--mock` 模式不受限。

### UWF 未安装

进入 TUI 引导页（不退出），按提示在**管理员 PowerShell** 执行并重启：

```powershell
Enable-WindowsOptionalFeature -Online -FeatureName Client-UnifiedWriteFilter
```

重启后回到本工具按 `[F5]` 重新检测即可。（UWF 仅 Windows 企业版 / 教育版 / IoT 提供。）

### 快捷键

| 位置 | 按键 | 说明 |
| :--- | :--- | :--- |
| 全局 | `[1-4]` / `[Tab]` / `[Shift+Tab]` | 切换页面 |
| 全局 | `[F5]` | 全部刷新 |
| 全局 | `[?]` | 帮助弹窗 |
| 全局 | `[Q]` / `[Esc]` | 退出（弹窗打开时仅关闭弹窗） |
| [1] 状态概览 | `[E]` `[D]` | 启用 / 禁用筛选器（下次启动生效） |
| | `[R]` | 重置全部 UWF 配置（确认弹窗） |
| | `[S]` | Overlay 配置弹窗 |
| | `[F10]` `[F11]` | 重启 / 关机（确认弹窗） |
| [2] 卷保护 | `[↑/↓]` `[P]` `[U]` | 选择 / 保护 / 取消保护（下次启动生效） |
| [3] 排除项 | `[←/→]` | 文件栏 / 注册表栏焦点切换 |
| | `[S]` | 当前生效 / 下次生效视图切换 |
| | `[A]` `[R]` | 添加文件排除（可 F2 文件选择器）/ 注册表排除 |
| | `[D]` | 删除选中项 |
| | `[I]` `[X]` | 导入 txt / 导出当前视图（`EXCLUSIONLIST_yyyy_MM_dd_HH_mm_ss.txt`） |
| [4] 提交工具 | `[F]` | 选择文件 → 提交（当前会话，立即固化） |
| | `[D]` | 删除提交（确认弹窗；先真实删除文件，再固化删除） |
| | `[R]` | 注册表提交（输入完整路径 `HKLM\...\ValueName`） |

### 关键业务规则

- **R1**：筛选器**当前已启用**时，Overlay 类型与上限锁定（UI 置灰 + 后端校验）；
  预警/临界阈值随时可改。
- **R2/R4**：卷保护切换、排除项增删均作用于**下次会话**，重启后生效；
  查询支持当前 / 下次双视图。
- **R3**：文件提交作用于当前会话；删除提交为两步语义（先删文件，再 WMI 固化删除）。
- **R5**：注册表提交按最后一个 `\` 拆分为键路径 + 值名称。
- **导入格式**：txt 每行一条，`HK` 前缀 → 注册表排除，其余 → 文件排除（绝对路径）。

### 与原版的差异

接入 UI 的扩展（原版有实现但未接线）：`ResetSettings`、`RestartSystem`、`ShutdownSystem`。
有意省略（原版为空壳/损坏，登记备查）：`UWF_Servicing`、`GetOverlayFiles`、
`SetBindByDriveLetter`、`FindExclusion`、`CommitRegistryDeletion`（详见 PLAN §6.6）。

## 开发与验证

```bash
cargo clippy --all-targets -- -D warnings   # 零告警基线（双 target 均通过）
cargo test                                  # 单元 + reducer + UI 渲染断言（23 项）
cargo check --target x86_64-pc-windows-gnu  # Windows 目标编译验证
python3 scripts/tui_smoke.py                # pty 端到端冒烟（虚拟屏幕断言）
```

架构：UI 线程（crossterm poll + 2s tick）⇄ `std::sync::mpsc` ⇄ 后端线程
（actor 模式独占 `UwfBackend`，COM 套间约束天然规避）；全程序无异步运行时，
`unsafe` 仅提权检测一处（模块内显式豁免）。
