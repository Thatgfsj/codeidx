"""Polyglot fixture: Python side (models)."""


class GoldWidget:
    """验收锚点类：app.py 中的构造与 helper_seal 调用都指向这里。"""

    def __init__(self, name: str, depth: int) -> None:
        self.name = name
        self.depth = depth

    def describe(self) -> str:
        """被本文件与 app.py 间接触发的方法。"""
        return f"{self.name}:{self.depth}"


def helper_seal(widget: GoldWidget) -> str:
    """自由函数：definition/references 的第二锚点。"""
    return widget.describe()
