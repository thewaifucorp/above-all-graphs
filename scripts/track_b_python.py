#!/usr/bin/env python3
"""Track B, entity extraction, measured against an independent oracle.

The evaluation contract asks for entity precision and recall per type, and
warns that ground truth authored by whoever wrote the extractor proves
nothing. This script sidesteps that by not authoring ground truth at all: it
uses CPython's own ``ast`` module as the oracle. That parser was written by
someone else, for another purpose, and is the definition of what a Python
function or class is.

It measures one language and one thing — did the engine find the declarations
that exist — which is a slice of Track B, not Track B. Relationship precision,
resolution ambiguity, contract matching, and impact accuracy all still need
ground truth nobody here can supply honestly.

Usage:

    python3 scripts/track_b_python.py <repo> <graph.db> [--json out.json]

The graph database must have been produced by indexing that same repository:

    aag bigbang --path <repo> --no-viz --no-install
"""

from __future__ import annotations

import argparse
import ast
import json
import pathlib
import sqlite3
import subprocess
import sys

# Engine node kinds that correspond to a Python declaration. `struct` is what
# the engine calls a class-like declaration across every language.
FUNCTION_KINDS = {"function", "method"}
CLASS_KINDS = {"struct"}


def tracked_python_files(repo: pathlib.Path) -> list[str]:
    """Python files git tracks — the same set the engine walks."""
    result = subprocess.run(
        ["git", "ls-files", "*.py"],
        cwd=repo,
        capture_output=True,
        text=True,
        check=False,
    )
    return [line for line in result.stdout.splitlines() if line.strip()]


def oracle(repo: pathlib.Path, files: list[str]) -> tuple[set, set, list[str]]:
    """Declarations CPython itself finds, as (file, name) pairs.

    Returns functions, classes, and the files that could not be parsed — a
    syntax error or a Python 2 file is excluded from both sides rather than
    counted as a miss, because the engine is not being asked to beat CPython
    at parsing files CPython rejects.
    """
    functions: set[tuple[str, str]] = set()
    classes: set[tuple[str, str]] = set()
    unparsable: list[str] = []
    for relative in files:
        path = repo / relative
        try:
            source = path.read_text(encoding="utf-8", errors="strict")
            tree = ast.parse(source)
        except (OSError, SyntaxError, UnicodeDecodeError):
            unparsable.append(relative)
            continue
        for node in ast.walk(tree):
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
                functions.add((relative, node.name))
            elif isinstance(node, ast.ClassDef):
                classes.add((relative, node.name))
    return functions, classes, unparsable


def engine(database: pathlib.Path, files: set[str]) -> tuple[set, set]:
    """Declarations the engine recorded for the same files."""
    connection = sqlite3.connect(str(database))
    functions: set[tuple[str, str]] = set()
    classes: set[tuple[str, str]] = set()
    for kind, name, file_path in connection.execute(
        "SELECT kind, name, file_path FROM nodes WHERE file_path LIKE '%.py'"
    ):
        if file_path not in files:
            continue
        if kind in FUNCTION_KINDS:
            functions.add((file_path, name))
        elif kind in CLASS_KINDS:
            classes.add((file_path, name))
    connection.close()
    return functions, classes


def score(truth: set, found: set) -> dict:
    """Precision, recall, and F1, with the raw counts that produced them."""
    true_positives = len(truth & found)
    false_positives = len(found - truth)
    false_negatives = len(truth - found)
    precision = true_positives / (true_positives + false_positives) if found else 0.0
    recall = true_positives / (true_positives + false_negatives) if truth else 0.0
    f1 = (
        2 * precision * recall / (precision + recall)
        if precision + recall > 0
        else 0.0
    )
    return {
        "truth": len(truth),
        "found": len(found),
        "true_positives": true_positives,
        "false_positives": false_positives,
        "false_negatives": false_negatives,
        "precision": round(precision, 4),
        "recall": round(recall, 4),
        "f1": round(f1, 4),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("repo", type=pathlib.Path)
    parser.add_argument("database", type=pathlib.Path)
    parser.add_argument("--json", type=pathlib.Path, default=None)
    parser.add_argument(
        "--examples",
        type=int,
        default=5,
        help="how many misses of each kind to print",
    )
    arguments = parser.parse_args()

    files = tracked_python_files(arguments.repo)
    if not files:
        print(f"no tracked Python files in {arguments.repo}", file=sys.stderr)
        return 1

    truth_functions, truth_classes, unparsable = oracle(arguments.repo, files)
    measured = set(files) - set(unparsable)
    found_functions, found_classes = engine(arguments.database, measured)

    report = {
        "track": "B: engine extraction (entities, Python only)",
        "oracle": "CPython ast",
        "repository": arguments.repo.resolve().name,
        "revision": subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=arguments.repo,
            capture_output=True,
            text=True,
            check=False,
        ).stdout.strip(),
        "python_files_tracked": len(files),
        "python_files_measured": len(measured),
        "python_files_unparsable_by_oracle": len(unparsable),
        "functions": score(truth_functions, found_functions),
        "classes": score(truth_classes, found_classes),
    }
    report["all_entities"] = score(
        truth_functions | truth_classes, found_functions | found_classes
    )

    print(json.dumps(report, indent=2))
    for label, truth, found in (
        ("function", truth_functions, found_functions),
        ("class", truth_classes, found_classes),
    ):
        missed = sorted(truth - found)[: arguments.examples]
        spurious = sorted(found - truth)[: arguments.examples]
        for file_path, name in missed:
            print(f"missed {label}: {file_path}:{name}", file=sys.stderr)
        for file_path, name in spurious:
            print(f"extra  {label}: {file_path}:{name}", file=sys.stderr)

    if arguments.json:
        arguments.json.parent.mkdir(parents=True, exist_ok=True)
        arguments.json.write_text(json.dumps(report, indent=2) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
