"""PacketSage 图形界面（Streamlit）。

《GUI 工程规格书 v0.1》把界面拆成四件事，本包按同样方式拆分：

* :mod:`gui.paths` —— 仓库路径锚点（样本、报告、数据库、引擎日志）；
* :mod:`gui.engine` —— 引擎/会话生命周期（U3）：一个进程一个 ``packetsage serve``；
* :mod:`gui.jobs` —— 后台线程 + 队列的渲染模型（U4/U5）：analyze 与 run 都不占脚本线程；
* :mod:`gui.views` —— 设计基线 §4 的渲染件（证据链、告警、预算、降级、报告）。

唯一入口是 ``gui/app.py``（``streamlit run gui/app.py``，或 Windows 的
``scripts/run_gui.cmd``）。界面取数一律走 Python 模块 API（收口文档 §5.12），
只有两个例外走 CLI 的机器出口：``doctor --json``（U7 的前置检查）与
``db query --readonly --jsonl``（唯一的只读 SQL 出口，用来列历史 task）。
"""

