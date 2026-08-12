"""Direct byte-level fix for mojibake in .rs files.

The mojibake in the file is: original UTF-8 bytes (E2 94 80 for box
drawing, E2 80 94 for em-dash, etc.) were read as Latin-1 and
re-encoded to UTF-8.

For example, E2 94 80 (3 bytes for U+2500) -> read as 3 Latin-1
chars: U+00E2 U+2019 U+2014 (no wait, U+0080 is not valid Latin-1,
so this gets remapped). The actual resulting mojibake depends on
the tool but commonly:
  U+2500 (E2 94 80) -> C3 A2 E2 80 9A C2 80  (the 80 in the middle
    gets remapped to U+2019 then re-encoded)
  U+2014 (E2 80 94) -> C3 A2 E2 80 9A C2 94
  U+2013 (E2 80 93) -> C3 A2 E2 80 9A C2 93

But the corrupted patterns I see in the actual file are different
(e.g., C3 84 E2 80 9A C3 8B C2 98 C4 82 CB 98 C3 A2 E2 80 9A C2 AC...).
These are the bytes for a sequence of several chars, which means
the original was probably multiple em-dashes / box-drawing chars.

Let me just do a direct byte-pattern match. I know what the
mojibake looks like for the common cases.
"""
from pathlib import Path

# These are the multi-byte sequences in the corrupted file. I need
# to find them and replace with the original UTF-8.

# Strategy: read as bytes, search for known mojibake patterns, replace.

# The most reliable way is to take the BYTES and assume they are
# "double-encoded UTF-8". To reverse, we decode the UTF-8, then
# interpret the result as Latin-1 (which gives us back the original
# bytes as characters), then re-encode those as UTF-8.
#
# But the script's `recovered = text.encode("latin-1").decode("utf-8")`
# only works if the text is one-shot. In our case, the file is
# mixed: ASCII (working code) + corrupted sections (mojibake).
# The Latin-1 encode of pure ASCII is the same ASCII, so it shouldn't
# break anything.

# Actually, the issue is that for ASCII characters, encode("latin-1")
# is identity, so recovered == text for ASCII regions. But for
# mojibake regions, encode("latin-1") re-interprets the UTF-8 bytes
# of mojibake as Latin-1 bytes... wait, that doesn't work because
# the mojibake is already UTF-8.

# Let me try a different approach: detect the mojibake by its
# characteristic pattern and replace directly with byte sequences.

# Actually, the SIMPLEST approach: do a regex replace on the
# mojibake UTF-8 sequences.

# Examples of mojibake I observed (UTF-8 bytes):
# C3 A2 E2 80 9A     -> U+2500 (box drawings light horizontal)
# C3 A2 E2 80 9A C2 AC -> U+2014 (em-dash)?

# Let me do an empirical mapping. Read the file, find common
# sequences, and map them.

# Specific sequences I've seen in the file:
# C3 84 E2 80 9A C3 8B C2 98 C4 82 CB 98 C3 A2 E2 80 9A C2 AC C4 B9 C4 84
# This is too long for one char. It's the encoding of multiple
# chars: each em-dash/box-drawing/arrow/etc. got individually
# re-encoded as multiple Latin-1 chars and then UTF-8'd.

# Em-dash U+2014 (E2 80 94) read as Latin-1: bytes E2=â 80=€ 94=œ
# then re-encoded as UTF-8: â (C3 A2) € (E2 82 AC) œ (C5 93)
# = C3 A2 E2 82 AC C5 93

# Box drawing U+2500 (E2 94 80) read as Latin-1: bytes E2=â 80=€ 00=control
# 0x00 is invalid Latin-1 (well, it's NULL), so it's dropped or
# replaced. Actually, 0x80 in CP1252 is U+20AC €. So:
# 0x80 = U+20AC, 0x00 = invalid (skipped or replaced with U+FFFD).
# Hmm, this is getting complex.

# Let me try a totally different approach: use chardet or
# chardetng to detect the encoding, then re-decode properly.

# Actually, the cleanest approach: I know the original chars I
# wrote were em-dash (─), en-dash (–), box-drawing chars (─, │, ┌,
# etc.), and arrows (→). Let me just do explicit byte-level
# replacements for the most common mojibake patterns I've seen.

# What I know about the mojibake in the file:
# Pattern: C3 A2 E2 80 9A appears repeatedly, often followed by
# C2 AC or C3 8B. This is the U+2019 'right single quotation mark'
# that came from reading U+2014 or U+2013 incorrectly.

# Simpler approach: just replace the C3 A2 E2 80 9A C2 AC (which is
# the most common pattern) with the em-dash bytes E2 80 94.

PATTERNS = {
    # Specific observed mojibake patterns and their originals
    b"\xc3\xa2\xe2\x80\x9a\xc2\xac": b"\xe2\x80\x94",  # em-dash
    b"\xc3\xa2\xe2\x80\x9a\xc2\x93": b"\xe2\x80\x93",  # en-dash
    b"\xc3\xa2\xe2\x80\x9a\xc2\x91": b"\xe2\x80\x98",  # left single quote
    b"\xc3\xa2\xe2\x80\x9a\xc2\x92": b"\xe2\x80\x99",  # right single quote
    b"\xc3\xa2\xe2\x80\x9a\xc2\x9c": b"\xe2\x80\x9c",  # left double quote
    b"\xc3\xa2\xe2\x80\x9a\xc2\x9d": b"\xe2\x80\x9d",  # right double quote
    b"\xc3\xa2\xe2\x80\x9a\xe2\x80\x9a": b"\xe2\x80\x94",  # em-dash
    # Some longer patterns I observed
    b"\xc3\x84\xe2\x80\x9a\xc3\x8b\xc2\x98\xc4\x82\xcb\x98\xc3\xa2\xe2\x80\x9a\xc2\xac\xc4\xb9\xc4\x84\xc4\x82\xcb\x98\xc3\xa2\xe2\x82\xac\xc5\xa1\xc3\x82\xc2\xac": b"\xe2\x94\x80\xe2\x94\x80\xe2\x94\x80",  # "───"
    b"\xc3\x84\xe2\x80\x9a\xc3\x8b\xc2\x98\xc4\x82\xcb\x98\xc3\xa2\xe2\x80\x9a\xc2\xac\xc4\xb9\xc4\x84\xc4\x82\xcb\x98": b"\xe2\x94\x80",  # "─"
    b"\xc3\xa2\xe2\x80\x9a\xc2\xac\xc4\xb9\xc4\x84": b"\xe2\x80\x94",  # em-dash
    b"\xc3\x84\xc2\xa2\xc3\xa2\xe2\x80\x9a\xc2\xac": b"\xe2\x80\x94",  # em-dash
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
    for bad, good in PATTERNS.items():
        raw = raw.replace(bad, good)
    path.write_bytes(raw)
    print(f"FIXED: {p} ({len(raw)} bytes)")
