# ds41f_review.md

> UWF Manager（Rust 1.99 / Edition 2024 / ratatui TUI）代码 Review 与修复方案
>
> Review 日期：2026-10-18
> Review 范围：`src/` 全部源码、`build.rs`、`app.manifest`、`uwf_manager.rc`、`Cargo.toml`、`README.md`、`PLAN.md`、`scripts/tui_smoke.py`
> 结论口径：按“是否会导致错误状态 / 数据丢失 / 卡死 / 可维护性下降”排序，而不是按代码风格排序。
> 说明：本次 review 只做分析，没有修改仓库源码。P0 中的复现验证是在 `/tmp` 临时副本中增加断言完成的，仓库本身保持干净。

---

## 0. 结论摘要

项目整体架构是成立的：

- UI 线程 + 后端 actor 线程，用 `std::sync::mpsc` 通信；
- `UwfBackend` trait 隔离 Windows WMI 与 Mock；
- `WMIConnection` 的 `!Send` 约束被正确地关在后端线程内；
- 没有异步运行时，没有把 WMI 调用放进 UI 线程；
- Mock 实现了“下次会话状态机”，Linux 上可以完整演示 R1–R4；
- 默认 clippy 干净，23 个单测和 2 个 pty 冒烟场景都能通过。

但也有几处**真实的正确性问题**，主要集中在状态机边界和 WMI 两阶段操作：

1. **电源动作在 `pending` 时被静默丢弃，却仍然处于 armed 状态**，下一条无关的 `OpOk` 会导致误退出或假重启。
2. **查询类错误会清掉“变更在途”锁和电源动作**，因为没有请求关联 id，`pending` 是全局 bool。
3. **“UWF 未安装”引导页只替换渲染，不替换键盘路由**，仍能发出管理命令。
4. **删除提交先删除真实文件，再校验 UWF 卷**，可能出现“文件已删、删除未固化”的不可回滚状态。
5. **Restart/Shutdown 不校验 WMI `ReturnValue`，成功后还做全量刷新**，可能假成功或卡在关机流程。
6. **变更在途时 `q/Esc` 仍可直接退出，后端线程不 join**，可能强杀正在执行的 WMI 调用。

这 6 项建议作为 P0 先修。其余 P1 是语义/UX 和错误处理问题，P2 是 dead code、smell 和 over-design。

---

## 1. Review 方法与验证基线

### 1.1 实际执行的验证

| 命令 | 结果 | 说明 |
| :--- | :--- | :--- |
| `cargo test` | 23 passed | 当前单测全部通过 |
| `cargo clippy --all-targets -- -D warnings` | 通过 | 默认 clippy 零告警 |
| `python3 scripts/tui_smoke.py` | 18 项断言全部 PASS | 需要先存在 `target/debug/uwf_manager` |
| `cargo fmt --check` | **失败，约 62 处 diff** | 仓库没有保持 rustfmt 基线 |
| `cargo clippy --all-targets -- -W clippy::pedantic` | 约 70 条 warning | 多数是 `unnested_or_patterns`、分号、文档反引号等机械问题 |
| `cargo clippy --target x86_64-pc-windows-gnu --all-targets -- -D warnings` | 通过（临时副本） | 临时把 `build.rs` 的资源编译替换为空实现，以绕过本机缺少 `llvm-rc` 的问题 |
| 仓库内直接 `cargo check --target x86_64-pc-windows-gnu` | 受 RC 工具链限制 | 与 README 的描述一致：需要 `llvm-rc` 或 `windres` |

### 1.2 Review 限制

- 本机没有 Windows + UWF 真机环境，**WMI 运行时行为没有在真机上验证**；Windows 分支只做了编译/clippy 级验证。
- 没有在 Windows Terminal / ConPTY 下做真实键盘和终端尺寸测试。
- 没有对 `wmi` crate 的 `GetExclusions` 嵌入对象数组做真机首验。
- 因此，涉及 WMI 方法语义的结论以代码和 PLAN 为依据；涉及文件删除、状态机的结论已在临时副本中复现。

---

## 2. 架构评价

### 2.1 做得好的部分

- **线程边界清晰**：`BackendFactory` 在后端线程内构造 `Box<dyn UwfBackend>`，避免 `WMIConnection` 跨线程；`UwfBackend` 不要求 `Send`，注释也解释了原因。
- **Mock 有价值**：不是简单返回常量，而是完整模拟 current/next 两侧状态，使 Linux 上可以测试 R1–R4。
- **UI 与后端解耦**：`BackendCmd` / `BackendEvent` 的方向明确，UI 不直接碰 WMI。
- **错误文案面向用户**：`UwfError` 基本是中文，WMI 错误码按十六进制展示，符合工具类应用的使用场景。
- **终端安全**：`#![deny(unsafe_code)]`，仅 `elevate.rs` 模块显式允许 unsafe；panic hook 的顺序也考虑了终端恢复。
- **Windows 细节**：UAC manifest、TokenElevation 兜底、D6 引导页、`--mock` 预览入口都考虑到了。

### 2.2 主要架构风险

1. **协议没有请求关联 id**：`BackendEvent` 只是事件流，UI 无法判断某个 `OpOk` / `OpErr` 属于哪条命令。`pending` 是一个全局 bool，导致查询错误、快照、电源动作互相污染。
2. **`refresh_all` 是副作用函数**：只返回 `()`，失败时通过 channel 发 `OpErr`，调用方无法决定“动作成功但刷新失败”该如何呈现。
3. **状态有多个来源**：`App.overlay_runtime` 与 `App.snapshot.overlay_runtime` 同时存在；`Snapshot` 和 `OverlayRuntime` 事件更新不同的副本。
4. **UI 状态机分散**：`on_key`、各 modal 的 `on_key`、`send_mutation`、`execute_confirm` 各自处理“是否允许操作”，没有统一 guard。
5. **WMI 两阶段操作没有事务/补偿**：删除提交、Overlay 多项配置、导入都是多步操作，部分失败时缺少明确的部分成功模型。

---

## 3. 问题总览

| ID | 级别 | 问题 | 主要位置 |
| :--- | :--- | :--- | :--- |
| P0-1 | 严重 | pending 时电源动作被丢弃但仍 armed，误退出/假重启 | [app.rs](src/app.rs#L157-L162)、[app.rs](src/app.rs#L500-L510)、[app.rs](src/app.rs#L236-L246) |
| P0-2 | 严重 | 查询错误清空 pending 与 power_action_pending | [app.rs](src/app.rs#L202-L230)、[backend/mod.rs](src/backend/mod.rs#L190-L193) |
| P0-3 | 严重 | 未安装页不拦按键，仍可发管理命令 | [app.rs](src/app.rs#L293-L314)、[ui/mod.rs](src/ui/mod.rs#L33-L45) |
| P0-4 | 严重 | 删除提交先删文件后校验卷，可能部分成功 | [windows/mod.rs](src/backend/windows/mod.rs#L311-L326) |
| P0-5 | 严重 | Restart/Shutdown 不校验 ReturnValue 且成功后刷新 | [windows/mod.rs](src/backend/windows/mod.rs#L185-L196)、[backend/mod.rs](src/backend/mod.rs#L271-L276) |
| P0-6 | 严重 | pending 时 q/Esc 直接退出，后端线程不 join | [app.rs](src/app.rs#L298-L299)、[main.rs](src/main.rs#L43-L50) |
| P1-1 | 高 | 变更后的刷新失败被后续 OpOk 覆盖 | [backend/mod.rs](src/backend/mod.rs#L318-L330) |
| P1-2 | 高 | “当前生效”视图仍可增删，但命令只写 next | [app.rs](src/app.rs#L395-L397)、[app.rs](src/app.rs#L557-L580) |
| P1-3 | 高 | Gauge 使用 next 上限计算当前 runtime 百分比 | [dashboard.rs](src/ui/views/dashboard.rs#L86-L94) |
| P1-4 | 高 | Overlay 锁定字段仍参与解析，缺少业务校验 | [overlay_settings.rs](src/ui/components/overlay_settings.rs#L90-L101) |
| P1-5 | 高 | 导入全部失败仍发 OpOk | [backend/mod.rs](src/backend/mod.rs#L357-L366) |
| P1-6 | 高 | 单卷 GetExclusions 失败被静默跳过 | [windows/mod.rs](src/backend/windows/mod.rs#L331-L349) |
| P1-7 | 中 | list_volumes 只以 current 为基准，和 PLAN R2 不一致 | [windows/mod.rs](src/backend/windows/mod.rs#L264-L285) |
| P1-8 | 中 | `--mock` 仍受 requireAdministrator manifest 约束 | [app.manifest](app.manifest#L7-L10)、[README.md](README.md#L29-L34) |
| P1-9 | 中 | overlay_runtime 有两份状态，增量事件只更新一份 | [app.rs](src/app.rs#L100-L101)、[app.rs](src/app.rs#L204-L213) |
| P1-10 | 中 | 后端线程无监督，send/try_recv 错误被忽略 | [main.rs](src/main.rs#L43-L50)、[app.rs](src/app.rs#L152-L154) |
| P1-11 | 中 | F5/overlay 回包不能唤醒 poll，最多延迟 2 秒 | [app.rs](src/app.rs#L169-L189) |
| P1-12 | 中 | 文件选择器无滚动、盘符双反斜杠、A-Z 同步探测 | [file_picker.rs](src/ui/components/file_picker.rs#L182-L239) |
| P1-13 | 中 | 输入框接受 Ctrl/Alt 字符，光标偏一格 | [input_dialog.rs](src/ui/components/input_dialog.rs#L68-L70) |
| P1-14 | 中 | `C:foo` / `C:/foo` 归一化错误 | [backend/mod.rs](src/backend/mod.rs#L398-L417) |
| P1-15 | 中 | NotInstalled 映射过宽，权限/临时 WMI 错误被误报 | [windows/mod.rs](src/backend/windows/mod.rs#L49-L58) |
| P1-16 | 中 | 选择按索引保存，刷新后可能指向另一项 | [app.rs](src/app.rs#L259-L267) |
| P1-17 | 中 | 多步 Overlay 配置可能部分成功但只报一个错 | [backend/mod.rs](src/backend/mod.rs#L203-L219) |
| P1-18 | 中 | Mock 读方法有副作用、删除不校验卷 | [mock.rs](src/backend/mock.rs#L143-L152)、[mock.rs](src/backend/mock.rs#L232-L236) |
| P1-19 | 中 | 导入/导出编码与覆盖策略 | [backend/mod.rs](src/backend/mod.rs#L334-L390) |
| P2-1 | 低 | 日志历史完全没展示 | [app.rs](src/app.rs#L22-L25)、[app.rs](src/app.rs#L248-L257) |
| P2-2 | 低 | `OverlayConfig.current_session` 只写不读 | [overlay.rs](src/model/overlay.rs#L36-L42) |
| P2-3 | 低 | 三个 WMI `CurrentSession` 字段只写不读 | [classes.rs](src/backend/windows/classes.rs#L30-L60) |
| P2-4 | 低 | `UwfError` 整体 `allow(dead_code)` 且注释过期 | [error.rs](src/error.rs#L7-L9) |
| P2-5 | 低 | 未使用的 `Win32_UI_Shell` feature | [Cargo.toml](Cargo.toml#L16-L21) |
| P2-6 | 低 | `InputOutcome::Browse(_)` 是不可达分支 | [app.rs](src/app.rs#L472-L478) |
| P2-7 | 低 | PLAN 声称保留 `RemoveAllExclusions`，代码没有 | [PLAN.md](PLAN.md#L329-L333) |
| P2-8 | 低 | `FullSnapshot` 的 Option/Vec 加载语义不一致 | [backend/mod.rs](src/backend/mod.rs#L73-L85) |
| P2-9 | 低 | 文本层 D7 没有真正落地 | [text.rs](src/ui/text.rs#L1-L8) |
| P2-10 | 低 | `Tab::index()` 与 `ALL` 顺序强耦合 | [app.rs](src/app.rs#L35-L61) |

---

## 4. P0 详细问题与修复方案

### P0-1：pending 时电源动作被静默丢弃，但仍然 armed

#### 现象

在 [app.rs](src/app.rs#L500-L510)：

```rust
ConfirmAction::Restart => {
    self.power_action_pending = Some(PowerAction::Restart);
    self.send_mutation(BackendCmd::RestartSystem);
}
```

而 [send_mutation](src/app.rs#L157-L162) 在 `self.pending == true` 时直接 no-op：

```rust
fn send_mutation(&mut self, cmd: BackendCmd) {
    if !self.pending {
        self.pending = true;
        self.send(cmd);
    }
}
```

触发序列：

1. 用户按 `E`，发送 `EnableFilter`，`pending = true`；
2. 用户立刻按 `F10`。`on_dashboard_key` 没有 pending 检查，确认弹窗正常打开；
3. 用户按 Enter。`execute_confirm(Restart)` 先把 `power_action_pending` 设为 `Some(Restart)`，随后 `send_mutation` 丢弃 `RestartSystem`；
4. 前一条 `EnableFilter` 的 `OpOk` 到达，进入 [on_power_action_ok](src/app.rs#L236-L246)；
5. Windows 下 `should_quit = true`，**没有重启却退出 TUI**；Mock 下打印“（模拟）已重启”，**假成功**。

#### 复现

已在临时副本中加入测试，构造 `pending = true` 后调用 `execute_confirm(Restart)`：

- `cmd_rx.try_recv()` 为空，确认命令没有发出；
- `power_action_pending` 仍为 `Some`；
- 再注入一条无关的 `OpOk`，`should_quit` 变为 `true`。

#### 影响

- Windows：误退出，且用户以为重启已发出；
- Mock：日志出现不存在的“已重启”；
- 更一般地：任何“先置状态、后发送”的动作都有同样风险。

#### 修复方案

**推荐（与 P0-2 一起做）**：引入请求 id，`send_mutation` 返回 ticket；只有发送成功才 arm 电源动作。

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestId(u64);

pub struct App {
    next_request_id: u64,
    mutation: Option<RequestId>,
    power_action: Option<(RequestId, PowerAction)>,
    // ...
}

fn begin_mutation(&mut self, cmd: BackendCmd) -> Option<RequestId> {
    if self.mutation.is_some() {
        return None;
    }
    let id = RequestId(self.next_request_id);
    self.next_request_id += 1;
    self.mutation = Some(id);
    if self.cmd_tx.send(BackendCommand { id, cmd }).is_err() {
        self.mutation = None;
        return None;
    }
    self.log(false, "操作已提交，等待后端完成");
    Some(id)
}

fn execute_confirm(&mut self, action: ConfirmAction) {
    match action {
        ConfirmAction::Restart => {
            let Some(id) = self.begin_mutation(BackendCmd::RestartSystem) else {
                self.log(false, "有操作正在执行，请稍后再试");
                return;
            };
            self.power_action = Some((id, PowerAction::Restart));
        }
        // Shutdown 同理
        // ...
    }
}
```

**最小修复（不引入 request id 时）**：

```rust
fn try_send_mutation(&mut self, cmd: BackendCmd) -> bool {
    if self.pending {
        return false;
    }
    if self.cmd_tx.send(cmd).is_err() {
        return false;
    }
    self.pending = true;
    true
}

// execute_confirm
if self.try_send_mutation(BackendCmd::RestartSystem) {
    self.power_action_pending = Some(PowerAction::Restart);
} else {
    self.log(false, "有操作正在执行，请稍后再试");
}
```

同时，`F10` / `F11` 打开确认框时也应检查 pending，或在确认时提示，而不是“关闭弹窗、静默丢弃”。

#### 回归测试

```rust
#[test]
fn power_action_not_armed_when_mutation_is_pending() {
    let (mut app, cmd_rx) = ready_app(BackendKind::Windows);
    app.pending = true;
    app.execute_confirm(ConfirmAction::Restart);
    assert!(cmd_rx.try_recv().is_err());
    assert!(app.power_action_pending.is_none());

    app.apply_event(BackendEvent::OpOk("前一条变更成功".into()));
    assert!(!app.should_quit);
}
```

#### 验收标准

- pending 时确认重启/关机，不发送命令、不 arm 电源动作、不关闭弹窗或给出明确提示；
- 只有 `RestartSystem` / `ShutdownSystem` 真正发送成功后，才允许 `on_power_action_ok` 触发退出/模拟重启。

---

### P0-2：查询类错误会清掉“变更在途”锁和电源动作

#### 现象

[apply_event](src/app.rs#L202-L230) 中：

```rust
BackendEvent::OpErr(msg) => {
    self.pending = false;
    self.power_action_pending = None;
    self.log(false, &msg);
}
```

但 `OpErr` 不只来自变更命令：

- 每 2 秒一次的 [RefreshOverlayRuntime](src/backend/mod.rs#L190-L193) 失败会发 `OpErr`；
- `F5` 的 `RefreshAll` 失败会发 `OpErr`；
- `Snapshot` 事件也无条件把 `pending = false`（[app.rs](src/app.rs#L204-L207)）；
- `NotInstalled` 也会把 `pending = false`。

如果查询在变更命令执行期间返回，就会提前解除变更在途锁，允许 UI 发第二条变更；电源动作也被一并清掉。

#### 复现

临时测试：

```rust
let (mut app, _) = ready_app(BackendKind::Mock);
app.pending = true;
app.apply_event(BackendEvent::OpErr("Overlay 轮询失败".into()));
assert!(!app.pending); // 查询错误错误地解锁了
```

#### 根因

`BackendEvent` 没有请求 id，UI 无法判断事件属于哪条命令；`pending` 是单个全局 bool，而不是“当前 mutation 的 id”。

#### 修复方案

**推荐方案：命令/事件加 `RequestId`。**

```rust
pub type RequestId = u64;

pub struct BackendCommand {
    pub id: RequestId,
    pub cmd: BackendCmd,
}

pub enum BackendEvent {
    Snapshot {
        id: RequestId,
        snapshot: Box<FullSnapshot>,
    },
    OverlayRuntime {
        id: RequestId,
        runtime: OverlayRuntime,
    },
    Finished {
        id: RequestId,
        outcome: Outcome,
    },
    NotInstalled {
        id: RequestId,
    },
}

pub enum Outcome {
    Ok(String),
    OkWithRefreshWarning { message: String, warning: String },
    Err(String),
}
```

后端线程只负责回显 id：

```rust
thread::spawn(move || {
    let mut backend = factory();
    while let Ok(BackendCommand { id, cmd }) = rx.recv() {
        if matches!(cmd, BackendCmd::Quit) {
            break;
        }
        handle_cmd(id, &mut *backend, cmd, &tx);
    }
});
```

UI 侧：

```rust
fn apply_event(&mut self, evt: BackendEvent) {
    match evt {
        BackendEvent::Snapshot { id, snapshot } => {
            // 快照永远可以更新显示，但不因为它是查询就清 mutation
            self.snapshot = *snapshot;
            if self.initial_load == Some(id) {
                self.initial_load = None;
            }
            self.not_installed = false;
            self.clamp_selections();
        }
        BackendEvent::OverlayRuntime { id: _, runtime } => {
            self.snapshot.overlay_runtime = Some(runtime);
        }
        BackendEvent::Finished { id, outcome } => {
            if self.mutation == Some(id) {
                self.mutation = None;
            }
            match outcome {
                Outcome::Ok(msg) => {
                    self.log(true, &msg);
                    if let Some((power_id, _)) = self.power_action
                        && power_id == id
                    {
                        self.on_power_action_ok();
                    }
                }
                Outcome::OkWithRefreshWarning { message, warning } => {
                    self.log(true, &message);
                    self.log(false, &warning);
                }
                Outcome::Err(msg) => {
                    self.log(false, &msg);
                    if let Some((power_id, _)) = self.power_action
                        && power_id == id
                    {
                        self.power_action = None;
                    }
                }
            }
        }
        BackendEvent::NotInstalled { id: _ } => {
            self.not_installed = true;
            self.log(false, "UWF 功能未安装或未启用");
        }
    }
}
```

**最小方案（不引入 id）**：

1. 新增 `BackendEvent::QueryFailed(String)`，查询类失败走这个变体；
2. `apply_event` 中 `QueryFailed` 只记日志，不改 `pending`、不清 `power_action_pending`；
3. `Snapshot` 不再清 `pending`；启动加载用单独的 `initial_load: bool`；
4. `NotInstalled` 不再清 `pending`；
5. 只有 `OpOk` / `OpErr`（变更命令专用）才能清 `pending`。

这个最小方案能解决当前 bug，但无法解决“多个 mutation 并发/未来扩展”的问题；推荐还是上 request id。

#### 回归测试

- 查询错误不影响 pending；
- 查询 Snapshot 不影响 pending；
- 电源动作只被对应 id 的成功/失败清除；
- 启动 RefreshAll 用 `initial_load` 表达，不再滥用 `pending`。

---

### P0-3：“UWF 未安装”引导页只替换渲染，不替换键盘路由

#### 现象

[ui/mod.rs](src/ui/mod.rs#L33-L45) 只是：

```rust
if app.not_installed {
    views::not_installed::render(frame, tabs_area, body_area);
} else {
    views::render_tabs(frame, app, tabs_area);
    match app.tab { ... }
}
```

而 [app.rs 的 on_key()](src/app.rs#L293-L314) 完全不知道 `not_installed`，仍按隐藏的 tab 路由。

#### 影响

- `F10` 仍可打开重启确认框；Enter 后仍会向后端发送 `RestartSystem`（临时测试已确认命令发出）；
- `s`、`a`、`d`、`p` 等仍会向“未安装”的后端发管理命令；
- 底部 [render_hotkeys](src/ui/mod.rs#L44-L45) 仍显示 `[Tab]`、`[↑/↓]` 等正常页快捷键；
- 用户可能看到一堆误导性的失败日志，甚至在后端状态恢复时执行真实操作。

真实 Windows 后端是否最终执行，取决于 `filter_instance()` 是否成功；但 UI 层不应该在这个状态下发出任何管理命令。

#### 修复方案

在 `App::on_key` 最前面做状态 guard：

```rust
fn on_key(&mut self, key: KeyEvent) {
    if self.not_installed {
        match key.code {
            KeyCode::F(5) => self.send(BackendCmd::RefreshAll),
            KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => {
                self.should_quit = true;
            }
            KeyCode::Char('?') => self.modal = Some(Modal::Help),
            _ => {}
        }
        return;
    }
    // ... 正常路由
}
```

同时：

- `BackendEvent::NotInstalled` 处理时关闭当前 modal：`self.modal = None;`
- 引导页底部渲染单独的快捷键条，只显示 `[F5] 重新检测 [Q] 退出 [ ? ] 帮助`；
- 如果 `not_installed` 为真，`Tab`、数字键、方向键全部忽略。

#### 回归测试

```rust
#[test]
fn not_installed_blocks_management_keys() {
    let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
    app.not_installed = true;
    for code in [
        KeyCode::F(10),
        KeyCode::Char('e'),
        KeyCode::Char('s'),
        KeyCode::Char('a'),
        KeyCode::Char('d'),
        KeyCode::Char('p'),
    ] {
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE));
        assert!(app.modal.is_none());
        assert!(cmd_rx.try_recv().is_err());
    }
}
```

（测试中注意 `KeyCode` 与 `KeyEvent` 的构造方式；重点是断言没有命令发出。）

---

### P0-4：删除提交先删真实文件，再校验 UWF 卷

#### 现象

[windows/mod.rs](src/backend/windows/mod.rs#L311-L326)：

```rust
fn commit_file_deletion(&mut self, drive_letter: &str, file_rel_path: &str) -> Result<()> {
    let abs_path = format!("{drive_letter}{file_rel_path}");
    match std::fs::remove_file(&abs_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    let path = self.find_volume_path(false, drive_letter)?;
    let out: ReturnOnly = self.exec::<UWF_Volume, _>(
        &path,
        "CommitFileDeletion",
        FileNameParam { FileName: file_rel_path.to_string() },
    )?;
    Self::ensure_ok("CommitFileDeletion", out)
}
```

#### 影响

如果 `find_volume_path(false, drive)` 找不到卷实例，或 WMI 调用失败：

- 文件已经真实删除；
- 删除没有固化到 UWF，下次重启后文件可能出现；
- 没有回滚路径，也没有明确的“部分成功”文案。

#### 修复方案

先做所有可能失败的校验，再执行不可逆删除：

```rust
fn commit_file_deletion(&mut self, drive_letter: &str, file_rel_path: &str) -> Result<()> {
    // 1. 先确认下次会话存在该卷实例；失败则完全不碰文件
    let path = self.find_volume_path(false, drive_letter)?;

    // 2. 再删除真实文件（NotFound 容忍）
    let abs_path = format!("{drive_letter}{file_rel_path}");
    match std::fs::remove_file(&abs_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }

    // 3. 最后调 WMI；失败时明确告知“文件已删除，固化失败”
    let out: ReturnOnly = self
        .exec::<UWF_Volume, _>(
            &path,
            "CommitFileDeletion",
            FileNameParam { FileName: file_rel_path.to_string() },
        )
        .map_err(|e| UwfError::CommitDeleteAfterFileRemoved {
            path: abs_path.clone(),
            message: e.to_string(),
        })?;
    Self::ensure_ok("CommitFileDeletion", out)
        .map_err(|e| UwfError::CommitDeleteAfterFileRemoved {
            path: abs_path,
            message: e.to_string(),
        })
}
```

新增错误变体（示例）：

```rust
#[error("文件 {path} 已删除，但 UWF 固化删除失败：{message}")]
CommitDeleteAfterFileRemoved {
    path: String,
    message: String,
},
```

调用处把底层错误转成字符串即可；如果希望保留 source chain，可以把 `message` 改成 `Box<dyn std::error::Error + Send + Sync>`，但要确认 `UwfError` 在 Windows 下的 `Send/Sync` 约束。

如果不想新增错误类型，至少返回 `UwfError::Io` + 明确文案，但不要伪装成普通的“删除失败”。

#### 回归测试

- 用一个不存在的卷 `Z:` 调 `commit_file_deletion`，断言不会删除真实文件；
- Windows 后端可抽出一个纯函数 `validate_volume_before_delete` 做单测；
- 若 WMI 失败，错误字符串必须包含“文件已删除”/“固化失败”。

---

### P0-5：Restart/Shutdown 不校验 `ReturnValue`，且成功后还做全量刷新

#### 现象

[windows/mod.rs](src/backend/windows/mod.rs#L185-L196)：

```rust
fn restart_system(&mut self) -> Result<()> {
    let path = self.filter_instance()?.path;
    let _: ReturnOnly = self.exec::<UWF_Filter, _>(&path, "RestartSystem", ())?;
    Ok(())
}
```

`shutdown_system` 同样。

而 [backend/mod.rs](src/backend/mod.rs#L271-L276) 走 `mutate`，`mutate_with` 在动作成功后调用 `refresh_all`：

```rust
Ok(msg) => {
    refresh_all(backend, tx);
    send(tx, BackendEvent::OpOk(msg));
}
```

#### 影响

- WMI 返回非零（重启被拒绝/不支持）时，UI 仍报成功，Windows 下直接退出；
- 系统正在关机/重启时又发起一整轮 WMI 查询，可能卡住、延迟退出，甚至终端来不及 restore；
- `refresh_all` 失败还会先发 `OpErr`，再发 `OpOk`，日志最终显示成功（见 P1-1）。

> 定级说明（2026-10-20）：本项包含两半。**“动作成功后仍全量刷新”是确定缺陷**，
> 保留 P0；“必须校验 Restart/Shutdown 的 ReturnValue”是合理的防御性建议，但代码
> 注释明确写着“与原版一致：不校验”，且 Windows 真机上 `RestartSystem` 是否能在
> 重启前返回非零尚未验证。修复时保留 ReturnValue 校验，但验收需标注“待 Windows
> 真机确认”，不要把它当作已经证实的线上后果。

#### 修复方案

1. 恢复 `ReturnValue` 校验：

```rust
fn restart_system(&mut self) -> Result<()> {
    let path = self.filter_instance()?.path;
    let out: ReturnOnly = self.exec::<UWF_Filter, _>(&path, "RestartSystem", ())?;
    Self::ensure_ok("RestartSystem", out)
}

fn shutdown_system(&mut self) -> Result<()> {
    let path = self.filter_instance()?.path;
    let out: ReturnOnly = self.exec::<UWF_Filter, _>(&path, "ShutdownSystem", ())?;
    Self::ensure_ok("ShutdownSystem", out)
}
```

2. 增加“不刷新”的变更出口：

```rust
fn mutate_without_refresh(
    backend: &mut dyn UwfBackend,
    tx: &Sender<BackendEvent>,
    action: impl FnOnce(&mut dyn UwfBackend) -> Result<String>,
) {
    match action(backend) {
        Ok(msg) => send(tx, BackendEvent::OpOk(msg)),
        Err(e) => send_err(tx, e),
    }
}

BackendCmd::RestartSystem => {
    mutate_without_refresh(backend, tx, |b| {
        b.restart_system()?;
        Ok("系统即将重启".into())
    });
}
```

3. UI 收到电源动作 `OpOk` 后立即退出；不要等待 `RefreshAll`。

#### 回归测试

- 用 fake backend 测试 `mutate_without_refresh` 不发送 `Snapshot`；
- Windows 真机测试：如果 `RestartSystem` 返回非零，UI 不能退出，必须显示错误；
- Mock 下确认重启仍能演示 next→current，但不要在真实 shutdown 路径做刷新。

---

### P0-6：变更在途时 `q/Esc` 仍可直接退出，后端线程不 join

#### 现象

[app.rs](src/app.rs#L298-L299)：

```rust
KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => self.should_quit = true,
```

没有检查 `pending`。

[main.rs](src/main.rs#L43-L50)：

```rust
let (factory, kind) = create_backend_factory(force_mock);
let _backend_thread = spawn_backend_thread(factory, cmd_rx, evt_tx);
// ...
let result = app.run(&mut terminal);
```

`_backend_thread` 被 drop，线程 detach；`Quit` 只是发出去，没有 join/ack。

#### 影响

- 用户可能在 WMI 写操作进行中退出，进程结束会中断后端线程；
- 没有机会确认“操作是否已经执行”；
- 与 [PLAN.md](PLAN.md#L370) 的“变更在途时 Esc 仅取消弹窗”不一致。

#### 修复方案

**UI 层**：

```rust
fn request_quit(&mut self) {
    if self.mutation.is_some() {
        self.modal = Some(Modal::Confirm(ConfirmState::new(
            "退出确认",
            "有操作正在执行，退出可能中断 WMI 调用。确定退出？",
            ConfirmAction::Quit,
        )));
        return;
    }
    self.should_quit = true;
}
```

新增 `ConfirmAction::Quit`，确认后设置 `should_quit`。或者更保守：pending 时直接忽略 `q/Esc`，并记录“操作进行中，请稍候”。

**线程层**：

- `spawn_backend_thread` 返回一个带 stop ack 的句柄；
- 主线程发送 `Quit` 后等待 `BackendEvent::Stopped` 或 `recv_timeout(2s)`；
- 超时后恢复终端并退出，但日志/文档说明“后端调用可能被强制中断”。

示例：

```rust
let (stop_tx, stop_rx) = std::sync::mpsc::channel();
let backend_thread = std::thread::spawn(move || {
    let mut backend = factory();
    while let Ok(BackendCommand { id, cmd }) = rx.recv() {
        if matches!(cmd, BackendCmd::Quit) {
            let _ = stop_tx.send(());
            break;
        }
        handle_cmd(id, &mut *backend, cmd, &tx);
    }
});

// exit path
let _ = cmd_tx.send(BackendCommand { id: 0, cmd: BackendCmd::Quit });
let _ = stop_rx.recv_timeout(Duration::from_secs(2));
let _ = backend_thread.join();
```

如果 join 可能永久阻塞（WMI 调用挂死），就只等 ack + 超时，不要无条件 join。

#### 回归测试

- pending 时 `q`/`Esc` 不直接置 `should_quit`；
- 确认退出后发送 `Quit`，并收到 `Stopped`/超时；
- 后端断开时 UI 进入错误态，而不是永久 pending。

---

## 5. P1 详细问题与修复方案

### P1-1：变更后的刷新失败被后续 `OpOk` 覆盖

#### 位置

[backend/mod.rs](src/backend/mod.rs#L318-L330)、[refresh_all](src/backend/mod.rs#L281-L287)。

#### 问题

`refresh_all` 返回 `()`：

```rust
fn refresh_all(...) {
    match collect_snapshot(backend) {
        Ok(snapshot) => send(...),
        Err(UwfError::NotInstalled) => send(NotInstalled),
        Err(e) => send_err(tx, e),
    }
}

fn mutate_with(...) {
    match action(backend) {
        Ok(msg) => {
            refresh_all(backend, tx);
            send(tx, BackendEvent::OpOk(msg));
        }
        Err(e) => send_err(tx, e),
    }
}
```

如果动作成功但刷新失败：

- 先发 `OpErr` / `NotInstalled`；
- 再发 `OpOk`；
- UI 最后显示成功，用户不知道快照已经失败。

#### 修复

让刷新结果成为返回值。若采用 P0-2 的 `RequestId`/`Outcome`，代码可以写成：

```rust
fn refresh_all(
    id: RequestId,
    backend: &mut dyn UwfBackend,
    tx: &Sender<BackendEvent>,
) -> Result<(), UwfError> {
    match collect_snapshot(backend) {
        Ok(snapshot) => {
            send(tx, BackendEvent::Snapshot {
                id,
                snapshot: Box::new(snapshot),
            });
            Ok(())
        }
        Err(UwfError::NotInstalled) => {
            send(tx, BackendEvent::NotInstalled { id });
            Err(UwfError::NotInstalled)
        }
        Err(e) => Err(e), // 由调用方决定如何上报，避免先报错再报成功
    }
}
```

`mutate_with`：

```rust
match action(backend) {
    Ok(msg) => match refresh_all(id, backend, tx) {
        Ok(()) => send(tx, BackendEvent::Finished {
            id,
            outcome: Outcome::Ok(msg),
        }),
        Err(e) => send(tx, BackendEvent::Finished {
            id,
            outcome: Outcome::OkWithRefreshWarning {
                message: msg,
                warning: format!("操作已成功，但状态刷新失败：{e}"),
            },
        }),
    },
    Err(e) => send(tx, BackendEvent::Finished {
        id,
        outcome: Outcome::Err(e.to_string()),
    }),
}
```

如果是独立 `RefreshAll` 命令，刷新失败则直接发 `Finished { id, outcome: Outcome::Err(...) }`；如果是变更命令后的刷新，则发 `OkWithRefreshWarning`，让 UI 同时知道“动作成功”和“刷新失败”。

#### 回归测试

- 构造一个 `collect_snapshot` 失败但 action 成功的 fake backend；
- 断言 UI 收到“动作成功 + 刷新警告”，而不是覆盖成纯成功；
- 断言 `pending` 最终被正确清除。

---

### P1-2：“当前生效”视图里也能增删，但命令永远写 next

#### 位置

[app.rs](src/app.rs#L395-L397)、[delete_selected_exclusion](src/app.rs#L557-L580)、[windows/mod.rs](src/backend/windows/mod.rs#L351-L370)。

#### 问题

`[S]` 切换 `exclusion_session_current`；`A` / `D` / `F2` 的添加删除命令不携带会话，后端 R4 规定只操作 next。

后果：

- 在“当前生效”视图按 `D`，实际删的是 next；
- 当前列表不会变化，用户以为没删掉，可能继续按；
- 下一次切到 next 视图时，发现另一条被删了。

#### 修复

优先方案：**当前视图只读**。

```rust
fn ensure_mutable_exclusions(&mut self) -> bool {
    if self.exclusion_session_current {
        self.log(false, "当前生效视图只读，请按 [S] 切换到下次生效视图");
        return false;
    }
    true
}
```

在 `A` / `R` / `D` / `F2` 分支前调用；`I`（导入）也建议切到 next 或提示。`X`（导出）可以保留，因为它是只读。

备选方案：添加/删除时自动切到 next，并在日志中说明“已切换到下次生效视图”。

#### 回归测试

- current 视图按 `D` 不发命令；
- next 视图按 `D` 发 `RemoveFileExclusion`；
- 切换视图后选中项 clamp 正确。

---

### P1-3：Gauge 使用 next 上限计算当前 runtime 百分比

#### 位置

[dashboard.rs](src/ui/views/dashboard.rs#L86-L94)。

#### 问题

`runtime` 来自 `UWF_Overlay`，代表当前会话；分母却用 `next.max_size_mb`：

```rust
let percent = runtime.usage_percent(next.max_size_mb);
```

若 current=2048、next=4096，实际 512 MB 用量会显示成 12.5%，而不是 25%。临时 UI 测试已复现渲染为 `已用: 512 MB / 4096 MB`。

#### 修复

```rust
let max_size = current.max_size_mb.max(1);
let percent = runtime.usage_percent(max_size);
let gauge = Gauge::default()
    .percent(percent)
    .label(format!(
        "已用: {} MB / {} MB  {}%",
        runtime.consumption_mb, max_size, percent
    ));
```

信息行可以继续同时显示 current/next 上限，但运行时百分比必须用 current。另建议把 `{:.1}%` 改成 `{}%`，因为 `usage_percent` 返回整数，`{:.1}` 永远显示 `.0`。

#### 回归测试

- current=2048、next=4096、consumption=512，断言屏幕包含 `512 MB / 2048 MB` 和 `25%`；
- next 小于 current 时也不应把百分比算超过 100（已有 `min(100)`）。

---

### P1-4：Overlay 提交缺少业务校验（编号拆分说明）

> 修订（2026-10-20）：原描述把“锁定 + 上限为空”当作可达缺陷，但 `collect_snapshot`
> 要么返回全部 `Some`，要么整体失败，`filter_state=Some(current_enabled=true)` 且
> `overlay_config_next=None` 的合成快照在正常流程中不可达。因此该场景只保留为
> 防御性处理，不再作为复现依据。真正可达的问题是 **submit 缺少业务校验**：
> `max_mb > 0`、`warning < critical`、`critical <= max_mb`。

#### 位置

[overlay_settings.rs](src/ui/components/overlay_settings.rs#L42-L64)、[submit](src/ui/components/overlay_settings.rs#L90-L101)。

#### 问题

`submit` 只做 `u32` 解析，没有校验：

- 上限为 `0`；
- `warning >= critical`；
- `critical > max_mb`。

用户可以把明显非法的阈值提交给后端；Windows WMI 可能接受后产生不可预期的 Overlay 行为，或只返回一个难以理解的错误码。

另外，锁定类型/上限时 `submit` 仍无条件解析 `max_size`。正常快照下该字段有预填值，因此不构成用户可见故障；但作为防御性处理，锁定时应使用快照中的原始上限，而不是依赖输入框字符串。

#### 修复

1. 快照未加载完成时，不预填空字符串，而是显式禁用保存并提示“正在加载配置”；
2. 锁定状态下不解析锁定字段，使用保存的原始数值；
3. 无论是否锁定，都校验 `max_mb > 0`、`warning < critical`、`critical <= max_mb`。

```rust
pub struct OverlaySettingsState {
    pub overlay_type: OverlayType,
    pub max_size: String,
    pub locked_max_size: Option<u32>,
    // ...
}

fn submit(&mut self) -> OverlayOutcome {
    let max_mb = if self.lock_type_size {
        match self.locked_max_size {
            Some(v) => v,
            None => {
                self.error = Some("配置尚未加载完成，无法保存".into());
                return OverlayOutcome::Stay;
            }
        }
    } else {
        match self.max_size.parse::<u32>() {
            Ok(v) => v,
            Err(_) => {
                self.error = Some("上限须为 0-4294967295 的整数".into());
                return OverlayOutcome::Stay;
            }
        }
    };

    if max_mb == 0 {
        self.error = Some("上限必须大于 0".into());
        return OverlayOutcome::Stay;
    }

    let warning_mb = match self.warning.parse::<u32>() { /* ... */ };
    let critical_mb = match self.critical.parse::<u32>() { /* ... */ };
    if warning_mb >= critical_mb {
        self.error = Some("预警阈值必须小于临界阈值".into());
        return OverlayOutcome::Stay;
    }
    if critical_mb > max_mb {
        self.error = Some("临界阈值不能大于 Overlay 上限".into());
        return OverlayOutcome::Stay;
    }
    // ...
}
```

#### 回归测试

- 上限为 0：拒绝；
- warning >= critical：拒绝；
- critical > max：拒绝；
- 正常输入：提交；
- 锁定 + 上限字段为空（防御性合成状态）：使用快照原始上限，不报“上限须为整数”。

---

### P1-5：导入全部失败仍发 `OpOk`

#### 位置

[backend/mod.rs](src/backend/mod.rs#L357-L366)。

#### 问题

```rust
Ok((ok, failed)) => {
    let msg = if failed == 0 { ... } else { ... };
    refresh_all(backend, tx);
    send(tx, BackendEvent::OpOk(msg));
}
```

即使 `ok == 0 && failed > 0`，也是绿色成功日志。

#### 修复

```rust
match (ok, failed) {
    (0, 0) => send(tx, BackendEvent::OpOk("导入完成：文件为空".into())),
    (0, n) => send_err(tx, UwfError::ImportAllFailed { failed: n }),
    (n, 0) => send(tx, BackendEvent::OpOk(format!("导入完成：成功 {n} 项"))),
    (n, m) => send(tx, BackendEvent::OpWarn(format!(
        "导入完成：成功 {n} 项，失败 {m} 项"
    ))),
}
```

需要新增 `OpWarn` 事件（黄色）或 `Outcome::Warning`。

#### 回归测试

- 文件只有 `not_a_path`：断言收到 `OpErr`/`Outcome::Err`；
- 部分成功：断言收到 warning，不是纯 `OpOk`；
- 正常：断言成功计数。

---

### P1-6：单卷 `GetExclusions` 失败被静默跳过

#### 位置

[windows/mod.rs](src/backend/windows/mod.rs#L331-L349)。

#### 问题

```rust
let Ok(out) = self.exec::<UWF_Volume, FileExclusionsOut>(...) else {
    continue;
};
if out.ReturnValue != 0 {
    continue;
}
```

任何单卷读取失败都被吞掉。UI 会显示“没有排除项”，但实际可能是 WMI 失败。

#### 修复

方案 A（简单）：任何一卷失败就整体返回错误，避免展示不完整数据。

方案 B（更符合原版）：收集部分结果 + 失败卷列表：

```rust
fn get_file_exclusions(&mut self, current_session: bool) -> Result<Vec<FileExclusion>> {
    let mut items = Vec::new();
    let mut failures = Vec::new();
    for volume in self.volumes_for(current_session)? {
        match self.exec::<UWF_Volume, FileExclusionsOut>(
            &volume.path,
            "GetExclusions",
            (),
        ) {
            Ok(out) if out.ReturnValue == 0 => {
                for file in out.ExcludedFiles.unwrap_or_default() {
                    items.push(FileExclusion::new(volume.DriveLetter.clone(), file.FileName));
                }
            }
            Ok(out) => failures.push(format!(
                "{}: ReturnValue=0x{:08X}",
                volume.DriveLetter, out.ReturnValue
            )),
            Err(e) => failures.push(format!("{}: {e}", volume.DriveLetter)),
        }
    }
    items.sort();
    if !failures.is_empty() {
        // 通过 OpWarn/Snapshot.warnings 上报，不能静默
    }
    Ok(items)
}
```

如果不想改 trait 返回类型，可在 `FullSnapshot` 增加 `warnings: Vec<String>`，由 `collect_snapshot` 汇总，UI 状态栏显示“部分卷读取失败”。

#### 回归测试

- fake backend 让某一卷返回错误，断言结果包含 warning 或整体错误；
- 不会显示成“完全成功且无排除项”。

---

### P1-7：`list_volumes` 只以 current 为基准

#### 位置

[windows/mod.rs](src/backend/windows/mod.rs#L264-L285)。

#### 问题

代码注释说“以当前会话盘符集为准”，但 PLAN R2 要求 current/next 两次查询按盘符合并。若 next 出现 current 不存在的卷，该卷不会显示、也不能 protect/unprotect。

#### 修复

用 `BTreeMap` 做并集：

```rust
fn list_volumes(&mut self) -> Result<Vec<VolumeInfo>> {
    let current = self.volumes_for(true)?;
    let next = self.volumes_for(false)?;

    let mut map: std::collections::BTreeMap<String, VolumeInfo> = BTreeMap::new();

    for v in current {
        let key = v.DriveLetter.to_ascii_uppercase();
        map.insert(key, VolumeInfo {
            drive_letter: v.DriveLetter,
            current_protected: v.Protected.unwrap_or(false),
            next_protected: false,
        });
    }

    for v in next {
        let key = v.DriveLetter.to_ascii_uppercase();
        let entry = map.entry(key).or_insert_with(|| VolumeInfo {
            drive_letter: v.DriveLetter.clone(),
            current_protected: false,
            next_protected: false,
        });
        entry.next_protected = v.Protected.unwrap_or(false);
    }

    Ok(map.into_values().collect())
}
```

需要确认“next-only 卷”在 UWF 语义上是否存在；如果不存在，至少在注释中写明为什么可以只取 current。

#### 回归测试

- fake `volumes_for` 返回 current=[C:], next=[C:, D:]，断言结果包含 D:；
- current/next 大小写不同也能合并。

---

### P1-8：`--mock` 仍受 `requireAdministrator` manifest 约束

#### 位置

[app.manifest](app.manifest#L7-L10)、[main.rs](src/main.rs#L21-L33)、[README.md](README.md#L29-L34)。

#### 问题

manifest 使用：

```xml
<requestedExecutionLevel level="requireAdministrator" uiAccess="false" />
```

`--mock` 只跳过运行时的 `is_elevated()` 检查；Windows 在启动 exe 时仍会按 manifest 弹 UAC。README 的“Windows 上免管理员预览”不成立。

#### 修复

三选一：

1. **只修文档**（最小改动）：明确 `--mock` 仍需管理员，因为它只跳过运行时检查，不改变 manifest。
2. **单独 mock 构建**：增加 build feature 或第二个二进制 target，使用 `asInvoker` manifest，例如 `uwf_manager_mock.exe` / `cargo build --features mock-preview`。
3. **改为 asInvoker + 运行时提权重启**：manifest 用 `asInvoker`，真机模式检测未提权时用 `ShellExecuteEx(runas)` 重新启动；这更复杂，且需要处理新控制台窗口/参数。

推荐短期方案 1 + 中期方案 2；方案 3 改动大，不建议在当前阶段做。

#### 验收

- README 与 `--mock` 实际行为一致；
- 如果有 mock 构建，非管理员终端能直接运行并显示 `[Mock 演示数据]`。

---

### P1-9：`overlay_runtime` 有两份状态

#### 位置

[app.rs](src/app.rs#L100-L101)、[apply_event](src/app.rs#L204-L213)、[overlay_settings.rs](src/ui/components/overlay_settings.rs#L47-L50)。

#### 问题

- `Snapshot` 同时写 `app.overlay_runtime` 和 `app.snapshot.overlay_runtime`；
- `OverlayRuntime` 增量事件只写 `app.overlay_runtime`；
- `OverlaySettingsState::from_snapshot` 读 `snapshot.overlay_runtime`，可能是旧值。

#### 修复

保留一个权威来源，推荐保留 `snapshot.overlay_runtime`：

```rust
BackendEvent::OverlayRuntime(runtime) => {
    self.snapshot.overlay_runtime = Some(runtime);
}
```

删除 `App.overlay_runtime` 字段，UI 统一读 `app.snapshot.overlay_runtime`。如果某些渲染路径更愿意直接读 runtime，可提供一个只读 getter：

```rust
pub fn overlay_runtime(&self) -> Option<OverlayRuntime> {
    self.snapshot.overlay_runtime
}
```

#### 回归测试

- 注入 `Snapshot` 后再注入 `OverlayRuntime`，断言 `from_snapshot` 读到新值；
- UI 渲染的 warning/critical 与 runtime 一致。

---

### P1-10：后端线程无监督，send/try_recv 错误被忽略

#### 位置

[main.rs](src/main.rs#L43-L50)、[backend/mod.rs](src/backend/mod.rs#L171-L185)、[app.rs](src/app.rs#L152-L154)、[app.rs](src/app.rs#L196-L200)。

#### 问题

- `send()` 忽略 `SendError`；
- `drain_backend_events` 只处理 `Ok`，忽略 `Disconnected`；
- 后端线程 panic 后 UI 可能永远停在 pending；
- `JoinHandle` 被 drop，线程 detach。

#### 修复

```rust
fn send(&self, cmd: BackendCmd) -> Result<(), String> {
    self.cmd_tx
        .send(cmd)
        .map_err(|_| "后端线程已退出".to_string())
}

fn drain_backend_events(&mut self) {
    loop {
        match self.evt_rx.try_recv() {
            Ok(evt) => self.apply_event(evt),
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => {
                self.backend_lost = true;
                self.pending = false;
                self.log(false, "后端线程已退出，请重启应用");
                break;
            }
        }
    }
}
```

`begin_mutation` 只在 `send` 成功时置 pending；UI 渲染 `backend_lost` 时禁用管理键。

#### 回归测试

- drop `evt_tx`/`cmd_rx` 模拟后端退出，断言 UI 进入错误态；
- `send` 失败时不置 pending。

---

### P1-11：F5/overlay 回包不能唤醒 `poll`，最多延迟 2 秒

#### 位置

[app.rs](src/app.rs#L169-L189)。

#### 问题

`event::poll` 只等待终端事件；后端回包不会唤醒它。`pending` 时把超时缩短到 50 ms，但 `F5` 和 2 秒 overlay 刷新没有设置 pending，回包可能等到下一次 2 秒超时才被 drain。

#### 修复

方案 A（简单）：给所有“等待回包”的请求设置 `waiting_for_backend` 计数，并沿用 50 ms poll。缺点是查询也会让 UI 以 20 fps 重绘。

方案 B（推荐）：增加一个终端事件读取线程，把终端事件和后端事件汇入同一个 channel：

```rust
enum UiEvent {
    Terminal(crossterm::event::Event),
    Backend(BackendEvent),
}

let (ui_tx, ui_rx) = std::sync::mpsc::channel();

std::thread::spawn(move || {
    loop {
        match crossterm::event::read() {
            Ok(evt) => {
                if ui_tx.send(UiEvent::Terminal(evt)).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
});

// 主循环
match ui_rx.recv_timeout(tick_timeout) {
    Ok(UiEvent::Terminal(Event::Key(key))) => { /* ... */ }
    Ok(UiEvent::Backend(evt)) => self.apply_event(evt),
    Err(RecvTimeoutError::Timeout) => { /* tick */ }
    Err(RecvTimeoutError::Disconnected) => { /* 退出 */ }
}
```

这样后端回包立即被处理，终端事件也不丢。注意 `event::read` 在 raw mode 下阻塞，退出时要确保线程不阻止进程结束。

#### 回归测试

- 发送 F5 后 100 ms 内注入 Snapshot，断言 UI 已经应用；
- overlay 回包不再等到下一次 2 秒 tick。

---

### P1-12：文件选择器无滚动、盘符双反斜杠、A-Z 同步探测

#### 位置

[file_picker.rs](src/ui/components/file_picker.rs#L182-L239)。

#### 问题

1. `List::new(items)` + 手写 selected，没有 `ListState` offset；目录项超过可视区域时，选中项移出屏幕。
2. `list_drives` 的 `name` 是 `C:\`，渲染时又 `format!("{}{sep}", entry.name)`，Windows 显示 `C:\\`。
3. 打开选择器时同步 `Path::exists()` 探测 A–Z，遇到网络盘/软驱可能卡 UI。
4. 没有 PageUp/PageDown/Home/End，长目录只能逐行移动。

#### 修复

1. 在 `FilePickerState` 增加 `list_state: ListState`，渲染用 `render_stateful_widget`；选择变化时同步 `list_state.select(Some(selected))`。
2. 盘符名不拼接分隔符：

```rust
// list_drives
name: format!("{}:", letter as char), // C:
path: PathBuf::from(format!("{}:\\", letter as char)), // C:\
```

或渲染时：

```rust
let name = if entry.name.ends_with(std::path::MAIN_SEPARATOR) {
    entry.name.clone()
} else {
    format!("{}{}", entry.name, sep)
};
```

3. 使用 Win32 `GetLogicalDrives`（需要额外 feature）或 `std::fs::read_dir` 不能直接列根；至少把探测放到启动时后台线程，避免每次打开卡 UI。
4. 增加 `PageUp`/`PageDown`/`Home`/`End`，按可视高度移动。

#### 回归测试

- entries 30 项、列表高度 10，移动到第 20 项时 `ListState.offset` 使选中项可见；
- Windows 盘符列表渲染不出现 `C:\\`（需要 Windows 单测或纯函数测试）；
- 打开选择器不阻塞 UI（人工测试）。

---

### P1-13：输入框接受 Ctrl/Alt 字符，光标偏一格

#### 位置

[input_dialog.rs](src/ui/components/input_dialog.rs#L68-L70)、[render](src/ui/components/input_dialog.rs#L96-L98)。

#### 问题

- `KeyCode::Char(c)` 不区分 modifier；Ctrl+C 会插入 `c`，而不是取消/退出；
- 光标 `rect.x + 4` 按 `"> "` + border 计算应为 `rect.x + 3`；
- 没有横向滚动，长路径会溢出。

#### 修复

```rust
KeyCode::Char(c) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) => {
    self.buffer.push(c);
    InputOutcome::Stay
}
```

光标：

```rust
use unicode_width::UnicodeWidthStr;

let cursor_x = rect.x
    + 3
    + state.buffer.width() as u16;
frame.set_cursor_position(Position::new(
    cursor_x.min(rect.x + rect.width.saturating_sub(2)),
    rect.y + 3,
));
```

如果不想新增 `unicode-width` 直接依赖，可以用 ratatui 的 `Line` 宽度工具，或先按 ASCII 估算并注释限制。长路径场景建议只显示 buffer 的尾部（横向滚动）。

#### 回归测试

- Ctrl+C 不插入字符；
- 输入 "abc" 时光标在第 6 列（border 1 + "> " 2 + 3）；
- CJK 输入不越界（若做宽度支持）。

---

### P1-14：`C:foo` / `C:/foo` 归一化错误

#### 位置

[backend/mod.rs](src/backend/mod.rs#L398-L417)。

#### 问题

当前逻辑：

```rust
let rel = &path[2..];
let rel = if rel.is_empty() {
    "\\".to_string()
} else if rel.starts_with('\\') {
    rel.to_string()
} else {
    format!("\\{rel}")
};
```

- `C:foo` 被改写成 `C:\foo`；Windows 的 `C:foo` 是“C 盘当前目录下的 foo”，语义不同；
- `C:/foo` 被改写成 `\/foo`，传给 WMI 可能非法；
- `C::\foo` 之类的输入也没有被拒绝。

#### 修复

只接受真正的绝对路径，并统一分隔符：

```rust
pub fn split_absolute_path(path: &str) -> Result<(String, String)> {
    let bytes = path.as_bytes();
    if bytes.len() < 2 || bytes[1] != b':' || !bytes[0].is_ascii_alphabetic() {
        #[cfg(not(windows))]
        if let Some(rest) = path.strip_prefix('/') {
            return Ok(("C:".into(), format!("\\{}", rest.replace('/', "\\"))));
        }
        return Err(UwfError::InvalidPath(path.into()));
    }

    let drive = path[..2].to_ascii_uppercase();
    let rest = &path[2..];

    if rest.is_empty() {
        return Ok((drive, "\\".into()));
    }

    // 必须是 C:\... 或 C:/...，拒绝 C:foo（drive-relative）
    if !(rest.starts_with('\\') || rest.starts_with('/')) {
        return Err(UwfError::InvalidPath(format!(
            "必须是绝对路径（如 C:\\Windows\\Temp）: {path}"
        )));
    }

    let mut rel = rest.replace('/', "\\");
    if !rel.starts_with('\\') {
        rel.insert(0, '\\');
    }
    // 可选：拒绝 / 归一化 "..", "//" 等
    Ok((drive, rel))
}
```

注意：如果产品确实要支持 `C:foo`，应该在 UI 中明确提示“相对于 C 盘当前目录”，不能悄悄改成 `C:\foo`。当前实现最安全的是拒绝。

#### 回归测试

- `C:\Windows\a` -> `("C:", "\Windows\a")`；
- `C:/Windows/a` -> `("C:", "\Windows\a")`；
- `C:foo` -> `InvalidPath`；
- `C:` -> `("C:", "\")`；
- `C::\foo` -> `InvalidPath`；
- 非 Windows 的 `/home/x` 仍映射到 Mock `C:`。

---

### P1-15：NotInstalled 映射过宽

#### 位置

[windows/mod.rs](src/backend/windows/mod.rs#L49-L58)、[filter_instance](src/backend/windows/mod.rs#L87-L94)。

#### 问题

```rust
WMIConnection::with_namespace_path(NAMESPACE)
    .map_err(|_| UwfError::NotInstalled)?;
```

以及：

```rust
self.con()?.query().map_err(|_| UwfError::NotInstalled)?;
```

权限不足、WMI 服务异常、临时 RPC 错误都会被报成“UWF 未安装”，用户会被引导去执行 `Enable-WindowsOptionalFeature`，但真正的问题可能是权限或 WMI 服务。

#### 修复

- 只在错误明确表示“命名空间/类不存在”时映射为 `NotInstalled`；
- 其他错误保留为 `UwfError::Wmi`，并在 UI 中显示原始错误；
- 在 `NotInstalled` 页面增加“如果是权限问题，请确认管理员权限”的提示；
- 在日志中记录原始 WMI 错误（至少 `Debug`）。

由于 `wmi::WMIError` 的具体变体需要查 crate 版本，建议先写一个 helper：

```rust
fn map_wmi_connect_error(e: wmi::WMIError) -> UwfError {
    // 仅当错误码是 WBEM_E_INVALID_NAMESPACE / WBEM_E_NOT_FOUND 时
    // 返回 NotInstalled；其余返回 Wmi(e)
    UwfError::Wmi(Box::new(e))
}
```

实现前先打印/断言错误码，避免猜。

#### 回归测试

- 纯函数测试 `map_wmi_connect_error` 对已知错误码的分类；
- 在 Windows 真机上模拟权限不足（非管理员启动未提权版本），确认不会显示“未安装”引导。

---

### P1-16：选择按索引保存，刷新后可能指向另一项

#### 位置

[app.rs](src/app.rs#L259-L267)。

#### 问题

`clamp_selections` 只保证索引不越界，不保持选中项身份。排除项排序、增删后，索引可能落到另一条；用户连续按 `D` 可能删错项。

#### 修复

为排除项保存稳定 key：

```rust
#[derive(Default)]
pub struct App {
    selected_file: Option<(String, String)>, // (drive, rel_path)
    selected_registry: Option<String>,
    // ...
}

fn restore_selection(&mut self) {
    if let Some((drive, rel)) = &self.selected_file {
        let list = self.visible_file_exclusions();
        self.file_exclusion_state.select(
            list.iter().position(|e| &e.drive_letter == drive && &e.file_rel_path == rel)
        );
    }
    // registry 同理
}
```

在切换视图/刷新后调用。如果不想引入新字段，可以在每次 `Snapshot` 后把选择重置到 0，作为保守行为；但这会牺牲连续性。

#### 回归测试

- 选中第 2 项，注入一个使排序变化的 Snapshot，断言仍选中同一路径；
- 选中项被删除后，选择收敛到相邻项而不是随机索引。

---

### P1-17：多步 Overlay 配置可能部分成功但只报一个错

#### 位置

[backend/mod.rs](src/backend/mod.rs#L203-L219)。

#### 问题

`SetOverlayConfig` 依次调用 `set_overlay_type` / `set_overlay_max_size` / `set_warning_threshold` / `set_critical_threshold`。中间失败时：

- 前面的修改已经生效；
- 只回一个 `OpErr`；
- 没有刷新，UI 显示旧状态。

#### 修复

1. 每一步记录步骤名；
2. 失败时仍然 `refresh_all`，让 UI 展示实际状态；
3. 返回“部分成功”错误，包含失败的步骤：

```rust
let mut applied = Vec::new();
if !locked {
    b.set_overlay_type(overlay_type)?;
    applied.push("类型");
    b.set_overlay_max_size(max_mb)?;
    applied.push("上限");
}
b.set_warning_threshold(warning_mb)?;
applied.push("预警阈值");
b.set_critical_threshold(critical_mb)?;
applied.push("临界阈值");
Ok(...)
```

失败时用 `UwfError::Partial { applied, failed, source }`，或至少 `OpErr(format!("{failed} 设置失败；已应用: {applied:?}"))`。

#### 回归测试

- fake backend 让第 2 步失败，断言错误包含已应用步骤；
- 失败后 UI 收到 Snapshot，显示实际状态。

---

### P1-18：Mock 读方法有副作用、删除不校验卷

#### 位置

[mock.rs](src/backend/mock.rs#L143-L152)、[mock.rs](src/backend/mock.rs#L232-L236)。

#### 问题

- `get_overlay_runtime` 每次调用让 consumption +7；查询方法有副作用，测试和真实后端不一致；
- `remove_file_exclusion` 不校验卷存在性，未知卷删除静默成功，和 `add_file_exclusion` 不一致。

#### 修复

- 用时间/显式 tick 驱动演示数据：

```rust
struct MockUwfBackend {
    last_runtime_sample: Instant,
    // ...
}

fn get_overlay_runtime(&mut self) -> Result<OverlayRuntime> {
    let now = Instant::now();
    let elapsed = now.duration_since(self.last_runtime_sample);
    self.last_runtime_sample = now;
    // 按 elapsed 计算增长，或保持不变
}
```

- `remove_file_exclusion` 调用 `self.known_volume(drive_letter)?`；
- `remove_registry_exclusion` 可选校验存在性。

#### 回归测试

- 连续两次 `get_overlay_runtime` 不应无条件 +7（如果这是测试依赖，改用显式 `advance`）；
- 未知卷 remove 返回 `VolumeNotFound`。

---

### P1-19：导入/导出编码与覆盖策略

#### 位置

[backend/mod.rs](src/backend/mod.rs#L334-L390)。

#### 问题

- `read_to_string` 不支持 UTF-16；UTF-8 BOM 会让首行路径/`HK` 前缀判断失败；
- 导出空列表也写 `"\n"`；
- 同一秒内重复导出会静默覆盖；
- 没有覆盖确认。

#### 修复

- 读取后剥离 BOM：

```rust
let content = std::fs::read_to_string(path)?;
let content = content.strip_prefix('\u{feff}').unwrap_or(&content);
```

- 明确只支持 UTF-8（在 README/错误中说明）；
- 导出：

```rust
let body = if lines.is_empty() {
    String::new()
} else {
    lines.join("\n") + "\n"
};
std::fs::write(path, body)?;
```

- 文件名冲突：使用毫秒时间戳或循环 `_1`、`_2`；
- 覆盖前确认：如果 `path.exists()`，在 UI 里弹确认，或自动选择唯一文件名。

#### 回归测试

- BOM 文件首行能正确解析；
- 空列表导出文件长度为 0；
- 同一秒导出两次不会覆盖（或明确提示）。

---

## 6. P2：Dead Code、Smell、Over-design

### 6.1 Dead code / 冗余

| 项 | 证据 | 处理 |
| :--- | :--- | :--- |
| 日志历史 | [MAX_LOG/StatusEntry](src/app.rs#L22-L25)、[log](src/app.rs#L248-L257)、[render_log](src/ui/mod.rs#L54-L65) 只取 `.back()` | 改成 `Option<StatusEntry>`，或增加日志弹窗真正消费历史 |
| `OverlayConfig.current_session` | [overlay.rs](src/model/overlay.rs#L36-L42)；在 [mock.rs](src/backend/mock.rs#L127-L140)、[windows/mod.rs](src/backend/windows/mod.rs#L200-L207) 赋值，从未读取 | 删除字段；会话由 current/next 字段和调用参数表达 |
| WMI `CurrentSession` | [classes.rs](src/backend/windows/classes.rs#L30-L60) 三处 `allow(dead_code)` | 删除；serde 会忽略 `SELECT *` 的多余字段 |
| `UwfError` 整体 allow | [error.rs](src/error.rs#L7-L9) | 改成定向 `cfg_attr`；Linux 下只有 `WmiMethod` 等少数变体未用 |
| `Win32_UI_Shell` | [Cargo.toml](Cargo.toml#L16-L21) | 删除；当前只用 Foundation/Security/Threading |
| `InputOutcome::Browse(_)` | [app.rs](src/app.rs#L472-L478) | 当前不可达；改成无参数 `Browse`，或显式处理所有 action |
| `RemoveAllExclusions` 文档 | [PLAN.md](PLAN.md#L329-L333) | 补实现或删文档 |
| `FullSnapshot` 的 Box/Option | [backend/mod.rs](src/backend/mod.rs#L73-L85)、[collect_snapshot](src/backend/mod.rs#L289-L300) | Option 永远 Some，Box 收益很小；统一加载语义，Box 可选保留 |

### 6.2 Code smell / bad design

1. **D7 文本层没有真正落地。**  
   [text.rs](src/ui/text.rs#L1-L8) 只覆盖一部分；弹窗标题/提示在 [app.rs](src/app.rs#L322-L343)，成功文案在 [backend/mod.rs](src/backend/mod.rs#L195-L198)，Overlay 文案在 [overlay_settings.rs](src/ui/components/overlay_settings.rs#L156-L192)，文件选择器文案在 [file_picker.rs](src/ui/components/file_picker.rs#L200-L244)，`OverlayType::label` 在 [model/overlay.rs](src/model/overlay.rs#L28-L33)。  
   建议二选一：
   - 认真集中到 `ui/text.rs`（包括 modal 标题、成功/失败文案、按钮提示）；
   - 接受中文硬编码，删除“全部文案集中”的承诺，避免误导。

2. **错误类型里嵌 UI 引导文案。**  
   [error.rs](src/error.rs#L12) 的 PowerShell 命令和 [text.rs](src/ui/text.rs#L34-L35) 重复。建议 `UwfError::NotInstalled` 只表达“未安装”，引导命令由 UI 层根据错误种类渲染。

3. **`App` 的公共字段过多。**  
   [app.rs](src/app.rs#L94-L116) 几乎所有字段都是 `pub`，视图直接改 `TableState`/`ListState`。对小 TUI 可以接受，但 `pending`、`not_installed`、`snapshot` 这些有不变量的状态应私有化，并提供方法。

4. **`FullSnapshot` 的加载语义不一致。**  
   [collect_snapshot](src/backend/mod.rs#L289-L300) 永远返回 `Some`；`Option` 只表达“首次加载中”。`volumes` 是 `Vec`，空数组既可能是“无卷”也可能是“未加载”：[dashboard.rs](src/ui/views/dashboard.rs#L111-L113) 把空 volumes 显示成“加载中”，排除项空数组却显示“（无）”。  
   建议 `App.snapshot: Option<FullSnapshot>`，或统一 `loaded: bool`。

5. **`Tab::index()` 与 `ALL` 顺序强耦合。**  
   [app.rs](src/app.rs#L35-L61) 目前顺序一致，但重排变体后 `index()` 会错位。建议 `ALL.iter().position(|t| *t == self)` 或显式 repr + 测试。

6. **`InputState.browse` 与 `InputAction` 冗余。**  
   [input_dialog.rs](src/ui/components/input_dialog.rs#L20-L27) 的 `browse` 可由 `action == AddFileExclusion` 推导，两个状态可能不一致。

7. **`mutate` / `refresh_all` 的副作用设计。**  
   `refresh_all` 返回 `()`，失败通过 channel 发 `OpErr`，调用方无法判断刷新结果。见 P1-1。

### 6.3 Over-design

1. **`scripts/tui_smoke.py` 是最大的 over-design。**  
   8KB 手写终端模拟器 + 硬编码 `sleep` 调度 + `SIGKILL` + `glob` 删除所有 `EXCLUSIONLIST_*.txt`（[scripts/tui_smoke.py](scripts/tui_smoke.py#L20-L40)、[scenario_1](scripts/tui_smoke.py#L104-L136)）。  
   问题：
   - 依赖固定 35×110 和机器速度，CI 容易 flaky；
   - 需要事先构建 `target/debug/uwf_manager`，README 没写；
   - 会删除仓库里所有同名导出文件，可能误删用户文件。  
   建议：保留少量稳定场景，或使用成熟的 expect/vt100 方案；测试运行在临时目录，避免 glob 误删。

2. **日志历史**：`MAX_LOG = 50` + `VecDeque` + 时间戳，最后只显示一条。典型“先做通用，后来只用最后一条”。

3. **`BackendEvent::Snapshot(Box<FullSnapshot>)`**：Box 对几个 Vec 的移动收益很小，属于可简化的防御性优化。

4. **D7 文本层**：只完成 20% 的 i18n 抽象，维护者会误以为文案已集中。

5. **`build.rs` 的编译器回退链**：逻辑复杂但有明确目的；如果只支持一种交叉工具链，可简化。当前不算 bug，属于可接受的构建复杂度。

---

## 7. 修复路线图

### 阶段 0：基线（0.5 天）

- `cargo fmt`，把 `cargo fmt --check` 加入 README/CI；
- `Cargo.toml` 增加 `rust-version = "1.99"`；
- 删除未使用的 `Win32_UI_Shell`；
- 确认 `cargo test` / `cargo clippy --all-targets -- -D warnings` 仍通过。

### 阶段 1：P0 状态机与危险动作（1–2 天）

- 引入 `RequestId` / `BackendCommand` / `Outcome`（P0-1、P0-2）；
- `pending` 改成 `Option<RequestId>`，电源动作绑定 id；
- `not_installed` 键盘 guard + 关闭 modal + 专用快捷键栏（P0-3）；
- `q/Esc` pending 确认，后端 Quit ack/join 或超时（P0-6）；
- 回归测试覆盖 P0-1/2/3/6。

### 阶段 2：WMI 安全语义（1 天）

- `commit_file_deletion` 先校验后删，增加部分成功错误（P0-4）；
- Restart/Shutdown 校验 `ReturnValue`，增加 no-refresh 变更出口（P0-5）；
- `refresh_all` 返回 `Result` 或 `Outcome`，避免成功被刷新失败覆盖（P1-1）；
- Windows 真机手动验证：非 UWF 卷删除、重启失败、关机/重启终端恢复。

### 阶段 3：语义/UX（1–2 天）

- Gauge 用 current 上限（P1-3）；
- Overlay 锁定字段/业务校验（P1-4）；
- 当前/下次视图增删语义（P1-2）；
- 导入全失败/部分失败事件（P1-5）；
- `GetExclusions` 部分失败上报（P1-6）；
- `list_volumes` 并集（P1-7）；
- `--mock` 文档/构建策略（P1-8）；
- `overlay_runtime` 单一来源（P1-9）。

### 阶段 4：健壮性与 UI（1 天）

- 后端断线检测、send 错误处理（P1-10）；
- event loop 唤醒/统一事件 channel（P1-11）；
- 文件选择器滚动/盘符显示/探测（P1-12）；
- 输入框 modifier/光标（P1-13）；
- 路径归一化（P1-14）；
- NotInstalled 错误映射（P1-15）；
- 选择身份保持（P1-16）；
- Overlay 多步部分成功（P1-17）。

### 阶段 5：清理与文档（0.5–1 天）

- 删除 dead code（第 6.1 节）；
- 决定文本层策略（集中或删除承诺）；
- 修 P1-18：Mock 读方法副作用、未知卷删除；
- 修 P1-19：BOM/UTF-16 编码、空列表导出、同秒覆盖；
- 修 `import_export_roundtrip_with_mock`、硬编码个人路径、demo fixture 重复；
- 更新 README/PLAN 漂移；
- 更新 smoke 测试的前置条件和临时目录。

---

## 8. 回归测试清单

| ID | 场景 | 测试类型 | 期望 |
| :--- | :--- | :--- | :--- |
| T1 | pending 时确认重启 | App 单测 | 不发送命令、不 arm、不退出 |
| T2 | 查询 OpErr 不影响 pending | App 单测 | pending 保持 |
| T3 | 查询 Snapshot 不影响 pending | App 单测 | pending 保持 |
| T4 | NotInstalled 下按 F10/E/S | App 单测 | 无 modal、无命令 |
| T5 | 删除提交先校验卷 | Windows 纯逻辑/单测 | 卷不存在时不删文件 |
| T6 | Restart 返回非零 | fake backend/Windows | UI 不退出、显示错误 |
| T7 | 电源动作成功后不刷新 | backend 单测 | 不发送 Snapshot |
| T8 | pending 时 q/Esc | App 单测 | 不直接退出/二次确认 |
| T9 | Gauge current/next 不同 | UI 渲染测试 | 使用 current，百分比正确 |
| T10 | Overlay 锁定 + 上限为空 | 组件单测 | 只改阈值可保存 |
| T11 | warning >= critical | 组件单测 | 拒绝并显示错误 |
| T12 | 导入全失败 | backend 单测 | 非 OpOk |
| T13 | 单卷 GetExclusions 失败 | fake backend | 有 warning/整体错误 |
| T14 | list_volumes next-only | 纯逻辑单测 | 包含该卷 |
| T15 | `C:foo` / `C:/foo` | backend 单测 | 拒绝/规范化正确 |
| T16 | BOM 导入 | backend 单测 | 首行正确解析 |
| T17 | 空列表导出 | backend 单测 | 文件为空或合理 |
| T18 | 后端断开 | App 单测 | 进入错误态，不永久 pending |
| T19 | 选择刷新后保持身份 | App 单测 | 仍选中同一项 |
| T20 | 文件选择器滚动 | UI 状态测试 | 选中项可见 |
| T21 | Ctrl+C 输入框 | 组件单测 | 不插入字符 |
| T22 | Mock 查询无副作用 / 未知卷删除 | Mock 单测 | 连续查询不改变用量；未知卷返回 `VolumeNotFound` |
| T23 | 导入 BOM / 空列表导出 / 同秒重复导出 | backend 单测 | BOM 首行正确；空导出为 0 字节；不覆盖已有文件 |

建议先为 P0 的 T1–T8 建测试，再补 P1。现有 `app::tests` 中的 `mutation_gated_while_pending`、`restart_flow_mock_stays_windows_quits` 需要相应调整，避免它们固化旧的错误行为；`backend::tests::r6_split_absolute_path` 当前断言 `C:foo -> \foo`，修 P1-14 时必须同步改为拒绝。

---

## 9. 验收标准

### 9.1 自动化

- [ ] `cargo fmt --check` 通过；
- [ ] `cargo clippy --all-targets -- -D warnings` 通过；
- [ ] `cargo test` 全部通过，新增回归测试覆盖 T1–T21 中至少 P0/P1 关键项；
- [ ] `cargo check --target x86_64-pc-windows-gnu` 在具备 `llvm-rc`/`windres` 的环境通过；
- [ ] `python3 scripts/tui_smoke.py` 在 `cargo build` 后通过，且不污染仓库。

### 9.2 手动（Windows + UWF 真机）

- [ ] `--mock` 的行为与文档一致（要么免管理员，要么明确仍需管理员）；
- [ ] 在非 UWF 卷上执行删除提交，不会出现“文件删了但固化失败”而无提示；
- [ ] `RestartSystem` / `ShutdownSystem` 返回非零时 UI 不退出并显示错误；
- [ ] 重启/关机成功后终端正常恢复；
- [ ] 修改 Overlay 上限/阈值后，Dashboard 的 Gauge 百分比基准正确；
- [ ] 当前/下次排除项视图的增删符合 R4，不出现“当前视图删 next”；
- [ ] 单卷读取失败时 UI 有提示，不静默显示为空；
- [ ] 权限不足 / WMI 服务异常不会被误报为“UWF 未安装”。

### 9.3 代码质量

- [ ] `pending`、电源动作、选择状态只有一个权威来源；
- [ ] 没有 `allow(dead_code)` 掩盖新 dead code；
- [ ] 文本层策略与 README/PLAN 一致；
- [ ] 新增错误变体有中文 Display，且能区分“动作失败”和“动作成功但刷新失败”。

---

## 10. 文档与构建一致性

| 文档 | 当前问题 | 建议 |
| :--- | :--- | :--- |
| [README.md](README.md#L29-L34) | `--mock` 免管理员与 manifest 冲突 | 改为“仍需管理员”，或提供独立 mock 构建 |
| [README.md](README.md#L21-L27) | “Windows 本机构建无需额外组件”偏乐观 | 说明 embed-resource 需要 `rc.exe`/`windres`，通常由 VS Build Tools/Windows SDK 提供 |
| [README.md](README.md#L121-L125) | smoke 测试未写 `cargo build` 前置 | 增加 `cargo build`，并说明 pty 仅 Unix |
| [README.md](README.md#L128-L130) | “unsafe 仅提权检测一处” | build.rs 也有 `set_var` unsafe；可注明“运行时代码仅一处” |
| [PLAN.md](PLAN.md#L165-L173) | `UwfBackend: Send` 与代码 `!Send` 不一致 | 改为 `!Send`，解释 COM 套间约束 |
| [PLAN.md](PLAN.md#L137) | `RegistryExclusion` 模型不存在 | 改成 `Vec<String>` 或补模型 |
| [PLAN.md](PLAN.md#L329-L333) | `RemoveAllExclusions` 未实现 | 补实现或删除条目 |
| [PLAN.md](PLAN.md#L370) | 说“变更在途时 Esc 仅取消弹窗”，代码不是 | 修代码或改文档 |
| [Cargo.toml](Cargo.toml#L1-L4) | 缺少 `rust-version` | 增加 `rust-version = "1.99"` |
| [Cargo.toml](Cargo.toml#L16-L21) | `Win32_UI_Shell` 未用 | 删除 |

---

## 11. 附：Review 复现命令

```bash
cd /home/gewei/uwf_manager

# 基线
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check

# Mock 端到端
cargo build
python3 scripts/tui_smoke.py

# Windows 交叉检查（需要 llvm-rc 或 windres）
cargo check --target x86_64-pc-windows-gnu
cargo clippy --target x86_64-pc-windows-gnu --all-targets -- -D warnings
```

本次 P0 复现是在临时副本中完成的（未修改仓库源码）：

- 构造 `pending = true` 后调用 `execute_confirm(Restart)`：命令未发出但 `power_action_pending` 仍为 `Some`，随后无关 `OpOk` 触发退出；
- 注入查询类 `OpErr`：`pending` 被错误清除；
- `not_installed = true` 后按 `F10` + Enter：`RestartSystem` 仍被发送；
- `filter current enabled + overlay_config_next = None` 时按 Enter：锁定字段仍因上限解析失败而无法保存；
- `current.max_size_mb = 2048`、`next.max_size_mb = 4096`、`consumption = 512`：UI 渲染为 `/4096`，确认使用了 next；
- 仅含 `not_a_path` 的导入文件：仍收到 `OpOk`；
- `C:foo` -> `\foo`，`C:/foo` -> `\/foo`；
- Mock 对未知卷 `Z:` 删除文件排除项仍返回 `Ok`。

---

## 12. 最终建议

如果只安排一个迭代，建议顺序是：

1. **P0-1 + P0-2 + P0-6**：统一 request id / mutation 状态 / 电源动作绑定 / 退出协议；
2. **P0-3**：未安装页键盘隔离；
3. **P0-4 + P0-5**：删除提交顺序、重启/关机 ReturnValue 与 no-refresh；
4. **P1-1 / P1-3 / P1-4 / P1-5**：刷新结果、Gauge、Overlay 校验、导入结果；
5. **P1-2 / P1-6 / P1-7 / P1-9**：当前/下次语义、部分读取、卷并集、单一 runtime 状态；
6. 最后做 dead code、文本层、fmt、文档和 smoke 清理。

P0-1 和 P0-2 的共同根因是“事件没有归属”，建议一次性引入 `RequestId`，不要用更多 bool 打补丁。把状态机收敛之后，后面的 dead code 和样式清理都是低风险工作。
