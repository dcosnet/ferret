#!/usr/bin/env python3
"""Audit Rust match arms for missing `*` dereferences.

When a function takes `&Cmd` and matches on it, every captured field is
a reference. Forwarding that reference to a context expecting a value
(without `*`) is a compile error. This script finds such cases by:

  1. Finding functions whose signature takes `&EnumType` parameters.
  2. Finding `match <param> { Enum::Variant(capture) => body }` blocks.
  3. Checking if `capture` appears in `body` without a leading `*`
     in a value-expecting position (function arg, struct field, etc.).

Conservative: flags potential issues for manual review.
"""
import re
import sys
from pathlib import Path


def find_ref_params(text: str) -> dict[str, str]:
    """Find function params of form `name: &EnumType`. Returns {param: type}."""
    params = {}
    # fn foo(... name: &SomeType, ...)
    for m in re.finditer(r'(\w+)\s*:\s*&(\w+)', text):
        param_name = m.group(1)
        type_name = m.group(2)
        # Heuristic: only flag types that start with uppercase (enums/structs)
        if type_name[0].isupper():
            params[param_name] = type_name
    return params


def find_match_on_param(text: str, param: str) -> list[tuple[int, str, list[str]]]:
    """Find `match <param> { ... }` blocks and extract arm captures.

    Returns list of (line, variant, [captures]).
    """
    results = []
    # Find `match <param> {` — then scan arms until matching `}`
    for m in re.finditer(rf'match\s+{re.escape(param)}\s*\{{', text):
        block_start = m.end()
        # Find matching close brace (naive depth counting)
        depth = 1
        i = block_start
        while i < len(text) and depth > 0:
            if text[i] == '{':
                depth += 1
            elif text[i] == '}':
                depth -= 1
            i += 1
        block_end = i
        block = text[block_start:block_end]

        # Find arm patterns: Enum::Variant(captures) =>
        for arm in re.finditer(
            r'(\w+)::(\w+)\s*\(([^)]*)\)\s*=>',
            block,
        ):
            arm_line = text[:block_start + arm.start()].count('\n') + 1
            variant = f"{arm.group(1)}::{arm.group(2)}"
            captures_raw = arm.group(3).strip()
            if not captures_raw:
                continue
            captures = []
            for c in captures_raw.split(','):
                c = c.strip()
                # Strip type annotations: `deg: u16` -> `deg`
                if ':' in c:
                    c = c.split(':')[0].strip()
                if c and (c[0].isalpha() or c[0] == '_'):
                    captures.append(c)
            if captures:
                results.append((arm_line, variant, captures))
    return results


def check_deref_in_arm(text: str, arm_line: int, ident: str) -> bool:
    """Check if `ident` is used without `*` deref in the arm body (next ~15 lines).

    Returns True if a potential issue is found.
    """
    lines = text.split('\n')
    # Scan from arm_line+1 for up to 15 lines
    for offset in range(1, 16):
        idx = arm_line - 1 + offset  # 0-indexed
        if idx >= len(lines):
            break
        line = lines[idx]
        # Stop at next arm (line containing `=>` at start, or `}` ending block)
        stripped = line.strip()
        if stripped.startswith('}') or stripped.startswith('_') and '=>' in stripped:
            break
        # Find usages of `ident` not preceded by `*` or `&` or `.`
        for m in re.finditer(rf'(?<![*.&>])\b{re.escape(ident)}\b', line):
            # Skip if followed by `.` (method/field access — fine on references)
            after_idx = m.end()
            if after_idx < len(line) and line[after_idx] == '.':
                continue
            # Skip if it's in a closure binding: |ident|
            before = line[:m.start()]
            if before.endswith('|'):
                continue
            # Skip if it's a pattern match itself: Some(ident)
            # (preceded by `(` or `,` and followed by `)` or `,`)
            if before.endswith('(') or before.endswith(','):
                after = line[after_idx:] if after_idx < len(line) else ''
                if after.startswith(')') or after.startswith(','):
                    continue
            # Skip if explicitly dereferenced with `*`
            if before.endswith('*'):
                continue
            # Found a usage that might need `*`
            return True
    return False


def audit_file(path: Path) -> list[str]:
    text = path.read_text()
    issues = []
    ref_params = find_ref_params(text)
    for param, type_name in ref_params.items():
        arms = find_match_on_param(text, param)
        for arm_line, variant, captures in arms:
            for ident in captures:
                if ident.startswith('_'):
                    continue
                if check_deref_in_arm(text, arm_line, ident):
                    issues.append(
                        f"  {path}:{arm_line}: `{ident}` captured from "
                        f"`{variant}` (matching `&{type_name} {param}`) — "
                        f"verify `*{ident}` is used where a value is expected"
                    )
    return issues


def main():
    root = Path(sys.argv[1]) if len(sys.argv) > 1 else Path('.')
    rs_files = sorted(root.rglob('*.rs'))
    total_issues = 0
    for f in rs_files:
        issues = audit_file(f)
        if issues:
            print(f"\n{f}:")
            for iss in issues:
                print(iss)
                total_issues += 1
    print(f"\n{total_issues} potential issue(s) across {len(rs_files)} files")
    print("(Each flag requires manual review — this is a heuristic.)")


if __name__ == '__main__':
    main()
