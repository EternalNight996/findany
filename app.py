# -*- coding: utf-8 -*-
"""内容扫描器 findany — PySide6 桌面 GUI 入口。

用法：python app.py
"""
from __future__ import annotations

import os
import sys
import time
import traceback

if getattr(sys, "frozen", False):
    # PyInstaller 打包后：__file__ 指向临时解压目录(_MEIPASS)，输出必须落在 exe 旁边
    APP_DIR = os.path.dirname(os.path.abspath(sys.executable))
else:
    APP_DIR = os.path.dirname(os.path.abspath(__file__))

# 日志目录：统一落到 log 下，以项目名命名
LOGS_DIR = os.path.join(APP_DIR, "logs")
try:
    os.makedirs(LOGS_DIR, exist_ok=True)
except Exception:
    LOGS_DIR = APP_DIR


def _log_path(name: str) -> str:
    return os.path.join(LOGS_DIR, name)


def _write_file(name: str, text: str, mode: str = "a") -> None:
    """写日志文件；运行日志超过 5MB 自动轮换为 name.1。"""
    p = _log_path(name)
    try:
        if mode == "a" and os.path.exists(p) and os.path.getsize(p) > 5 * 1024 * 1024:
            try:
                os.replace(p, _log_path(name + ".1"))
            except Exception:
                pass
        with open(p, mode, encoding="utf-8") as f:
            f.write(text)
    except Exception:
        pass

try:
    from PySide6.QtCore import Qt, QThread, Signal, QTimer
    from PySide6.QtGui import QColor, QBrush, QFont
    from PySide6.QtWidgets import (
        QApplication, QMainWindow, QWidget, QVBoxLayout, QHBoxLayout, QGridLayout,
        QLabel, QLineEdit, QPushButton, QComboBox, QSlider, QSpinBox, QCheckBox,
        QFileDialog, QTableWidget, QTableWidgetItem, QTextEdit, QDialog, QGroupBox,
        QMessageBox, QProgressBar, QAbstractItemView, QHeaderView, QSizePolicy,
    )
    from sonar.config import SearchConfig, load_config, save_config, default_out_dir, APP_NAME
    from sonar.scanner import ScanEngine
    from sonar.exporter import export_excel, copy_hits, make_batch_dir
    from sonar.logfilter.engine import FilterEngine, FilterRunCfg
    from sonar.logfilter.uploader import UploadProfile
    from sonar.logfilter.types import LogType
    from sonar.logfilter import autoconfig
except Exception:
    # 任何导入失败（pythonw 下无控制台）→ logs/findany-crash.log + 本机弹窗 + stderr
    _err = traceback.format_exc()
    try:
        _write_file("findany-crash.log", _err, "w")
    except Exception:
        pass
    try:
        import ctypes
        ctypes.windll.user32.MessageBoxW(0, _err, "findany 启动失败", 0x00000010)
    except Exception:
        pass
    sys.stderr.write(_err)
    raise SystemExit(1)

# ---------- 主题色（与 HTML 样板 token 对齐） ----------
COLORS = {
    "dark": {
        "bg": "#0f0f0f", "panel": "#151517", "panel2": "#1c1c1f",
        "border": "rgba(255,255,255,0.09)", "border2": "rgba(255,255,255,0.14)",
        "text": "#f5f5f5", "text2": "#979da6", "brand": "#5686fe",
        "brand_d": "#4176e6", "ok": "#22c55e", "err": "#ef4444", "warn": "#f59e0b",
    },
    "light": {
        "bg": "#ffffff", "panel": "#fafafa", "panel2": "#f2f2f3",
        "border": "rgba(0,0,0,0.08)", "border2": "rgba(0,0,0,0.14)",
        "text": "#0f0f0f", "text2": "#65676b", "brand": "#4176e6",
        "brand_d": "#2563eb", "ok": "#16a34a", "err": "#dc2626", "warn": "#d97706",
    },
}


def build_qss(dark: bool) -> str:
    c = COLORS["dark" if dark else "light"]
    return f"""
* {{ font-family: "Segoe UI", "Microsoft YaHei", sans-serif; font-size: 13px; }}
QMainWindow {{ background: {c['bg']}; }}
QWidget {{ background: transparent; color: {c['text']}; }}
/* 顶栏 */
#topbar {{ background: {c['panel']}; border-bottom: 1px solid {c['border']}; }}
#topbar QLabel {{ font-size: 14px; font-weight: 600; }}
#topbar QLabel#sub {{ font-size: 11px; color: {c['text2']}; font-weight: 400; }}
/* 面板 */
#panel {{ background: {c['panel']}; border-right: 1px solid {c['border']}; }}
#panel QLabel {{ color: {c['text2']}; font-size: 12px; }}
#panel QLabel#h2 {{ color: {c['text2']}; font-size: 11px; font-weight: 600; }}
QLineEdit, QComboBox, QSpinBox, QTextEdit {{
  background: {c['panel']}; border: 1px solid {c['border2']}; border-radius: 6px;
  padding: 5px 8px; color: {c['text']}; selection-background-color: {c['brand']};
}}
QLineEdit:focus, QComboBox:focus, QSpinBox:focus {{ border-color: {c['brand']}; }}
QLineEdit[mono="true"], QSpinBox {{ font-family: Consolas, monospace; }}
QComboBox QAbstractItemView {{ background: {c['panel2']}; color: {c['text']}; border: 1px solid {c['border2']}; }}
QPushButton {{
  background: {c['panel2']}; border: 1px solid {c['border2']}; border-radius: 6px;
  padding: 6px 14px; color: {c['text']};
}}
QPushButton:hover {{ background: {c['bg']}; }}
QPushButton:checked {{ background: {c['brand']}; color: #fff; border: 1px solid {c['brand_d']}; font-weight: 600; }}
QPushButton:disabled {{ color: {c['text2']}; border-color: {c['border']}; background: {c['panel']}; }}
QPushButton.primary {{ background: {c['brand']}; color: #fff; border: none; }}
QPushButton.primary:hover {{ background: {c['brand_d']}; }}
QPushButton.primary:disabled {{ background: {c['panel2']}; color: {c['text2']}; }}
QPushButton.stop {{ background: transparent; border: 1px solid {c['err']}; color: {c['err']}; }}
QPushButton.stop:hover {{ background: {c['err']}; color: #fff; }}
QPushButton.ghost {{ background: transparent; border: none; color: {c['text2']}; }}
QPushButton.ghost:hover {{ color: {c['text']}; background: {c['panel2']}; }}
QSlider::groove:horizontal {{ height: 6px; background: {c['panel2']}; border-radius: 3px; }}
QSlider::handle:horizontal {{ background: {c['brand']}; width: 14px; height: 14px; margin: -5px 0; border-radius: 7px; }}
QSlider::sub-page:horizontal {{ background: {c['brand']}; border-radius: 3px; }}
QCheckBox {{ color: {c['text2']}; }}
QCheckBox::indicator {{ width: 15px; height: 15px; border: 1px solid {c['border2']}; border-radius: 4px; background: {c['panel']}; }}
QCheckBox::indicator:checked {{ background: {c['brand']}; border-color: {c['brand']}; }}
/* 表格 */
QTableWidget {{ background: {c['bg']}; border: none; gridline-color: {c['border']}; }}
QTableWidget::item {{ padding: 4px 8px; }}
QHeaderView::section {{ background: {c['panel']}; color: {c['text2']}; padding: 8px; border: none; border-bottom: 1px solid {c['border']}; font-size: 12px; }}
QTableWidget::item:selected {{ background: {c['panel2']}; color: {c['text']}; }}
/* 进度与日志 */
#stat QLabel {{ color: {c['text2']}; font-size: 11px; }}
#stat QLabel.n {{ font-family: Consolas, monospace; font-size: 17px; font-weight: 600; color: {c['text']}; }}
#stat QLabel.n.good {{ color: {c['ok']}; }}
#stat QLabel.n.bad {{ color: {c['err']}; }}
QProgressBar {{ background: {c['panel2']}; border: none; border-radius: 4px; height: 8px; text-align: center; color: transparent; }}
QProgressBar::chunk {{ background: {c['brand']}; border-radius: 4px; }}
#logbar {{ background: {c['panel']}; border-top: 1px solid {c['border']}; color: {c['text2']}; font-size: 12px; }}
#logbar QPushButton {{ padding: 2px 8px; font-size: 11px; }}
/* 弹窗 */
QDialog {{ background: {c['panel']}; }}
QDialog QTextEdit {{ background: {c['bg']}; font-family: Consolas, monospace; font-size: 12px; }}
#modal-head {{ background: {c['panel2']}; border-bottom: 1px solid {c['border']}; }}
#modal-head QLabel {{ font-size: 14px; font-weight: 600; }}
QScrollBar:vertical {{ background: transparent; width: 9px; }}
QScrollBar::handle:vertical {{ background: {c['text2']}; border-radius: 4px; min-height: 24px; }}
QScrollBar::add-line, QScrollBar::sub-line {{ height: 0; }}
"""


class ScanWorker(QThread):
    """后台线程：跑 ScanEngine，进度/日志/结果通过 Qt 信号回主线程。"""
    progress = Signal(dict)
    log = Signal(str, str)
    done = Signal(list, object)     # items, ScanSummary
    error = Signal(str)

    def __init__(self, cfg: SearchConfig):
        super().__init__()
        self.cfg = cfg
        self._engine: ScanEngine | None = None

    def cancel(self):
        if self._engine:
            self._engine.cancel()

    def run(self):
        try:
            eng = ScanEngine(
                self.cfg,
                on_progress=lambda d: self.progress.emit(d),
                on_log=lambda lvl, msg: self.log.emit(lvl, msg),
            )
            self._engine = eng
            items, summary = eng.scan()
            self.done.emit(items, summary)
        except Exception:
            self.error.emit(traceback.format_exc())


class FilterWorker(QThread):
    """后台线程：跑 FilterEngine（判型提取 → 逐台回传），信号回主线程。"""
    progress = Signal(dict)
    upload = Signal(dict)
    log = Signal(str, str)
    done = Signal(list, object)     # items, FilterSummary
    error = Signal(str)

    def __init__(self, cfg: SearchConfig):
        super().__init__()
        self.cfg = cfg
        self._engine: FilterEngine | None = None

    def cancel(self):
        if self._engine:
            self._engine.cancel()

    def run(self):
        try:
            # 回传方案：SN 关联多文件（方案一）/ 单文件（方案二）
            file_list = None
            if self.cfg.filter_sn:
                file_list = autoconfig.find_sn_logs(self.cfg.root_dir, self.cfg.filter_sn,
                                                    self.cfg.recursive, self.cfg.extensions or ["log"])
                if not file_list:
                    self.error.emit(f"未找到与 SN「{self.cfg.filter_sn}」关联的日志（{self.cfg.root_dir}）")
                    return
                self.log.emit("info", f"SN「{self.cfg.filter_sn}」关联日志 {len(file_list)} 份："
                                      + "、".join(os.path.basename(p) for p in file_list))
            elif self.cfg.filter_file:
                file_list = [self.cfg.filter_file]
            rcfg = FilterRunCfg(
                root_dir=self.cfg.root_dir,
                out_dir=self.cfg.out_dir,
                log_type=self.cfg.filter_log_type,
                recursive=self.cfg.recursive,
                extensions=self.cfg.extensions or ["log"],   # 扩展名共享（GUI 可控，建议 log）
                encoding=self.cfg.encoding,
                file_list=file_list,
                max_file_mb=self.cfg.max_file_mb,
                threads=self.cfg.threads,
                keep_logs=self.cfg.copy_files,       # 选项「将命中文件复制到 out」（共享）
                upload_enabled=self.cfg.upload_enabled,
                upload_types=([self.cfg.filter_log_type] if self.cfg.filter_log_type != "auto"
                              else [t.strip() for t in self.cfg.upload_types.split(",") if t.strip()]),
                dry_run=self.cfg.upload_dry_run,
                profile=UploadProfile(
                    cli_path=self.cfg.upload_cli_path,
                    args=self.cfg.upload_args,
                    use_stdin=self.cfg.upload_stdin,
                    timeout_sec=float(self.cfg.upload_timeout),
                    max_retries=int(self.cfg.upload_retries),
                    secret_key=self.cfg.upload_secret_key,
                ),
                app_dir=APP_DIR,
            )
            eng = FilterEngine(
                rcfg,
                on_progress=lambda d: self.progress.emit(d),
                on_upload=lambda d: self.upload.emit(d),
                on_log=lambda lvl, msg: self.log.emit(lvl, msg),
            )
            self._engine = eng
            items, summary = eng.run()
            self.done.emit(items, summary)
        except Exception:
            self.error.emit(traceback.format_exc())


class CountdownDialog(QDialog):
    """完成后倒计时：归零自动退出；可取消 / 延时 30s / 打开输出目录。"""

    def __init__(self, parent, seconds: int, out_dir: str = ""):
        super().__init__(parent)
        self.setWindowTitle("自动退出")
        self.setModal(True)
        self.remaining = max(1, int(seconds))
        self.out_dir = out_dir
        lay = QVBoxLayout(self)
        lay.setContentsMargins(22, 18, 22, 14)
        lay.setSpacing(12)
        self.lbl = QLabel()
        lay.addWidget(self.lbl)
        row = QHBoxLayout()
        row.setSpacing(8)
        btn_cancel = QPushButton("取消关闭")
        btn_more = QPushButton("延时 30s")
        btn_open = QPushButton("打开输出目录")
        btn_cancel.clicked.connect(self.reject)
        btn_more.clicked.connect(self._extend)
        btn_open.clicked.connect(self._open_out)
        row.addWidget(btn_cancel)
        row.addWidget(btn_more)
        row.addStretch(1)
        row.addWidget(btn_open)
        lay.addLayout(row)
        self.timer = QTimer(self)
        self.timer.setInterval(1000)
        self.timer.timeout.connect(self._tick)
        self._render()
        self.timer.start()

    def _render(self):
        self.lbl.setText(f"全部完成，{self.remaining} 秒后自动关闭程序。")

    def _tick(self):
        self.remaining -= 1
        if self.remaining <= 0:
            self.timer.stop()
            self.accept()          # 归零 → 主窗口 close，程序退出
        else:
            self._render()

    def _extend(self):
        self.remaining += 30
        self._render()

    def _open_out(self):
        if self.out_dir and os.path.isdir(self.out_dir):
            try:
                os.startfile(self.out_dir)  # type: ignore[attr-defined]
            except Exception:
                pass


class LogDialog(QDialog):
    """运行日志弹窗。"""
    def __init__(self, parent=None):
        super().__init__(parent)
        self.setWindowTitle("运行日志")
        self.setModal(False)
        self.resize(720, 460)
        lay = QVBoxLayout(self)
        lay.setContentsMargins(0, 0, 0, 0)
        lay.setSpacing(0)

        head = QWidget(objectName="modal-head")
        hl = QHBoxLayout(head)
        hl.setContentsMargins(12, 8, 12, 8)
        lbl = QLabel("运行日志")
        clear = QPushButton("清空")
        close = QPushButton("关闭")
        clear.clicked.connect(self.clear_log)
        close.clicked.connect(self.close)
        hl.addWidget(lbl)
        hl.addStretch(1)
        hl.addWidget(clear)
        hl.addWidget(close)
        lay.addWidget(head)

        self.view = QTextEdit(readOnly=True)
        self.view.setAcceptRichText(True)
        self.view.setLineWrapMode(QTextEdit.LineWrapMode.NoWrap)
        lay.addWidget(self.view)

    def append(self, level: str, line: str):
        win = self.parent()
        k = "dark" if getattr(win, "dark", True) else "light"
        col = {"info": COLORS[k]["text2"], "ok": COLORS[k]["ok"],
               "warn": COLORS[k]["warn"], "err": COLORS[k]["err"]}.get(level, COLORS[k]["text"])
        t = time.strftime("%H:%M:%S")
        self.view.append(f'<span style="color:{COLORS[k]["text2"]}">{t}</span>&nbsp;&nbsp;'
                         f'<span style="color:{col}">{line}</span>')
        bar = self.view.verticalScrollBar()
        bar.setValue(bar.maximum())

    def clear_log(self):
        self.view.clear()


class MainWindow(QMainWindow):
    def __init__(self):
        super().__init__()
        self.dark = True
        self.setWindowTitle(f"内容扫描器 · {APP_NAME}")
        self.resize(1180, 760)
        self.setMinimumSize(980, 640)
        self._center_on_screen()

        self._logs = []            # 会话日志 [[t, level, line], ...]
        self._worker: ScanWorker | None = None
        self._running = False

        self.log_dialog = LogDialog(self)
        self._build_ui()
        self._load_cfg()
        self._apply_theme()

    def _center_on_screen(self):
        """主窗口在可用屏幕区域内居中，避免出现在屏幕外。"""
        try:
            scr = QApplication.primaryScreen()
            if scr:
                geo = scr.availableGeometry()
                fw = self.frameGeometry()
                fw.moveCenter(geo.center())
                self.move(fw.topLeft())
        except Exception:
            pass

    # ---------- UI 构建 ----------
    def _build_ui(self):
        central = QWidget()
        self.setCentralWidget(central)
        root = QVBoxLayout(central)
        root.setContentsMargins(0, 0, 0, 0)
        root.setSpacing(0)

        # 顶栏
        top = QWidget(objectName="topbar")
        tl = QHBoxLayout(top)
        tl.setContentsMargins(14, 8, 14, 8)
        brand = QLabel("内容扫描器")
        brand.setStyleSheet("font-size:14px;font-weight:600;")
        sub = QLabel(APP_NAME, objectName="sub")
        self.theme_btn = QPushButton("浅色")
        self.theme_btn.setObjectName("ghost")
        log_btn = QPushButton("日志")
        log_btn.setObjectName("ghost")
        open_btn = QPushButton("打开输出目录")
        open_btn.setObjectName("ghost")
        self.theme_btn.clicked.connect(self._toggle_theme)
        log_btn.clicked.connect(self._open_log)
        open_btn.clicked.connect(self._open_outdir)
        tl.addWidget(brand)
        tl.addWidget(sub)
        tl.addStretch(1)
        tl.addWidget(self.theme_btn)
        tl.addWidget(log_btn)
        tl.addWidget(open_btn)
        root.addWidget(top)

        # 中部：左配置 + 右结果
        body = QWidget()
        bl = QHBoxLayout(body)
        bl.setContentsMargins(0, 0, 0, 0)
        bl.setSpacing(0)

        panel = QWidget(objectName="panel")
        pv = QVBoxLayout(panel)
        pv.setContentsMargins(14, 12, 14, 12)
        pv.setSpacing(10)

        h2 = QLabel("扫描配置", objectName="h2")
        pv.addWidget(h2)
        self._build_config(panel, pv)
        self._build_filter_group(panel, pv)
        pv.addStretch(1)
        bl.addWidget(panel)

        # 右侧
        right = QWidget()
        rv = QVBoxLayout(right)
        rv.setContentsMargins(0, 0, 0, 0)
        rv.setSpacing(0)
        self._build_result(right, rv)
        bl.addWidget(right, 1)
        root.addWidget(body, 1)

        # 底部日志条
        logbar = QWidget(objectName="logbar")
        ll = QHBoxLayout(logbar)
        ll.setContentsMargins(12, 5, 12, 5)
        self.log_ind = QLabel("●")
        self.log_ind.setStyleSheet("color:#22c55e;font-size:10px;")
        lt = QPushButton("日志")
        lt.setObjectName("ghost")
        lt.clicked.connect(self._open_log)
        self.log_tail = QLabel("等待任务…")
        self.log_tail.setStyleSheet("color:#979da6;")
        ll.addWidget(self.log_ind)
        ll.addWidget(lt)
        ll.addWidget(self.log_tail, 1)
        root.addWidget(logbar)

    def _build_config(self, panel, pv: QVBoxLayout):
        mode_row = QHBoxLayout()
        mode_row.setSpacing(6)
        mode_lbl = QLabel("工作模式")
        self.work_combo = QComboBox()
        self.work_combo.addItem("通用扫描（包含 / 不包含）", "scan")
        self.work_combo.addItem("日志筛选 / 回传（etest 系）", "filter")
        self.work_combo.currentIndexChanged.connect(self._toggle_work_mode)
        mode_row.addWidget(mode_lbl)
        mode_row.addWidget(self.work_combo, 1)
        pv.addLayout(mode_row)

        grid = QGridLayout()
        grid.setVerticalSpacing(8)
        grid.setHorizontalSpacing(10)

        # 目录
        grid.addWidget(QLabel("扫描目录"), 0, 0)
        dir_row = QHBoxLayout()
        dir_row.setSpacing(6)
        self.dir_edit = QLineEdit()
        self.dir_edit.setProperty("mono", "true")
        browse = QPushButton("…")
        browse.setFixedWidth(30)
        browse.clicked.connect(self._pick_dir)
        dir_row.addWidget(self.dir_edit, 1)
        dir_row.addWidget(browse)
        grid.addLayout(dir_row, 0, 1)

        # 关键字（仅通用扫描）
        self._kw_label = QLabel("关键字 / 字符串")
        grid.addWidget(self._kw_label, 1, 0)
        self.kw_edit = QLineEdit()
        self.kw_edit.setProperty("mono", "true")
        grid.addWidget(self.kw_edit, 1, 1)

        # 模式（仅通用扫描）
        self._mode_label = QLabel("匹配模式")
        grid.addWidget(self._mode_label, 2, 0)
        mode_row = QHBoxLayout()
        mode_row.setSpacing(6)
        self.mode_inc = QPushButton("包含")
        self.mode_exc = QPushButton("不包含")
        self.mode_inc.clicked.connect(lambda: self._set_mode("inc"))
        self.mode_exc.clicked.connect(lambda: self._set_mode("exc"))
        for b in (self.mode_inc, self.mode_exc):
            b.setCheckable(True)
            b.setSizePolicy(QSizePolicy.Policy.Expanding, QSizePolicy.Policy.Fixed)
        mode_row.addWidget(self.mode_inc)
        mode_row.addWidget(self.mode_exc)
        grid.addLayout(mode_row, 2, 1)

        # 线程
        grid.addWidget(QLabel("并发线程数"), 3, 0)
        thr_row = QHBoxLayout()
        thr_row.setSpacing(8)
        self.thread_slider = QSlider(Qt.Orientation.Horizontal)
        self.thread_slider.setRange(1, 64)
        self.thread_spin = QSpinBox()
        self.thread_spin.setRange(1, 64)
        self.thread_slider.valueChanged.connect(self.thread_spin.setValue)
        self.thread_spin.valueChanged.connect(self.thread_slider.setValue)
        thr_row.addWidget(self.thread_slider, 1)
        thr_row.addWidget(self.thread_spin)
        grid.addLayout(thr_row, 3, 1)

        # 扩展名
        grid.addWidget(QLabel("文件扩展名"), 4, 0)
        self.ext_edit = QLineEdit()
        self.ext_edit.setProperty("mono", "true")
        self.ext_edit.setPlaceholderText("txt,log,csv,md,…（留空=全部）")
        grid.addWidget(self.ext_edit, 4, 1)

        # 编码
        grid.addWidget(QLabel("编码"), 5, 0)
        self.enc_combo = QComboBox()
        for label, val in [("自动探测 (UTF-8 / GBK)", "auto"), ("UTF-8", "utf-8"),
                           ("GBK / GB2312", "gbk"), ("UTF-16", "utf-16"), ("ASCII", "ascii")]:
            self.enc_combo.addItem(label, val)
        grid.addWidget(self.enc_combo, 5, 1)

        # 选项
        grid.addWidget(QLabel("选项"), 6, 0)
        opt_box = QWidget()
        ov = QVBoxLayout(opt_box)
        ov.setContentsMargins(0, 0, 0, 0)
        ov.setSpacing(4)
        self.case_check = QCheckBox("大小写敏感")
        self.rec_check = QCheckBox("递归子目录")
        self.copy_check = QCheckBox("将命中文件复制到 out")
        self.record_check = QCheckBox("同时在 Excel 记录未命中文件")
        self.rec_check.setChecked(True)
        self.record_check.setChecked(True)
        for cb in (self.case_check, self.rec_check, self.copy_check, self.record_check):
            ov.addWidget(cb)
        grid.addWidget(opt_box, 6, 1)

        # 输出目录
        grid.addWidget(QLabel("输出目录"), 7, 0)
        self.out_edit = QLineEdit()
        self.out_edit.setProperty("mono", "true")
        grid.addWidget(self.out_edit, 7, 1)

        pv.addLayout(grid)
        # 日志筛选模式下隐藏的通用扫描专属控件（共享项：扫描/输出目录、线程数、编码、扩展名、递归、复制命中）
        self._filter_hide = [self._kw_label, self.kw_edit, self._mode_label, self.mode_inc,
                             self.mode_exc, self.case_check, self.record_check]

    def _build_filter_group(self, panel, pv: QVBoxLayout):
        """日志筛选 / 回传配置组（工作模式=日志筛选时显示）。"""
        box = QGroupBox("日志筛选 / 回传")
        gv = QVBoxLayout(box)
        gv.setContentsMargins(10, 8, 10, 8)
        gv.setSpacing(8)
        g = QGridLayout()
        g.setVerticalSpacing(8)
        g.setHorizontalSpacing(10)

        g.addWidget(QLabel("筛选类型"), 0, 0)
        self.type_combo = QComboBox()
        for label, val in [("自动识别", LogType.AUTO.value), ("etest(OA3)", LogType.ETEST_OA3.value),
                           ("etest", LogType.ETEST.value), ("e-autotest", LogType.EAUTOTEST.value),
                           ("海格旧测试2", LogType.HEG_AUTOTEST2.value), ("海格旧测试3", LogType.HEG_AUTOTEST3.value)]:
            self.type_combo.addItem(label, val)
        g.addWidget(self.type_combo, 0, 1)

        self.upload_check = QCheckBox("启用数据回传（逐台调第三方 CLI；自动模式下仅 etest(OA3)）")
        self.dry_check = QCheckBox("dry-run（只组包校验，不调 CLI）")
        self.dry_check.setChecked(True)
        self.autoclose_check = QCheckBox("完成后倒计时自动关闭程序")
        for r, cb in enumerate((self.upload_check, self.dry_check, self.autoclose_check), start=1):
            g.addWidget(cb, r, 0, 1, 2)

        g.addWidget(QLabel("CLI 路径"), 5, 0)
        cli_row = QHBoxLayout()
        cli_row.setSpacing(6)
        self.cli_edit = QLineEdit()
        self.cli_edit.setProperty("mono", "true")
        self.cli_edit.setPlaceholderText("留空=程序目录下 intunehelper_cli.exe")
        cli_browse = QPushButton("…")
        cli_browse.setFixedWidth(30)
        cli_browse.clicked.connect(self._pick_cli)
        cli_row.addWidget(self.cli_edit, 1)
        cli_row.addWidget(cli_browse)
        g.addLayout(cli_row, 5, 1)

        g.addWidget(QLabel("SecretKey"), 6, 0)
        self.secret_edit = QLineEdit()
        self.secret_edit.setProperty("mono", "true")
        self.secret_edit.setEchoMode(QLineEdit.EchoMode.Password)
        g.addWidget(self.secret_edit, 6, 1)

        g.addWidget(QLabel("参数模板"), 7, 0)
        self.args_edit = QLineEdit()
        self.args_edit.setProperty("mono", "true")
        self.args_edit.setToolTip("占位符 ~key~：~secret_key~ / ~payload~ / 提取字段（~sn~ 等）。"
                                  "含 ~payload~ 时走参数传 JSON，否则 payload 写 stdin")
        g.addWidget(self.args_edit, 7, 1)

        g.addWidget(QLabel("超时 / 重试"), 8, 0)
        tr = QHBoxLayout()
        tr.setSpacing(8)
        self.timeout_spin = QSpinBox()
        self.timeout_spin.setRange(5, 600)
        self.timeout_spin.setSuffix(" s")
        self.retry_spin = QSpinBox()
        self.retry_spin.setRange(0, 10)
        self.retry_spin.setPrefix("重试 ")
        tr.addWidget(self.timeout_spin, 1)
        tr.addWidget(self.retry_spin, 1)
        g.addLayout(tr, 8, 1)

        g.addWidget(QLabel("倒计时"), 9, 0)
        self.countdown_spin = QSpinBox()
        self.countdown_spin.setRange(3, 3600)
        self.countdown_spin.setSuffix(" s")
        g.addWidget(self.countdown_spin, 9, 1)

        gv.addLayout(g)
        self.filter_group = box
        box.setVisible(False)
        pv.addWidget(box)

    def _build_result(self, right, rv: QVBoxLayout):
        # 顶部统计 + 操作
        toolbar = QWidget(objectName="stat")
        tl = QHBoxLayout(toolbar)
        tl.setContentsMargins(14, 8, 14, 8)
        self.st_scanned = QLabel("0", objectName="n")
        self.st_hit = QLabel("0", objectName="n good")
        self.st_miss = QLabel("0", objectName="n bad")
        self.st_skip = QLabel("0", objectName="n")
        self.st_time = QLabel("0s")
        for lbl, cap in [(self.st_scanned, "已扫描"), (self.st_hit, "命中"),
                         (self.st_miss, "未命中"), (self.st_skip, "跳过"), (self.st_time, "耗时")]:
            box = QVBoxLayout()
            box.setSpacing(1)
            box.addWidget(lbl)
            lab = QLabel(cap)
            lab.setStyleSheet("color:#979da6;font-size:10px;")
            box.addWidget(lab)
            tl.addLayout(box)
        tl.addStretch(1)

        self.progress = QProgressBar()
        self.progress.setFixedWidth(260)
        self.progress.setRange(0, 100)
        tl.addWidget(self.progress)
        self.prog_text = QLabel("就绪")
        self.prog_text.setStyleSheet("color:#979da6;font-size:11px;")
        tl.addWidget(self.prog_text)

        self.go_btn = QPushButton("开始扫描", objectName="primary")
        self.stop_btn = QPushButton("停止", objectName="stop")
        self.stop_btn.setEnabled(False)
        self.go_btn.clicked.connect(self._start)
        self.stop_btn.clicked.connect(self._stop)
        tl.addWidget(self.go_btn)
        tl.addWidget(self.stop_btn)
        rv.addWidget(toolbar)

        # 回传进度行（日志筛选模式显示）
        self.up_row = QWidget(objectName="stat")
        ul = QHBoxLayout(self.up_row)
        ul.setContentsMargins(14, 0, 14, 8)
        up_lbl = QLabel("回传")
        up_lbl.setStyleSheet("color:#979da6;font-size:11px;")
        self.up_progress = QProgressBar()
        self.up_progress.setRange(0, 100)
        self.up_progress.setFixedWidth(260)
        self.up_text = QLabel("—")
        self.up_text.setStyleSheet("color:#979da6;font-size:11px;")
        ul.addWidget(up_lbl)
        ul.addWidget(self.up_progress)
        ul.addWidget(self.up_text, 1)
        self.up_row.setVisible(False)
        rv.addWidget(self.up_row)

        # 结果表格
        self.table = QTableWidget(0, 11)
        self.table.setHorizontalHeaderLabels(
            ["#", "相对路径", "目录", "扩展名", "包含状态", "命中行号", "命中行内容", "匹配计数", "大小", "修改时间", "编码"])
        self.table.setEditTriggers(QAbstractItemView.EditTrigger.NoEditTriggers)
        self.table.setSelectionBehavior(QAbstractItemView.SelectionBehavior.SelectRows)
        self.table.verticalHeader().setVisible(False)
        self.table.horizontalHeader().setSectionResizeMode(QHeaderView.ResizeMode.Interactive)
        self.table.horizontalHeader().setStretchLastSection(False)
        self.table.setColumnWidth(1, 300)
        self.table.setColumnWidth(2, 120)
        self.table.setColumnWidth(6, 240)
        self._set_table_mode(False)
        rv.addWidget(self.table, 1)

    SCAN_HEADERS = ["#", "相对路径", "目录", "扩展名", "包含状态", "命中行号", "命中行内容", "匹配计数", "大小", "修改时间", "编码"]

    def _set_table_mode(self, is_filter: bool):
        """两种工作模式的表头与列宽。"""
        if is_filter:
            headers = ["#", "相对路径", "判型", "SN", "提取", "OA3结果", "PKID", "Hash长度", "回传", "request_id / 错误"]
            widths = {1: 280, 2: 88, 3: 210, 4: 56, 5: 118, 6: 108, 7: 66, 8: 84, 9: 250}
        else:
            headers = self.SCAN_HEADERS
            widths = {1: 300, 2: 120, 6: 240}
        self.table.clear()
        self.table.setColumnCount(len(headers))
        self.table.setHorizontalHeaderLabels(headers)
        for c, w in widths.items():
            self.table.setColumnWidth(c, w)

    # ---------- 配置读写 ----------
    def _pick_dir(self):
        d = QFileDialog.getExistingDirectory(self, "选择扫描目录", self.dir_edit.text() or ".")
        if d:
            self.dir_edit.setText(d)

    def _pick_cli(self):
        f, _ = QFileDialog.getOpenFileName(self, "选择回传 CLI", self.cli_edit.text() or APP_DIR,
                                           "可执行文件 (*.exe);;所有文件 (*.*)")
        if f:
            self.cli_edit.setText(f)

    def apply_auto(self, auto) -> bool:
        """TOML 自动化：覆盖 UI 配置；通过校验则标记自动开跑。"""
        cfg = self._collect_cfg()
        autoconfig.apply_to_config(auto, cfg)
        errs = cfg.validate()
        if errs:
            self._push_log("err", "TOML 配置错误：" + "; ".join(errs))
            return False
        self._apply_cfg(cfg)
        self._auto_sn = cfg.filter_sn
        self._auto_file = cfg.filter_file
        self._auto_pending = bool(getattr(auto, "enabled", False))
        scheme = "SN关联多文件" if cfg.filter_sn else ("单文件" if cfg.filter_file else "目录遍历")
        tail = "，即将自动开始…" if auto.auto_start else "（auto_start=false，手动点开始或改配置后自动跑）"
        self._push_log("info", f"自动化配置已加载（方案：{scheme}，dry-run={'开' if cfg.upload_dry_run else '关'}{tail}")
        return True

    def _toggle_work_mode(self):
        is_filter = self.work_combo.currentData() == "filter"
        if not is_filter:
            self._auto_sn = ""
            self._auto_file = ""
        self.filter_group.setVisible(is_filter)
        for w in getattr(self, "_filter_hide", []):
            w.setVisible(not is_filter)   # 扫描专属项隐藏，界面干净；共享项保持可用
        self.up_row.setVisible(is_filter)
        self._set_table_mode(is_filter)

    def _set_mode(self, mode: str):
        self.mode_inc.setChecked(mode == "inc")
        self.mode_exc.setChecked(mode == "exc")

    def _collect_cfg(self) -> SearchConfig:
        cfg = SearchConfig()
        cfg.root_dir = self.dir_edit.text().strip()
        cfg.keyword = self.kw_edit.text().strip()
        cfg.mode = "inc" if self.mode_inc.isChecked() else "exc"
        cfg.threads = self.thread_spin.value()
        cfg.extensions = [e.strip().lstrip(".") for e in self.ext_edit.text().split(",") if e.strip()]
        cfg.encoding = self.enc_combo.currentData()
        cfg.case_sensitive = self.case_check.isChecked()
        cfg.recursive = self.rec_check.isChecked()
        cfg.copy_files = self.copy_check.isChecked()
        cfg.record_miss = self.record_check.isChecked()
        out = self.out_edit.text().strip()
        cfg.out_dir = out if out else default_out_dir(APP_DIR)
        # 日志筛选 / 回传
        cfg.work_mode = self.work_combo.currentData() or "scan"
        cfg.filter_log_type = self.type_combo.currentData() or "auto"
        cfg.upload_enabled = self.upload_check.isChecked()
        cfg.upload_dry_run = self.dry_check.isChecked()
        cfg.upload_cli_path = self.cli_edit.text().strip()
        cfg.upload_secret_key = self.secret_edit.text().strip()
        cfg.upload_args = self.args_edit.text().strip() or "upload --stdin --secret-key ~secret_key~"
        cfg.upload_timeout = float(self.timeout_spin.value())
        cfg.upload_retries = self.retry_spin.value()
        cfg.upload_stdin = "~payload~" not in cfg.upload_args   # 模板含 ~payload~ 则走参数，否则写 stdin
        cfg.filter_countdown = self.countdown_spin.value()
        cfg.filter_auto_close = self.autoclose_check.isChecked()
        # TOML 自动化方案（SN 关联 / 单文件）：无对应控件，挂窗口属性回填
        cfg.filter_sn = getattr(self, "_auto_sn", "")
        cfg.filter_file = getattr(self, "_auto_file", "")
        return cfg

    def _apply_cfg(self, cfg: SearchConfig):
        self.dir_edit.setText(cfg.root_dir)
        self.kw_edit.setText(cfg.keyword)
        self._set_mode(cfg.mode)
        self.thread_spin.setValue(cfg.threads)
        self.ext_edit.setText(",".join(cfg.extensions))
        idx = self.enc_combo.findData(cfg.encoding)
        self.enc_combo.setCurrentIndex(idx if idx >= 0 else 0)
        self.case_check.setChecked(cfg.case_sensitive)
        self.rec_check.setChecked(cfg.recursive)
        self.copy_check.setChecked(cfg.copy_files)
        self.record_check.setChecked(cfg.record_miss)
        self.out_edit.setText(cfg.out_dir)
        idx_w = self.work_combo.findData(cfg.work_mode)
        self.work_combo.setCurrentIndex(idx_w if idx_w >= 0 else 0)
        idx_t = self.type_combo.findData(cfg.filter_log_type)
        self.type_combo.setCurrentIndex(idx_t if idx_t >= 0 else 0)
        self.upload_check.setChecked(cfg.upload_enabled)
        self.dry_check.setChecked(cfg.upload_dry_run)
        self.cli_edit.setText(cfg.upload_cli_path)
        self.secret_edit.setText(cfg.upload_secret_key)
        self.args_edit.setText(cfg.upload_args or "upload --stdin --secret-key ~secret_key~")
        self.timeout_spin.setValue(int(cfg.upload_timeout))
        self.retry_spin.setValue(int(cfg.upload_retries))
        self.countdown_spin.setValue(int(cfg.filter_countdown))
        self.autoclose_check.setChecked(cfg.filter_auto_close)

    def _load_cfg(self):
        self._apply_cfg(load_config(APP_DIR))

    # ---------- 主题 ----------
    def _apply_theme(self):
        QApplication.instance().setStyleSheet(build_qss(self.dark))
        self.theme_btn.setText("浅色" if self.dark else "深色")
        self._rerender_log()

    def _toggle_theme(self):
        self.dark = not self.dark
        self._apply_theme()
        self._push_log("info", f"已切换为{'深色' if self.dark else '浅色'}主题")

    def _rerender_log(self):
        """切主题时按当前配色重渲染会话日志。"""
        self.log_dialog.view.clear()
        for _t, level, line in self._logs:
            self.log_dialog.append(level, line)

    # ---------- 日志 ----------
    def _push_log(self, level: str, line: str):
        self._logs.append([time.strftime("%H:%M:%S"), level, line])
        # 落盘到 logs/findany-run.log
        _write_file("findany-run.log", time.strftime("%Y-%m-%d %H:%M:%S") + "  [" + level.upper() + "]  " + line + "\n")
        # 弹窗着色
        self.log_dialog.append(level, line)
        # 底部状态条
        self.log_tail.setText(line)
        if level == "err":
            self.log_ind.setStyleSheet("color:#ef4444;font-size:10px;")
        elif level == "ok":
            self.log_ind.setStyleSheet("color:#22c55e;font-size:10px;")
        elif level == "warn":
            self.log_ind.setStyleSheet("color:#f59e0b;font-size:10px;")
        else:
            self.log_ind.setStyleSheet("color:#5686fe;font-size:10px;")

    def _open_log(self):
        self.log_dialog.show()
        self.log_dialog.raise_()

    def _open_outdir(self):
        d = self.out_edit.text().strip() or default_out_dir(APP_DIR)
        if not os.path.isdir(d):
            os.makedirs(d, exist_ok=True)
        try:
            os.startfile(d)  # type: ignore[attr-defined]
        except Exception:
            self._push_log("warn", f"无法打开输出目录：{d}")

    # ---------- 扫描 ----------
    def _start(self):
        if self._running:
            return
        cfg = self._collect_cfg()
        errs = cfg.validate()
        if errs:
            QMessageBox.warning(self, "配置错误", "\n".join(errs))
            self._push_log("err", "配置错误：" + "; ".join(errs))
            return
        save_config(APP_DIR, cfg)
        self._running = True
        self._set_running(True)
        self.table.setRowCount(0)
        self.progress.setValue(0)
        self.up_progress.setValue(0)
        if cfg.work_mode == "filter":
            self.prog_text.setText("筛选中…")
            self.up_text.setText("等待提取完成…")
            self._push_log("info", f"开始日志筛选：{cfg.root_dir}  类型「{cfg.filter_log_type}」  "
                                   f"回传{'开' if cfg.upload_enabled else '关'}"
                                   + ("（dry-run）" if cfg.upload_enabled and cfg.upload_dry_run else ""))
            self._worker = FilterWorker(cfg)
            self._worker.upload.connect(self._on_upload_progress)
        else:
            self.prog_text.setText("扫描中…")
            self._push_log("info", f"开始扫描：{cfg.root_dir}  关键字「{cfg.keyword}」 模式：{'包含' if cfg.mode=='inc' else '不包含'}  线程 {cfg.threads}")
            self._worker = ScanWorker(cfg)
        self._worker.progress.connect(self._on_progress)
        self._worker.log.connect(self._push_log)
        self._worker.done.connect(self._on_done)
        self._worker.error.connect(self._on_error)
        self._worker.start()

    def _stop(self):
        if self._worker:
            self._worker.cancel()
            self._push_log("warn", "正在停止…")

    def _set_running(self, running: bool):
        self._running = running
        self.go_btn.setEnabled(not running)
        self.stop_btn.setEnabled(running)

    def _on_progress(self, d: dict):
        self.progress.setValue(int(d.get("pct", 0)))
        if d.get("phase") == "extract":
            self.prog_text.setText(f'提取 {d.get("done", 0)}/{d.get("total", 0)}')
        else:
            self.prog_text.setText(f'{d.get("done", 0)}/{d.get("total", 0)}')
        self.st_scanned.setText(str(d.get("done", 0)))
        self.st_hit.setText(str(d.get("hit", "—")))
        self.st_miss.setText(str(d.get("miss", "—")))
        self.st_skip.setText(str(d.get("skipped", "—")))

    def _on_upload_progress(self, d: dict):
        self.up_progress.setValue(int(d.get("pct", 0)))
        label = {"ok": "成功", "conflict": "冲突", "fail": "失败", "dry_run": "dry-run"}.get(d.get("status"), "")
        self.up_text.setText(f'{d.get("index", 0)}/{d.get("total", 0)}  {d.get("sn", "")}  {label}')

    def _on_done(self, items, summary):
        if isinstance(self._worker, FilterWorker):
            self._on_filter_done(items, summary)
            return
        cfg = self._extract_last_cfg()
        # 导出 + 落盘
        try:
            batch = make_batch_dir(cfg.out_dir)
            xlsx = os.path.join(batch.path, "scan_result.xlsx")
            p = export_excel(xlsx, items, summary, cfg)
            copied = 0
            if cfg.copy_files:
                copied = copy_hits(items, batch.path, cfg.mode)
            self._last_output_dir = batch.path
            self._push_log("ok", f"完成：命中 {summary.hit} / 未命中 {summary.miss} / 跳过 {summary.skipped}，耗时 {summary.elapsed}s")
            self._push_log("ok", f"Excel：{os.path.basename(p)}" + (f"  落盘文件：{copied}" if cfg.copy_files else ""))
            self._push_log("info", f"输出目录：{batch.path}")
        except Exception as e:
            self._push_log("err", f"导出失败：{e}\n{traceback.format_exc()}")
        self._populate_table(items, cfg)
        self._finish_ui(summary)

    def _on_filter_done(self, items, summary):
        cfg = self._collect_cfg()
        self._populate_filter_table(items)
        self._set_running(False)
        self.prog_text.setText("完成")
        self.st_time.setText(f"{summary.elapsed}s")
        self.st_scanned.setText(str(summary.total))
        self.st_hit.setText(str(summary.extracted))
        self.st_miss.setText(str(summary.unknown))
        self.st_skip.setText(str(summary.skipped))
        self.up_progress.setValue(100)
        self.up_text.setText(
            f"成功 {summary.upload_ok} / 冲突 {summary.upload_conflict} / 失败 {summary.upload_fail}"
            + (f" / dry-run {summary.upload_dry}" if summary.upload_dry else "")
            if cfg.upload_enabled else "未启用回传")
        self._push_log("ok", f"筛选完成：提取 {summary.extracted}/{summary.total}（未知类型 {summary.unknown}），"
                             f"回传 成功 {summary.upload_ok} / 冲突 {summary.upload_conflict} / 失败 {summary.upload_fail}")
        self._last_output_dir = summary.batch_dir
        self._worker = None
        if cfg.filter_auto_close:
            dlg = CountdownDialog(self, cfg.filter_countdown, summary.batch_dir)
            if dlg.exec():
                self.close()       # 倒计时归零 → 退出程序
        elif getattr(self, "_auto_pending", False):
            # TOML 自动化（auto_close=false）：不打断，保持界面
            self._push_log("ok", "自动化流程完成（auto_close=false，保持界面打开）")
        else:
            ret = QMessageBox.question(self, "完成",
                                       f"日志筛选完成。\n输出：{summary.batch_dir}\n是否打开输出目录？")
            if ret == QMessageBox.StandardButton.Yes:
                try:
                    os.startfile(summary.batch_dir)  # type: ignore[attr-defined]
                except Exception:
                    pass
        if self.isVisible():
            self.log_dialog.show()

    def _populate_filter_table(self, items):
        self.table.setRowCount(len(items))
        c_ok = COLORS["dark" if self.dark else "light"]["ok"]
        c_warn = COLORS["dark" if self.dark else "light"]["warn"]
        c_err = COLORS["dark" if self.dark else "light"]["err"]
        for r, it in enumerate(items):
            up = str(it.get("upload_state", "") or "")
            vals = [str(r + 1), it.get("rel_path", ""), it.get("detected_type", ""), it.get("sn", ""),
                    "成功" if it.get("extract_ok") else "失败", it.get("oa3_result", ""),
                    str(it.get("product_key_id", "") or ""), str(it.get("hardware_hash_len", "") or ""),
                    up, (it.get("request_id", "") or it.get("upload_error", "") or it.get("error", ""))]
            for c, v in enumerate(vals):
                t = QTableWidgetItem(v)
                if c == 4:
                    t.setForeground(QBrush(QColor(c_ok if it.get("extract_ok") else c_err)))
                elif c == 8 and up:
                    t.setForeground(QBrush(QColor({"成功": c_ok, "冲突(人工)": c_warn}.get(up, c_err))))
                self.table.setItem(r, c, t)

    def _on_error(self, msg: str):
        self._push_log("err", "扫描异常：\n" + msg)
        self._finish_ui(None)

    def _finish_ui(self, summary):
        self._set_running(False)
        self.prog_text.setText("完成" if summary else "已停止")
        if summary:
            self.st_time.setText(f"{summary.elapsed}s")
            self.st_scanned.setText(str(summary.scanned))
            self.st_hit.setText(str(summary.hit))
            self.st_miss.setText(str(summary.miss))
            self.st_skip.setText(str(summary.skipped))
        # 弹提示是否打开输出目录
        if getattr(self, "_last_output_dir", None):
            ret = QMessageBox.question(self, "完成", f"扫描完成。\n输出：{self._last_output_dir}\n是否打开输出目录？")
            if ret == QMessageBox.StandardButton.Yes:
                try:
                    os.startfile(self._last_output_dir)  # type: ignore[attr-defined]
                except Exception:
                    pass
        self._worker = None
        if self.isVisible():
            self.log_dialog.show()

    def _extract_last_cfg(self) -> SearchConfig:
        return self._collect_cfg()

    def _populate_table(self, items, cfg: SearchConfig):
        rows = [i for i in items if i.hit or cfg.record_miss]
        self.table.setRowCount(len(rows))
        for r, it in enumerate(rows):
            state = "命中" if it.hit else "未命中"
            lines = ",".join(str(x) for x in it.hit_lines) if it.hit else ""
            vals = [str(r + 1), it.rel_path, it.dir_name,
                    (it.filename.rsplit(".", 1)[1].lower() if "." in it.filename else ""),
                    state, lines, it.hit_line_text if it.hit else "",
                    str(it.hit_count if it.hit else 0), it.size_str, it.mtime_str, it.encoding]
            for c, v in enumerate(vals):
                item = QTableWidgetItem(v)
                if c == 4:  # 状态列着色
                    item.setForeground(QBrush(QColor(COLORS["dark" if self.dark else "light"]["ok"] if it.hit else COLORS["dark" if self.dark else "light"]["err"])))
                self.table.setItem(r, c, item)

    def closeEvent(self, ev):
        if self._worker and self._worker.isRunning():
            self._worker.cancel()
            self._worker.wait(2000)
        super().closeEvent(ev)


def _mark(msg: str):
    """逐步启动日志，便于定位 python app.py 在哪一步失败。写入 logs/findany-startup.log。"""
    _write_file("findany-startup.log", time.strftime("%Y-%m-%d %H:%M:%S") + "  " + msg + "\n")


def main():
    global app
    _mark("main start | python=" + sys.executable + " | appdir=" + APP_DIR)
    try:
        app = QApplication(sys.argv)
        app.setApplicationName(APP_NAME)
        _mark("QApplication created")
        w = MainWindow()
        _mark("MainWindow constructed")
        w.show()
        _mark("window shown")
        try:
            w.raise_()
            w.activateWindow()
            _mark("window raised/activated")
        except Exception:
            _mark("raise/activate failed")
        # TOML 自动化：--config findany.toml 或程序目录 findany.toml（run.auto_start=true）
        try:
            ns = autoconfig.parse_args(sys.argv[1:])
            auto = autoconfig.resolve_auto(ns, APP_DIR)
            if auto is not None:
                if auto.generated:
                    _toml_path = ns.config or os.path.join(APP_DIR, "findany.toml")
                    _mark("default toml generated: " + _toml_path)
                    w._push_log("ok", f"未找到 TOML 配置，已生成默认模板：{_toml_path}"
                                      f"（编辑 root_dir / scheme 后把 run.auto_start 改为 true 即自动开跑）")
                if w.apply_auto(auto) and auto.auto_start:
                    QTimer.singleShot(300, w._start)   # 等 UI 布局稳定后自动开跑
        except SystemExit:
            raise
        except Exception:
            _err2 = traceback.format_exc()
            _mark("auto config error: " + _err2.replace("\n", " | "))
            _write_file("findany-crash.log", _err2, "a")
        _write_file("findany-run.log", "\n===== findany 会话开始 " + time.strftime("%Y-%m-%d %H:%M:%S") + " =====\n")
        sys.exit(app.exec())
    except Exception:
        err = traceback.format_exc()
        _mark("EXCEPTION: " + err.replace("\n", " | "))
        try:
            _write_file("findany-crash.log", err, "w")
        except Exception:
            pass
        try:
            QMessageBox.critical(None, f"{APP_NAME} 启动失败", err)
        except Exception:
            pass
        if sys.stderr:
            sys.stderr.write(err)
        sys.exit(1)


if __name__ == "__main__":
    main()
