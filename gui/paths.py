"""仓库路径锚点。

界面里的每一个路径都从仓库根推导，所以 GUI 不依赖启动时的工作目录
（``streamlit run`` 的 cwd 取决于调用方，见 U8/S57）。
"""

from __future__ import annotations

import os
import tempfile
from pathlib import Path

#: 仓库根：``gui/`` 就在它下面。
ROOT = Path(__file__).resolve().parents[1]

#: 样本目录（``samples/*.pcap``，.gitignore 排除内容，只在本机存在）。
SAMPLES_DIR = ROOT / "samples"
#: 规则目录（引擎与 CLI 的默认值）。
RULES_DIR = ROOT / "rules"
#: 报告输出目录（.gitignore 排除）。
REPORTS_DIR = ROOT / "reports"
#: 默认数据库文件。
DB_PATH = ROOT / "packetsage.db"
#: 默认存储 URL：与 CLI 的 ``sqlite://packetsage.db`` 同一个文件，但写成绝对
#: 路径，这样引擎、``db query`` 与界面三方看到的一定是同一个库。
#: ``$PACKETSAGE_GUI_DB`` 可以整体换一个库（验收脚本与多机演示用）。
DB_URL = os.environ.get("PACKETSAGE_GUI_DB") or f"sqlite://{DB_PATH}"
#: 引擎 stderr 的落盘位置（``agent/.env`` 之外，.gitignore 覆盖）。
ENGINE_LOG_PATH = ROOT / "packetsage-gui.engine.log"
#: 上传的抓包先落盘再交给 ``analyze_file``（S62：路径含空格与中文必须可用）。
UPLOAD_DIR = Path(tempfile.gettempdir()) / "packetsage-gui-uploads"


def executable_name() -> str:
    """``packetsage.exe`` / ``packetsage``，按宿主平台。"""
    return "packetsage.exe" if os.name == "nt" else "packetsage"


def engine_candidates() -> list[Path]:
    """本机构建产物，按能力从高到低（release 先于 debug）。"""
    name = executable_name()
    return [
        ROOT / "target" / "release" / name,
        ROOT / "target" / "debug" / name,
    ]


def readonly_url(db_url: str) -> str:
    """给 ``packetsage db query --readonly`` 用的 URL（必须含 ``mode=ro``）。

    §8.1 的护栏在 Rust 侧：URL 里没有 ``mode=ro`` 就直接拒绝，所以这里只做
    拼接，不做别的猜测。
    """
    if "mode=ro" in db_url:
        return db_url
    joiner = "&" if "?" in db_url else "?"
    return f"{db_url}{joiner}mode=ro"


def sqlite_path(db_url: str) -> Path | None:
    """从 ``sqlite://<path>`` 取出文件路径；非 sqlite 或内存库返回 ``None``。"""
    if not db_url.startswith("sqlite://"):
        return None
    raw = db_url[len("sqlite://") :].split("?", 1)[0]
    if not raw or raw == ":memory:":
        return None
    return Path(raw)
