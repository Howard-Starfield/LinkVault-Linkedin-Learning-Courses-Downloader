"""Bump version 0.2.10 -> 0.2.11 in all four version-bearing files."""
import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]

TARGETS = [
    (
        REPO / "apps" / "desktop" / "package.json",
        re.compile(r'"version":\s*"0\.2\.10"'),
        '"version": "0.2.11"',
    ),
    (
        REPO / "apps" / "desktop" / "src-tauri" / "Cargo.toml",
        re.compile(r'^version = "0\.2\.10"', re.M),
        'version = "0.2.11"',
    ),
    (
        REPO / "apps" / "desktop" / "src-tauri" / "tauri.conf.json",
        re.compile(r'"version":\s*"0\.2\.10"'),
        '"version": "0.2.11"',
    ),
    (
        REPO / "apps" / "desktop" / "src" / "App.tsx",
        re.compile(r'const APP_VERSION = "0\.2\.10"'),
        'const APP_VERSION = "0.2.11"',
    ),
]


def main() -> int:
    rc = 0
    for path, pattern, replacement in TARGETS:
        text = path.read_text(encoding="utf-8")
        new_text, n = pattern.subn(replacement, text, count=1)
        if n == 0:
            print(f"  MISS  {path.relative_to(REPO)}  (pattern did not match)")
            rc = 1
            continue
        if new_text == text:
            print(f"  SAME  {path.relative_to(REPO)}")
            continue
        path.write_text(new_text, encoding="utf-8", newline="")
        print(f"  OK    {path.relative_to(REPO)}  ({n} substitution)")
    return rc


if __name__ == "__main__":
    sys.exit(main())
