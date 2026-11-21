"""Sample Python fixture: application entry."""

from utils import helper_spinner


class VaultApp:
    """演示应用类：run 内引用 utils.helper_spinner。"""

    def run(self, depth: int) -> int:
        return helper_spinner(depth)
