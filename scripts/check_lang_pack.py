#!/usr/bin/env python3
"""Check the overlay language pack against the UI strings Kaku can render.

Collects every English original that goes through the overlay pack:

* ``tr("...")`` and ``trf!("...")`` literals in ``kaku/`` and ``kaku-gui/``
* the toast lookup tables in ``kaku-gui/src/termwindow/mod.rs``
* static ``brief: "...".into()`` command definitions (menubar, palette,
  launcher all render ``brief`` through the pack)
* key-hint labels inside ``popup_title_line(...)`` / ``key_hint_spans(...)``
  call arguments

then compares them against a pack JSON (``{english: translation}``) and
reports missing keys (renders English today) and orphan keys (pack entries
whose original no longer exists — safe to keep, but usually stale).

Strings that must never be translated are excluded by construction:
``kaku/src/cli/*`` (upstream-mirrored, machine-readable), anything under a
``#[cfg(test)]`` ``mod tests`` block, and values that reach ``tr`` only via
a variable at runtime.

Usage:
    python3 scripts/check_lang_pack.py [pack.json]

With no argument the script uses ``~/.config/kaku/lang/zh.json`` when it
exists and otherwise just prints the collected keys. Exit code is 1 when a
pack is checked and keys are missing (so CI can gate on it), 0 otherwise.
"""

import json
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SCAN_DIRS = [REPO / "kaku" / "src", REPO / "kaku-gui" / "src"]
SKIP_PARTS = ("/cli/", "/tests/", "tests.rs")

TR_RE = re.compile(r'\btr(?:f)?!\?\(\s*"((?:[^"\\]|\\.)*)"')
TRFN_RE = re.compile(r'\btrf?!?\(\s*"((?:[^"\\]|\\.)*)"')
# Rust tuple entries end with `",` before the closing paren; without the
# optional comma the regex silently skips multi-line table rows.
TOAST_TABLE_RE = re.compile(r'\(\s*"[^"]+"\s*,\s*"((?:[^"\\]|\\.)*)"\s*,?\s*\)')
BRIEF_RE = re.compile(r'brief:\s*"((?:[^"\\]|\\.)*)"\.into\(\)')
MENUBAR_RE = re.compile(r'menubar:\s*&\[((?:"(?:[^"\\]|\\.)*"\s*,?\s*)*)\]')
FOOTER_LABEL_RE = re.compile(r'\b(?:long_label|short_label):\s*"((?:[^"\\]|\\.)*)"')
HINT_RE = re.compile(r'\(\s*"(?:[^"\\]|\\.)*"\s*,\s*"((?:[^"\\]|\\.)*)"\s*\)')
HINT_CALL_RE = re.compile(r'\b(?:popup_title_line|key_hint_spans)\s*\(')
PREFIX_RE = re.compile(r'\bpopup_title_line\(\s*"((?:[^"\\]|\\.)*)"')
# Config TUI field tables: `key`/`section` carry display text and
# `choices(&["On", "Off"])` lists the option values that get looked up when
# rendered. `default` is deliberately NOT collected: it is a persisted
# value (numbers, key combos, font names, model IDs), never a label.
FIELD_KEY_RE = re.compile(r'\b(?:key|section):\s*"((?:[^"\\]|\\.)*)"')
CHOICES_RE = re.compile(r'choices\(&\[((?:"(?:[^"\\]|\\.)*"\s*,?\s*)*)\]')
STRING_LIT_RE = re.compile(r'"((?:[^"\\]|\\.)*)"')
FIELD_TABLE_FILES = {
    "kaku/src/config_tui/mod.rs",
    "kaku/src/ai_config/tui.rs",
    "kaku/src/ai_config/tui/providers/assistant.rs",
}


def strip_tests(source: str) -> str:
    """Cut everything from the first ``mod tests`` block onwards.

    Test-only strings would pollute the report with keys no user surface
    renders. Files whose tests live before other code do not exist in this
    repo's layout (tests are last); if one appears, its pre-test strings are
    still collected and its test strings are simply part of the tail.
    """
    marker = source.find("mod tests")
    source = source if marker < 0 else source[:marker]
    # Drop line comments: they quote example keys ("full key: \"provider/...\"
    # in a comment) that no render path ever looks up.
    return re.sub(r"//[^\n]*", "", source)


def collect() -> set[str]:
    keys: set[str] = set()
    for base in SCAN_DIRS:
        for path in base.rglob("*.rs"):
            posix = path.as_posix()
            if any(part in posix for part in SKIP_PARTS):
                continue
            source = strip_tests(path.read_text(encoding="utf-8"))
            keys.update(TRFN_RE.findall(source))
            keys.update(BRIEF_RE.findall(source))
            keys.update(FOOTER_LABEL_RE.findall(source))
            if "commands.rs" in posix:
                for group in MENUBAR_RE.findall(source):
                    keys.update(STRING_LIT_RE.findall(group))
            if posix.endswith(tuple(FIELD_TABLE_FILES)):
                keys.update(FIELD_KEY_RE.findall(source))
                for group in CHOICES_RE.findall(source):
                    keys.update(STRING_LIT_RE.findall(group))
            if "termwindow/mod.rs" in posix:
                # Toast tables map event names to English display text.
                start = source.find("fn lookup_kaku_toast")
                end = source.find("lazy_static!")
                if 0 <= start < end:
                    keys.update(TOAST_TABLE_RE.findall(source[start:end]))
            # Key-hint labels: only the tuple arguments of the two shared
            # chrome helpers, not every 2-tuple in the file.
            for call in HINT_CALL_RE.finditer(source):
                chunk = source[call.end() : call.end() + 400]
                keys.update(HINT_RE.findall(chunk))
            keys.update(PREFIX_RE.findall(source))
    # The pack never translates these by rule; pure numbers are values.
    keys.discard("Kaku")
    return {
        k
        for k in keys
        if k and not k.startswith("_") and not re.fullmatch(r"[\d.]+", k)
    }


def main() -> int:
    pack_path = Path(sys.argv[1]) if len(sys.argv) > 1 else Path.home() / ".config/kaku/lang/zh.json"
    keys = sorted(collect())
    if not pack_path.exists():
        print(f"pack not found: {pack_path}; collected {len(keys)} translatable keys")
        for key in keys:
            print(f"  {key}")
        return 0
    pack = json.loads(pack_path.read_text(encoding="utf-8"))
    missing = [k for k in keys if k not in pack]
    orphans = sorted(k for k in pack if k not in set(keys) and not k.startswith("_"))
    print(f"{len(keys)} translatable keys, pack {pack_path}")
    if missing:
        print(f"\nmissing ({len(missing)}) — rendered as English today:")
        for key in missing:
            print(f"  {key}")
    if orphans:
        print(f"\norphans ({len(orphans)}) — no longer in code, safe to delete or keep:")
        for key in orphans:
            print(f"  {key}")
    if not missing and not orphans:
        print("pack is complete")
    return 1 if missing else 0


if __name__ == "__main__":
    sys.exit(main())
