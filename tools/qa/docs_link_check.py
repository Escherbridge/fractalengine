#!/usr/bin/env python3
"""Docs link check (spec R26): docs/extensions/*.md point only at files that exist.

Usage: python tools/qa/docs_link_check.py [--root .]   (exit 1 on any failure)
Rationale and rules: tools/qa/AGENTS.md section docs-link-check.
"""
import argparse
import re
import sys
from pathlib import Path

CHECKED_FENCE_LANGUAGES = {"rust", "toml", "json", "wit"}
EXAMPLE_ROOTS = ("extensions/", "tools/fe-ext/guest/", "fe-sdk/schema/examples/", "fe-plugin/wit/")
FENCE_PROXIMITY_LINES = 3

FENCE_OPEN = re.compile(r"^\s*(`{3,}|~{3,})\s*([A-Za-z0-9_+-]*)")
MARKDOWN_LINK = re.compile(r"\[[^\]]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
BACKTICK_SPAN = re.compile(r"`([^`\n]+)`")
EXTENSION_SUFFIX = re.compile(r"\.[A-Za-z0-9]{1,8}$")
NOT_A_PATH_CHARS = re.compile(r"[\s*<>{}$|()=,;:\\]")


def looks_like_repo_file(text):
    """True for a backticked token shaped like a repo-relative file path."""
    if "/" not in text or NOT_A_PATH_CHARS.search(text) or "://" in text or "..." in text:
        return False
    return bool(EXTENSION_SUFFIX.search(text.rstrip("/"))) and not text.endswith("/")


def resolve_existing(target, doc_path, root):
    """Return the existing file a link or path names (doc-relative first, then repo-root), else None."""
    target = target.split("#", 1)[0].split("?", 1)[0]
    if not target:
        return None
    candidates = [(doc_path.parent / target)]
    candidates.append(root / target.lstrip("/"))
    for candidate in candidates:
        try:
            resolved = candidate.resolve()
        except OSError:
            continue
        if resolved.is_file():
            return resolved
    return None


def is_example_file(resolved, root):
    try:
        relative = resolved.relative_to(root.resolve()).as_posix()
    except ValueError:
        return False
    return relative.startswith(EXAMPLE_ROOTS)


def is_external_or_anchor(target):
    return target.startswith(("#", "http://", "https://", "mailto:", "tel:")) or not target.strip()


def references_in_line(line):
    """Yield (kind, target) for every markdown link and file-shaped backtick span on a line."""
    for match in MARKDOWN_LINK.finditer(line):
        yield "link", match.group(1)
    without_links = MARKDOWN_LINK.sub(lambda m: m.group(0).split("](")[0], line)
    for match in BACKTICK_SPAN.finditer(without_links):
        token = match.group(1).strip()
        if looks_like_repo_file(token):
            yield "path", token


def parse_blocks(lines):
    """Split lines into prose line indexes and fenced blocks (start, end, language)."""
    prose, blocks = [], []
    open_marker, open_language, open_start = None, "", 0
    for index, line in enumerate(lines):
        fence = FENCE_OPEN.match(line)
        if open_marker is None:
            if fence:
                open_marker, open_language, open_start = fence.group(1)[0], fence.group(2).lower(), index
            else:
                prose.append(index)
        elif fence and fence.group(1)[0] == open_marker and not fence.group(2):
            blocks.append((open_start, index, open_language))
            open_marker = None
    if open_marker is not None:
        blocks.append((open_start, len(lines) - 1, open_language))
    return prose, blocks


def check_document(doc_path, root):
    failures = []
    relative_doc = doc_path.relative_to(root).as_posix()
    lines = doc_path.read_text(encoding="utf-8").splitlines()
    prose, blocks = parse_blocks(lines)

    for index in prose:
        for kind, target in references_in_line(lines[index]):
            if kind == "link" and is_external_or_anchor(target):
                continue
            if resolve_existing(target, doc_path, root) is None:
                failures.append(f"{relative_doc}:{index + 1}: {kind} target does not exist: {target}")

    for start, end, language in blocks:
        if language not in CHECKED_FENCE_LANGUAGES:
            continue
        window = list(range(max(0, start - FENCE_PROXIMITY_LINES), start))
        window += list(range(end + 1, min(len(lines), end + 1 + FENCE_PROXIMITY_LINES)))
        if not any(line_names_example_file(lines[i], doc_path, root) for i in window):
            failures.append(
                f"{relative_doc}:{start + 1}: {language} code block has no link to an existing example file "
                f"({', '.join(EXAMPLE_ROOTS)}) within {FENCE_PROXIMITY_LINES} lines"
            )
    return failures


def line_names_example_file(line, doc_path, root):
    for kind, target in references_in_line(line):
        if kind == "link" and is_external_or_anchor(target):
            continue
        resolved = resolve_existing(target, doc_path, root)
        if resolved is not None and is_example_file(resolved, root):
            return True
    return False


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--root", default=".", help="repository root (default: current directory)")
    arguments = parser.parse_args()
    root = Path(arguments.root).resolve()
    docs_directory = root / "docs" / "extensions"
    documents = sorted(docs_directory.glob("*.md"))
    if not documents:
        print(f"{docs_directory.as_posix()}:0: no docs/extensions/*.md found")
        return 1
    failures = []
    for document in documents:
        failures.extend(check_document(document, root))
    for failure in failures:
        print(failure)
    print(f"docs_link_check: {len(documents)} documents, {len(failures)} failures")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
