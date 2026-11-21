"""Polyglot fixture: Python side (entry)."""

from models import GoldWidget, helper_seal


def run() -> str:
    widget = GoldWidget("demo", 2)
    return helper_seal(widget)
