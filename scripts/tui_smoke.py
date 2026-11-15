#!/usr/bin/env python3
"""UWF Manager TUI pty 冒烟测试：虚拟屏幕重建 + 场景断言。

原理：ratatui 只重绘变化单元格，因此不能对原始输出做子串匹配。
本脚本实现迷你终端模拟器（光标定位 + 宽字符写入），把全部帧重放
到虚拟屏幕，并对每帧结束时的屏幕快照做断言。
"""
import os, pty, time, fcntl, termios, struct, select, re, signal, sys, glob, unicodedata
import shutil, tempfile

ROWS, COLS = 35, 110
REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(REPO, "target/debug/uwf_manager")
# 在临时目录运行，避免导出文件的 glob 误删仓库/用户文件。
RUN_DIR = tempfile.mkdtemp(prefix="uwf_smoke_")


def char_width(ch):
    """与 unicode-width crate 对齐：East Asian W/F 记 2，其余（含 Ambiguous）记 1。"""
    return 2 if unicodedata.east_asian_width(ch) in ("W", "F") else 1


def run_tui(schedule, duration=8.0, cwd=None):
    """在 pty 中运行 TUI，返回 (exit_code, 每帧屏幕快照列表)。"""
    cwd = cwd or RUN_DIR
    master, slave = pty.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))
    pid = os.fork()
    if pid == 0:
        os.setsid(); os.chdir(cwd)
        os.dup2(slave, 0); os.dup2(slave, 1); os.dup2(slave, 2)
        os.close(master); os.close(slave)
        os.execv(BIN, [BIN])
    os.close(slave)
    out, start, sent = b"", time.time(), set()
    while time.time() - start < duration:
        r, _, _ = select.select([master], [], [], 0.05)
        if r:
            try: out += os.read(master, 65536)
            except OSError: break
        for i, (t, k) in enumerate(schedule):
            if i not in sent and time.time() - start >= t:
                os.write(master, k); sent.add(i)
    try: os.kill(pid, signal.SIGKILL)
    except ProcessLookupError: pass
    _, status = os.waitpid(pid, 0)
    return os.waitstatus_to_exitcode(status), VirtualScreen().feed(out)


class VirtualScreen:
    """忠实模拟终端宽字符语义：写宽字符占 2 列；写入压到宽字符任一半时，
    终端会连带清除该宽字符的另一半。"""

    def __init__(self):
        self.grid = {}
        self.cx = self.cy = 0

    def put(self, ch):
        w = char_width(ch)
        if self.grid.get((self.cy, self.cx - 1), (" ", 1))[1] == 2:
            del self.grid[(self.cy, self.cx - 1)]
        if self.grid.get((self.cy, self.cx), (" ", 1))[1] == 2:
            self.grid.pop((self.cy, self.cx + 1), None)
        self.grid[(self.cy, self.cx)] = (ch, w)
        if w == 2:
            self.grid.pop((self.cy, self.cx + 1), None)
        self.cx += w

    def feed(self, data: bytes):
        text = data.decode("utf-8", "replace")
        frames = []
        i = 0
        while i < len(text):
            ch = text[i]
            if ch == "\x1b" and i + 1 < len(text) and text[i + 1] == "[":
                m = re.match(r"\x1b\[([0-9;?]*)([a-zA-Z])", text[i:])
                if m:
                    params, cmd = m.group(1), m.group(2)
                    i += len(m.group(0))
                    if cmd == "H":
                        parts = params.split(";")
                        self.cy = int(parts[0] or 1) - 1 if parts[0] else 0
                        self.cx = int(parts[1] or 1) - 1 if len(parts) > 1 else 0
                    elif cmd == "l" and "?25" in params:
                        frames.append(self.snapshot())
                    continue
            if ch >= " " and ch != "\x7f":
                self.put(ch)
            i += 1
        return frames

    def snapshot(self):
        lines = []
        for y in range(ROWS):
            line = "".join(self.grid.get((y, x), (" ", 1))[0] for x in range(COLS))
            lines.append(line)
        return "\n".join(lines)


def any_frame(frames, needle):
    squashed = needle.replace(" ", "")
    return any(squashed in f.replace(" ", "") for f in frames)


def scenario_1():
    """单屏四区展示 / R1 锁定提示 / 排除项增删与导入导出 / 帮助 / 模拟重启。"""
    sched = [
        (0.7, b"\t\t\t"),                               # Tab×3 到 4.排除列表
        (1.1, b"a"), (1.5, b"C:\\MyTools"), (2.0, b"\r"),  # 添加文件排除
        (2.6, b"x"),                                     # 导出（2 文件 + 1 注册表 = 3 项）
        (3.1, b"\x1b[H"),                                # Home 选中首项 C:\MyTools
        (3.4, b"d"),                                     # 删除选中
        (4.0, b"?"), (4.5, b"\x1b"),                     # 帮助弹窗
        (4.9, b"\x1b[21~"), (5.4, b"\r"),                # F10 + Enter 模拟重启
        (6.3, b"q"),
    ]
    code, frames = run_tui(sched, duration=7.2)
    exported = glob.glob(os.path.join(RUN_DIR, "EXCLUSIONLIST_*.txt"))
    export_ok = False
    if exported:
        content = open(exported[0]).read()
        export_ok = all(s in content for s in
                        ["C:\\MyTools", "C:\\Windows\\Temp", "HKLM\\SOFTWARE\\UwfManagerDemo"])
        for f in exported: os.remove(f)
    return {
        "单屏四区同屏展示": (
            any_frame(frames, "1.基本设置")
            and any_frame(frames, "2.缓存设置(MB)")
            and any_frame(frames, "3.保护分区")
            and any_frame(frames, "4.排除列表")
        ),
        "R1 锁定提示": any_frame(frames, "启用时最大缓存已锁定") and any_frame(frames, "[锁]"),
        "表单预填(2048/1536/1843)": any_frame(frames, "2048") and any_frame(frames, "1843"),
        "添加文件排除反馈": any_frame(frames, "已添加文件排除: C:\\MyTools"),
        "新增项出现在列表": any_frame(frames, "C:\\MyTools"),
        "导出 3 项反馈": any_frame(frames, "已导出 3 项至"),
        "导出文件内容(3项)": export_ok,
        "删除文件排除反馈": any_frame(frames, "已删除文件排除: C:\\MyTools"),
        "帮助弹窗": any_frame(frames, "操作说明"),
        "模拟重启反馈": any_frame(frames, "（模拟）已重启"),
        "干净退出": code == 0,
    }


def scenario_2():
    """R1 解锁周期 + 覆盖类型与缓存保存 + 卷保护重启生效 + 提交工具菜单。"""
    sched = [
        (0.7, b"\r"),                    # 基本设置 -> 写入过滤: 禁用（下次生效）
        (1.3, b"\x1b[21~"), (1.8, b"\r"),  # F10 模拟重启 → current=disabled
        (2.5, b"\x1b[B"), (2.8, b"\r"),  # Down 到覆盖类型 -> 切换为 磁盘(Disk)
        (3.3, b"\t"),                    # Tab 到 2.缓存设置（默认聚焦警告阈值）
        (3.6, b"\x1b[A"),                # Up 到 最大缓存
        (3.9, b"\x7f\x7f\x7f\x7f"),      # 清空 2048
        (4.2, b"4096"),                  # 输入 4096
        (4.6, b"\r"),                    # Enter 确认缓存
        (5.2, b"\t"),                    # Tab 到 3.保护分区
        (5.5, b"\x1b[B"),                # Down 选中 D:
        (5.8, b"p"),                     # 保护 D:（下次生效）
        (6.4, b"\x1b[21~"), (6.9, b"\r"),  # F10 模拟重启 → D: 当前/重启均为 已保护
        (7.7, b"c"),                     # 打开 提交固化工具 菜单
        (8.0, b"2"),                     # 选择 [2] 删除并固化文件 -> 文件选择器
        (8.4, b"\x1b"), (8.8, b"q"),
    ]
    code, frames = run_tui(sched, duration=9.8)
    return {
        "禁用筛选器反馈": any_frame(frames, "UWF 将在下次系统重启后禁用"),
        "覆盖类型切换反馈": any_frame(frames, "覆盖类型已设为 磁盘(Disk)"),
        "缓存配置已保存反馈": any_frame(frames, "缓存配置已保存"),
        "上限 4096 生效": any_frame(frames, "4096"),
        "保护 D: 反馈": any_frame(frames, "D: 保护成功"),
        "D: 重启后当前已保护": any_frame(frames, "D: 已保护 已保护"),
        "模拟重启反馈": any_frame(frames, "（模拟）已重启"),
        "提交菜单与文件选择器": any_frame(frames, "提交固化工具") and any_frame(frames, "文件选择"),
        "干净退出": code == 0,
    }


def main():
    failed = 0
    for name, fn in [("场景1", scenario_1), ("场景2", scenario_2)]:
        print(f"===== {name} =====")
        for check, ok in fn().items():
            print(("PASS" if ok else "FAIL"), check)
            failed += 0 if ok else 1
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    try:
        main()
    finally:
        shutil.rmtree(RUN_DIR, ignore_errors=True)
