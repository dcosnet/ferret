#!/usr/bin/env python3
"""Audit Rust bracket balance with a proper tokenizer.

Handles:
- Line comments (//...)
- Block comments (/* ... */)
- String literals ("..." with escapes)
- Raw string literals (r"...", r#"..."#)
- Char literals ('...' with escapes)
- Lifetime parameters ('ident)

Reports any file where (), {}, or [] are unbalanced.
"""
import sys
from pathlib import Path


def audit(path: Path) -> list[str]:
    text = path.read_text()
    issues = []
    stack: list[tuple[str, int, int]] = []  # (char, line, col)
    line, col = 1, 0
    i = 0
    n = len(text)

    openers = {'(': ')', '{': '}', '[': ']'}
    closers = set(openers.values())

    while i < n:
        c = text[i]
        if c == '\n':
            line += 1
            col = 0
            i += 1
            continue
        col += 1

        # Line comment
        if c == '/' and i + 1 < n and text[i + 1] == '/':
            while i < n and text[i] != '\n':
                i += 1
            continue
        # Block comment
        if c == '/' and i + 1 < n and text[i + 1] == '*':
            i += 2
            while i + 1 < n and not (text[i] == '*' and text[i + 1] == '/'):
                if text[i] == '\n':
                    line += 1
                    col = 0
                else:
                    col += 1
                i += 1
            i += 2
            continue
        # Raw string r"..." or r#"..."#
        if c == 'r' and i + 1 < n and text[i + 1] == '"':
            j = i + 2
            hashes = 0
            while j < n and text[j] == '#':
                hashes += 1
                j += 1
            if j < n and text[j] == '"':
                close = '"' + '#' * hashes
                start = j + 1
                end = text.find(close, start)
                if end == -1:
                    issues.append(f"{path}:{line}: unterminated raw string")
                    return issues
                for ch in text[i:end + len(close)]:
                    if ch == '\n':
                        line += 1
                        col = 0
                    else:
                        col += 1
                i = end + len(close)
                continue
        # Regular string
        if c == '"':
            j = i + 1
            while j < n:
                if text[j] == '\\' and j + 1 < n:
                    j += 2
                    continue
                if text[j] == '"':
                    break
                j += 1
            if j >= n:
                issues.append(f"{path}:{line}: unterminated string")
                return issues
            i = j + 1
            continue
        # Char literal or lifetime
        if c == "'":
            if i + 1 < n and (text[i + 1].isalpha() or text[i + 1] == '_'):
                j = i + 1
                while j < n and (text[j].isalnum() or text[j] == '_'):
                    j += 1
                if j < n and text[j] != "'":
                    i = j
                    continue
            j = i + 1
            while j < n:
                if text[j] == '\\' and j + 1 < n:
                    j += 2
                    continue
                if text[j] == "'":
                    break
                j += 1
            if j >= n:
                i += 1
                continue
            i = j + 1
            continue
        if c in openers:
            stack.append((c, line, col))
            i += 1
            continue
        if c in closers:
            if not stack:
                issues.append(f"{path}:{line}:{col}: unexpected '{c}'")
                i += 1
                continue
            top, tline, tcol = stack.pop()
            if openers[top] != c:
                issues.append(
                    f"{path}:{line}:{col}: '{c}' does not match '{top}' "
                    f"opened at {tline}:{tcol}"
                )
            i += 1
            continue
        i += 1

    for top, tline, tcol in stack:
        issues.append(f"{path}: unclosed '{top}' opened at {tline}:{tcol}")
    return issues


def main():
    root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path('.')
    rs_files = sorted(root.rglob('*.rs'))
    total_issues = 0
    for f in rs_files:
        issues = audit(f)
        for iss in issues:
            print(iss)
            total_issues += 1
    print(f"\n{total_issues} issue(s) across {len(rs_files)} files")


if __name__ == '__main__':
    main()
