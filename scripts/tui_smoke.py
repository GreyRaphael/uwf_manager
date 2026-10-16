#!/usr/bin/env python3
"""UWF Manager TUI pty 冒烟测试：虚拟屏幕重建 + 场景断言。

原理：ratatui 只重绘变化单元格，因此不能对原始输出做子串匹配。
本脚本实现迷你终端模拟器（光标定位 + 宽字符写入），把全部帧重放
到虚拟屏幕，并对每帧结束时的屏幕快照做断言。
"""
import os, pty, time, fcntl, termios, struct, select, re, signal, sys, glob, unicodedata

ROWS, COLS = 35, 110
REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = "./target/debug/uwf_manager"


def char_width(ch):
    """与 unicode-width crate 对齐：East Asian W/F 记 2，其余（含 Ambiguous）记 1。"""
    return 2 if unicodedata.east_asian_width(ch) in ("W", "F") else 1


def run_tui(schedule, duration=8.0, cwd=REPO):
    """在 pty 中运行 TUI，返回 (exit_code, 每帧屏幕快照列表)。"""
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
        self.grid = {}  # (y, x) -> (ch, width)，宽字符只占左半一格
        self.cx = self.cy = 0

    def put(self, ch):
        w = char_width(ch)
        # 写入位置是左邻宽字符的右半 → 左邻被连坐清除
        if self.grid.get((self.cy, self.cx - 1), (" ", 1))[1] == 2:
            del self.grid[(self.cy, self.cx - 1)]
        # 写入位置本身是宽字符左半 → 其右半（可能压着的字符）清除
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
    """宽字符在网格中占 2 单元（视觉连续但带空格），比对前去除全部空白。"""
    squashed = needle.replace(" ", "")
    return any(squashed in f.replace(" ", "") for f in frames)


def scenario_1():
    """排除项增删/导入导出/帮助/Overlay 锁定/模拟重启。"""
    sched = [
        (0.8, b"s"), (1.3, b"\x1b"),                    # Overlay 弹窗查看 R1 锁定
        (1.6, b"3"),                                     # Tab3 排除项
        (1.9, b"a"), (2.3, b"C:\\MyTools"), (2.9, b"\r"),  # 输入添加文件排除
        (3.5, b"x"),                                     # 导出（2 文件 + 1 注册表 = 3 项）
        (4.1, b"d"),                                     # 删除选中（C:\MyTools 排序第 0）
        (4.7, b"?"), (5.2, b"\x1b"),                     # 帮助
        (5.4, b"1"),                                     # 回到仪表盘（F10 仅在此页有效）
        (5.7, b"\x1b[21~"), (6.2, b"\r"),                # F10 + Enter 模拟重启
        (7.2, b"q"),
    ]
    code, frames = run_tui(sched)
    exported = glob.glob(os.path.join(REPO, "EXCLUSIONLIST_*.txt"))
    export_ok = False
    if exported:
        content = open(exported[0]).read()
        export_ok = all(s in content for s in
                        ["C:\\MyTools", "C:\\Windows\\Temp", "HKLM\\SOFTWARE\\UwfManagerDemo"])
        for f in exported: os.remove(f)
    return {
        "R1 锁定提示(已锁定/仅阈值)": any_frame(frames, "已锁定") and any_frame(frames, "仅阈值可改"),
        "弹窗预填(2048/1536/1843)": any_frame(frames, "2048") and any_frame(frames, "1843"),
        "添加文件排除反馈": any_frame(frames, "已添加文件排除: C:\\MyTools"),
        "新增项出现在列表": any_frame(frames, "C:\\MyTools"),
        "导出 3 项反馈": any_frame(frames, "已导出 3 项至"),
        "导出文件内容(3项)": export_ok,
        "删除文件排除反馈": any_frame(frames, "已删除文件排除: C:\\MyTools"),
        "帮助弹窗": any_frame(frames, "快捷键帮助"),
        "模拟重启反馈": any_frame(frames, "（模拟）已重启"),
        "干净退出": code == 0,
    }


def scenario_2():
    """R1 解锁周期 + 卷保护重启生效 + 文件选择器提交 + 删除提交确认。"""
    sched = [
        (0.8, b"d"),                    # 禁用筛选器（下次生效）
        (1.4, b"\x1b[21~"), (1.9, b"\r"),  # F10 模拟重启 → current=disabled
        (2.6, b"s"),                    # Overlay 配置：应已解锁
        (3.0, b"\x1b[B"),               # Down 到 上限 行
        (3.2, b"\x7f\x7f\x7f\x7f"),     # 清空 2048
        (3.5, b"4096"),                 # 输入 4096
        (3.8, b"\r"),                   # 保存
        (4.4, b"2"),                    # Tab2 卷保护
        (4.6, b"\x1b[B"),               # Down 选中 D:
        (4.8, b"p"),                    # 保护 D:（下次生效）
        (5.4, b"1"),                    # 回到仪表盘
        (5.7, b"\x1b[21~"), (6.2, b"\r"),  # 重启 → D: current 受保护
        (6.9, b"2"),                    # 回到卷保护页验证
        (7.5, b"4"),                    # Tab4 提交工具
        (7.8, b"d"),                    # 删除提交 → 文件选择器
        (8.4, b"\x1b"), (8.8, b"q"),
    ]
    code, frames = run_tui(sched, duration=10.0)
    return {
        "禁用筛选器反馈": any_frame(frames, "UWF 将在下次系统重启后禁用"),
        "Overlay 配置已保存反馈": any_frame(frames, "Overlay 配置已保存"),
        "上限 4096 生效": any_frame(frames, "4096"),
        "保护 D: 反馈": any_frame(frames, "D: 保护成功"),
        "D: 重启后当前受保护": any_frame(frames, "D: 受保护 受保护"),
        "模拟重启反馈": any_frame(frames, "（模拟）已重启"),
        "文件选择器弹窗": any_frame(frames, "文件选择"),
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
    main()
