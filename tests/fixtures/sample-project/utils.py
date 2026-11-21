"""Sample Python fixture: shared utilities."""


def helper_spinner(n: int) -> int:
    """自旋助手：被 app.py 的 VaultApp.run 引用。"""
    return n * 7 + 1
