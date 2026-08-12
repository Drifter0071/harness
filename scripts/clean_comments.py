"""Replace all non-ASCII characters in .rs files with ASCII equivalents.

The mojibake in commands.rs / models.rs / etc. is too corrupted to
reverse-engineer. We just remove all non-ASCII characters and let
the comments look slightly less pretty but functional.
"""
import re
from pathlib import Path

# Map of common non-ASCII chars to ASCII equivalents (or just remove)
REPLACEMENTS = {
    "─": "-",  # box drawings light horizontal
    "│": "|",  # box drawings light vertical
    "—": "--",  # em-dash
    "–": "-",  # en-dash
    "→": "->",  # rightwards arrow
    "├": "+",  # box drawings light vertical and right
    "└": "+",  # box drawings up and right
    "┌": "+",  # box drawings down and right
    "┐": "+",  # box drawings down and left
    "┘": "+",  # box drawings up and left
    "┴": "+",  # box drawings left and up
    "┬": "+",  # box drawings right and up
    "┼": "+",  # box drawings light vertical and horizontal
    "“": '"',  # left double quotation mark
    "”": '"',  # right double quotation mark
    "‘": "'",  # left single quotation mark
    "’": "'",  # right single quotation mark
    "…": "...",  # horizontal ellipsis
    "—": "--",  # another em-dash
    "\u00b7": "*",  # middle dot
    "\u00a0": " ",  # non-breaking space
    "─": "-",
    "ä": "a",  # random mojibake
    "ö": "o",
    "ü": "u",
    "é": "e",
}

paths = [
    r"C:\Users\garvangel\Documents\GitHub\harness\src-tauri\src\commands.rs",
    r"C:\Users\garvangel\Documents\GitHub\harness\src-tauri\src\models.rs",
    r"C:\Users\garvangel\Documents\GitHub\harness\src-tauri\src\lib.rs",
    r"C:\Users\garvangel\Documents\GitHub\harness\src-tauri\src\secrets.rs",
    r"C:\Users\garvangel\Documents\GitHub\harness\src-tauri\src\session.rs",
    r"C:\Users\garvangel\Documents\GitHub\harness\src-tauri\src\web_search.rs",
    r"C:\Users\garvangel\Documents\GitHub\harness\src-tauri\src\agent.rs",
    r"C:\Users\garvangel\Documents\GitHub\harness\src-tauri\src\clientx.rs",
    r"C:\Users\garvangel\Documents\GitHub\harness\src-tauri\src\main.rs",
]

for p in paths:
    path = Path(p)
    if not path.exists():
        print(f"SKIP: {p}")
        continue
    raw = path.read_bytes()
    if raw.startswith(b"\xef\xbb\xbf"):
        raw = raw[3:]
    # Read as UTF-8 (with replacement for bad bytes), apply
    # replacements, then write back as UTF-8 no BOM.
    text = raw.decode("utf-8", errors="replace")
    for bad, good in REPLACEMENTS.items():
        text = text.replace(bad, good)
    # Strip any remaining non-ASCII chars
    text = "".join(c if ord(c) < 128 else "?" for c in text)
    path.write_bytes(text.encode("ascii"))
    print(f"FIXED: {p} ({len(text)} chars)")
