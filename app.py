# -*- coding: utf-8 -*-
"""内容扫描器 ContentSonar — PySide6 桌面 GUI 入口。

用法：python app.py
"""
from __future__ import annotations

import os
import sys
import time
import traceback

APP_DIR = os.path.dirname(os.path.abspath(__file__))

try:
    from PySide6.QtCore import Qt, QThread, Signal
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
except Exception:
    # 任何导入失败（pythonw 下无控制台）→ crash.log + 本机弹窗 + stderr
    _err = traceback.format_exc()
    try:
        with open(os.path.join(APP_DIR, "crash.log"), "w", encoding="utf-8") as _f:
            _f.write(_err)
    except Exception:
        pass
    try:
        import ctypes
        ctypes.windll.user32.MessageBoxW(0, _err, "ContentSonar 启动失败", 0x00000010)
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

        self._logs = []            # 会话日志 [[t, level, line], ...]
        self._worker: ScanWorker | None = None
        self._running = False

        self.log_dialog = LogDialog(self)
        self._build_ui()
        self._load_cfg()
        self._apply_theme()

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

        # 关键字
        grid.addWidget(QLabel("关键字 / 字符串"), 1, 0)
        self.kw_edit = QLineEdit()
        self.kw_edit.setProperty("mono", "true")
        grid.addWidget(self.kw_edit, 1, 1)

        # 模式
        grid.addWidget(QLabel("匹配模式"), 2, 0)
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
        rv.addWidget(self.table, 1)

    # ---------- 配置读写 ----------
    def _pick_dir(self):
        d = QFileDialog.getExistingDirectory(self, "选择扫描目录", self.dir_edit.text() or ".")
        if d:
            self.dir_edit.setText(d)

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
        self.progress.setValue(int(d["pct"]))
        self.prog_text.setText(f'{d["done"]}/{d["total"]}')
        self.st_scanned.setText(str(d["done"]))
        self.st_hit.setText(str(d["hit"]))
        self.st_miss.setText(str(d["miss"]))
        self.st_skip.setText(str(d["skipped"]))

    def _on_done(self, items, summary):
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


def main():
    global app
    try:
        app = QApplication(sys.argv)
        app.setApplicationName(APP_NAME)
        w = MainWindow()
        w.show()
        sys.exit(app.exec())
    except Exception:
        err = traceback.format_exc()
        try:
            with open(os.path.join(APP_DIR, "crash.log"), "w", encoding="utf-8") as f:
                f.write(err)
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
