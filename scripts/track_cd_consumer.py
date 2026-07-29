#!/usr/bin/env python3
"""Tracks C and D: does the graph make a consumer better, and what does it cost.

The contract asks for a comparison that holds the consumer model and task
constant while varying what the consumer is given, and for the cost of each
condition to be recorded rather than estimated. That is what this does.

**Tasks come from an oracle, not from an author.** Every question and every
answer is generated mechanically from CPython's own ``ast`` over an external
repository, so no one involved in writing the engine wrote the answers. A task
is only kept when its answer is unambiguous (exactly one definition site, at
least one caller, and so on).

**Conditions** vary the producer while the consumer model stays fixed:

``none``
    The question alone. The floor: what the model already knows or can guess.
``aag``
    The question plus one ``aag explore`` slice for the symbol. No repository
    access, no tools.
``reference``
    The question plus a manifest slice built by a deterministic reference
    producer — the CPython oracle itself, rendered as context. The ceiling: what
    a perfect producer would hand a consumer.
``llm``
    The question plus a manifest slice an LLM produced from the repository in a
    separate call. The contract's LLM-only producer, at task granularity.
``raw``
    The question plus read access to the repository (``Read``/``Grep``/
    ``Glob``), no graph. What an agent does today without this project.

**Costs are measured, not modeled.** The consumer CLI reports input, output,
and cache tokens plus a dollar figure per call; every one is recorded per task
per condition, which is Track D.

Usage:

    python3 scripts/track_cd_consumer.py <repo> <graph.db> --tasks 8 \\
        --model claude-sonnet-5 --json bench/empirical/track-cd.json

Nothing here is free: each task runs one model call per condition.
"""

from __future__ import annotations

import argparse
import ast
import json
import pathlib
import re
import statistics
import subprocess
import sys
import tempfile
import time
from collections import defaultdict

CONDITIONS = ("none", "reference", "aag", "llm", "raw")


# --------------------------------------------------------------------------
# Oracle: tasks and answers, from CPython's parser
# --------------------------------------------------------------------------


def python_files(repo: pathlib.Path) -> list[str]:
    result = subprocess.run(
        ["git", "ls-files", "*.py"], cwd=repo, capture_output=True, text=True, check=False
    )
    return [line for line in result.stdout.splitlines() if line.strip()]


class _Attribution(ast.NodeVisitor):
    """Attributes each call to the *nearest enclosing function*, and to nothing
    else.

    The first version of this walked every function *and every class* and
    attributed a call to all of them, so a call inside a method counted as a
    call by the method and by its class. That inflated every answer key by a
    phantom caller and capped the measured F1 near 0.67 no matter which
    producer was under test — it made a correct answer look two-thirds right.
    A benchmark that punishes the right answer is worse than no benchmark.
    """

    def __init__(self) -> None:
        self.definitions: dict[str, set[str]] = defaultdict(set)
        self.callers: dict[str, set[str]] = defaultdict(set)
        self.file = ""
        self.stack: list[str] = []

    def visit_ClassDef(self, node: ast.ClassDef) -> None:  # noqa: N802
        self.definitions[node.name].add(self.file)
        # A class body is not a caller. Its methods are.
        self.generic_visit(node)

    def _function(self, node) -> None:
        self.definitions[node.name].add(self.file)
        self.stack.append(node.name)
        self.generic_visit(node)
        self.stack.pop()

    visit_FunctionDef = _function  # noqa: N815
    visit_AsyncFunctionDef = _function  # noqa: N815

    def visit_Call(self, node: ast.Call) -> None:  # noqa: N802
        target = node.func
        called = (
            target.id
            if isinstance(target, ast.Name)
            else target.attr
            if isinstance(target, ast.Attribute)
            else None
        )
        if called and self.stack and called != self.stack[-1]:
            self.callers[called].add(self.stack[-1])
        self.generic_visit(node)


def index_repository(repo: pathlib.Path) -> tuple[dict, dict]:
    """`name -> files defining it` and `name -> nearest-enclosing callers`."""
    visitor = _Attribution()
    for relative in python_files(repo):
        try:
            tree = ast.parse((repo / relative).read_text(encoding="utf-8"))
        except (OSError, SyntaxError, UnicodeDecodeError):
            continue
        visitor.file = relative
        visitor.visit(tree)
    return visitor.definitions, visitor.callers


def build_tasks(repo: pathlib.Path, wanted: int, min_callers: int = 5) -> list[dict]:
    """Unambiguous "who calls X" tasks, deterministic in selection order.

    A symbol qualifies when exactly one file defines it and it has at least
    `min_callers` distinct ones. Two callers turned out to be no test at all —
    every producer scored 1.000 — so the default asks for five, which is where
    the conditions start to separate.
    """
    definitions, callers = index_repository(repo)
    tasks = []
    for name in sorted(callers):
        sites = definitions.get(name, set())
        who = sorted(callers[name])
        if len(sites) != 1 or not min_callers <= len(who) <= 15 or len(name) < 5:
            continue
        tasks.append(
            {
                "id": f"callers::{name}",
                "symbol": name,
                "defined_in": sorted(sites)[0],
                "question": (
                    f"In this repository, which Python functions or methods contain a "
                    f"call to `{name}`? Answer with the names of the calling functions."
                ),
                "answer": who,
            }
        )
        if len(tasks) >= wanted:
            break
    return tasks


# --------------------------------------------------------------------------
# Producers: what the consumer is given
# --------------------------------------------------------------------------

ANSWER_PROTOCOL = (
    "Reply with one line of JSON and nothing else: "
    '{"answer": ["name1", "name2"]}. Use an empty list if you do not know.'
)


def aag_context(binary: pathlib.Path, repo: pathlib.Path, symbol: str) -> str:
    """One `aag explore` slice — the product's own answer surface."""
    result = subprocess.run(
        [str(binary), "explore", symbol, "--path", str(repo)],
        capture_output=True,
        text=True,
        check=False,
        timeout=120,
    )
    return result.stdout[:20000]


def reference_context(repo: pathlib.Path, symbol: str, oracle_callers: list[str]) -> str:
    """What a perfect producer would emit: the oracle's own answer, as a
    manifest slice. It is the ceiling of the comparison, not a competitor."""
    return json.dumps(
        {
            "symbol": symbol,
            "produced_by": "deterministic reference producer (CPython ast)",
            "callers": sorted(oracle_callers),
        },
        indent=2,
    )


def llm_context(repo: pathlib.Path, symbol: str, model: str) -> tuple[str, dict]:
    """An LLM-only producer: a separate model call compiles the slice, and the
    consumer then answers from that slice alone. Two calls, both charged."""
    prompt = (
        f"Compile a short context manifest for the Python symbol `{symbol}` in this "
        f"repository. List every function or method that calls it, with file paths. "
        f"Output plain text under 40 lines. Do not answer any other question."
    )
    produced = run_consumer(prompt, repo, model, ["Read", "Grep", "Glob"], max_turns=8)
    return produced.get("text", ""), produced


# A context-only consumer runs here: an empty directory, so "no repository
# access" is a fact about the filesystem rather than a tool policy the model
# can spend turns arguing with.
EMPTY_WORKSPACE = pathlib.Path(tempfile.mkdtemp(prefix="aag-consumer-empty-"))


def run_consumer(
    prompt: str,
    repo: pathlib.Path,
    model: str,
    tools: list[str] | None,
    max_turns: int | None = None,
) -> dict:
    """One consumer call. Returns the parsed CLI envelope plus the raw text."""
    command = [
        "claude",
        "-p",
        prompt,
        "--output-format",
        "json",
        "--model",
        model,
        "--strict-mcp-config",
    ]
    workspace = repo
    if tools:
        command += ["--allowed-tools", *tools, "--permission-mode", "acceptEdits"]
    else:
        command += ["--disallowed-tools", "Read", "Grep", "Glob", "Bash", "Task", "WebFetch"]
        workspace = EMPTY_WORKSPACE
    # A context-only condition is single-shot by definition. Without the cap a
    # consumer denied its tools retries them for a dozen turns, which is not the
    # condition being measured and costs more than the condition that works.
    if max_turns:
        command += ["--max-turns", str(max_turns)]
    started = time.monotonic()
    completed = subprocess.run(
        command, cwd=workspace, capture_output=True, text=True, check=False, timeout=900
    )
    elapsed_ms = (time.monotonic() - started) * 1000
    try:
        envelope = json.loads(completed.stdout)
    except json.JSONDecodeError:
        return {
            "failure": "consumer failure: unparsable envelope",
            "stderr": completed.stderr[-400:],
            "wall_ms": elapsed_ms,
        }
    usage = envelope.get("usage", {})
    return {
        "text": envelope.get("result", ""),
        "cost_usd": envelope.get("total_cost_usd", 0.0),
        "input_tokens": usage.get("input_tokens", 0),
        "output_tokens": usage.get("output_tokens", 0),
        "cache_read_tokens": usage.get("cache_read_input_tokens", 0),
        "cache_creation_tokens": usage.get("cache_creation_input_tokens", 0),
        "turns": envelope.get("num_turns", 0),
        "wall_ms": elapsed_ms,
        "is_error": envelope.get("is_error", False),
    }


def parse_answer(text: str) -> list[str] | None:
    """The JSON line the protocol asked for, wherever the model put it."""
    for candidate in re.findall(r"\{[^{}]*\"answer\"[^{}]*\}", text or "", re.S):
        try:
            parsed = json.loads(candidate)
        except json.JSONDecodeError:
            continue
        answer = parsed.get("answer")
        if isinstance(answer, list):
            return [str(item) for item in answer]
    return None


def grade(expected: list[str], given: list[str] | None) -> dict:
    """Set F1 over names. An unparsable answer scores zero, not an exception."""
    if given is None:
        return {"precision": 0.0, "recall": 0.0, "f1": 0.0, "invalid_output": True}
    truth, found = set(expected), set(given)
    hits = len(truth & found)
    precision = hits / len(found) if found else 0.0
    recall = hits / len(truth) if truth else 0.0
    f1 = 2 * precision * recall / (precision + recall) if precision + recall else 0.0
    return {
        "precision": round(precision, 4),
        "recall": round(recall, 4),
        "f1": round(f1, 4),
        "invalid_output": False,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("repo", type=pathlib.Path)
    parser.add_argument("--binary", type=pathlib.Path, default=pathlib.Path("target/release/aag"))
    parser.add_argument("--tasks", type=int, default=8)
    parser.add_argument(
        "--min-callers",
        type=int,
        default=5,
        help="smallest answer set a task may have; low values make every "
        "condition score 1.000 and measure nothing",
    )
    parser.add_argument("--model", default="claude-sonnet-5")
    parser.add_argument("--conditions", nargs="+", default=list(CONDITIONS))
    parser.add_argument(
        "--repetitions",
        type=int,
        default=1,
        help="how many times each (task, condition) pair runs — a consumer is "
        "stochastic, so one sample is an anecdote",
    )
    parser.add_argument("--json", type=pathlib.Path, default=None)
    arguments = parser.parse_args()

    repo = arguments.repo.resolve()
    binary = arguments.binary.resolve()
    tasks = build_tasks(repo, arguments.tasks, arguments.min_callers)
    if not tasks:
        print("no unambiguous tasks could be generated", file=sys.stderr)
        return 1
    print(f"{len(tasks)} task(s) from the oracle, {len(arguments.conditions)} condition(s)")

    records = []
    for task in tasks:
        for condition in arguments.conditions:
          for repetition in range(max(1, arguments.repetitions)):
              producer_cost = 0.0
              producer_call = None
              if condition == "none":
                prompt = f"{task['question']}\n\n{ANSWER_PROTOCOL}"
                tools = None
                turn_cap = 6
              elif condition == "reference":
                context = reference_context(repo, task["symbol"], task["answer"])
                prompt = (
                    f"{task['question']}\n\nHere is a reference manifest slice:\n\n"
                    f"```json\n{context}\n```\n\n{ANSWER_PROTOCOL}"
                )
                tools = None
                turn_cap = 6
              elif condition == "llm":
                context, producer_call = llm_context(repo, task["symbol"], arguments.model)
                producer_cost = producer_call.get("cost_usd", 0.0)
                prompt = (
                    f"{task['question']}\n\nHere is a context manifest slice another "
                    f"agent compiled for `{task['symbol']}`:\n\n```\n{context[:20000]}\n```"
                    f"\n\n{ANSWER_PROTOCOL}"
                )
                tools = None
                turn_cap = 1
              elif condition == "aag":
                context = aag_context(binary, repo, task["symbol"])
                prompt = (
                    f"{task['question']}\n\nHere is a code-graph slice for `{task['symbol']}`:\n\n"
                    f"```\n{context}\n```\n\n{ANSWER_PROTOCOL}"
                )
                tools = None
                turn_cap = 6
              else:
                prompt = (
                    f"{task['question']}\n\nThe repository is the current directory. "
                    f"Search it however you like.\n\n{ANSWER_PROTOCOL}"
                )
                tools = ["Read", "Grep", "Glob"]
                turn_cap = 12

              result = run_consumer(prompt, repo, arguments.model, tools, turn_cap)
              if "failure" in result:
                records.append({"task": task["id"], "condition": condition, **result})
                print(f"  {task['id']:<34} {condition:<5} FAILED", file=sys.stderr)
                continue
              if result.get("is_error"):
                # A capped or errored call is a consumer failure, not a wrong
                # answer. Scoring it zero would blame the producer for the
                # harness.
                records.append(
                    {
                        "task": task["id"],
                        "condition": condition,
                        "repetition": repetition,
                        "consumer_model": arguments.model,
                        "failure": "consumer failure: error envelope",
                        **{key: value for key, value in result.items() if key != "text"},
                    }
                )
                print(f"  {task['id']:<34} {condition:<5} CONSUMER ERROR", file=sys.stderr)
                continue
              scored = grade(task["answer"], parse_answer(result.get("text", "")))
              records.append(
                {
                    "task": task["id"],
                    "condition": condition,
                    "repetition": repetition,
                    "consumer_model": arguments.model,
                    "producer_cost_usd": round(producer_cost, 6),
                    "expected": task["answer"],
                    **scored,
                    **{key: value for key, value in result.items() if key != "text"},
                }
              )
              print(
                f"  {task['id']:<34} {condition:<5} f1={scored['f1']:.2f} "
                f"${result['cost_usd']:.4f} {result['wall_ms']/1000:.1f}s"
              )

    summary = {}
    for condition in arguments.conditions:
        rows = [row for row in records if row["condition"] == condition and "f1" in row]
        failures = [row for row in records if row["condition"] == condition and "f1" not in row]
        if not rows:
            continue
        scores = [row["f1"] for row in rows]
        summary[condition] = {
            "samples": len(rows),
            "tasks": len({row["task"] for row in rows}),
            "failures": len(failures),
            "mean_f1": round(statistics.fmean(scores), 4),
            "median_f1": round(statistics.median(scores), 4),
            "stdev_f1": round(statistics.stdev(scores), 4) if len(scores) > 1 else 0.0,
            "exact": sum(1 for row in rows if row["f1"] == 1.0),
            "invalid_outputs": sum(1 for row in rows if row["invalid_output"]),
            "total_cost_usd": round(
                sum(row["cost_usd"] + row.get("producer_cost_usd", 0.0) for row in rows), 4
            ),
            "mean_cost_usd": round(
                sum(row["cost_usd"] + row.get("producer_cost_usd", 0.0) for row in rows)
                / len(rows),
                4,
            ),
            "mean_output_tokens": round(sum(row["output_tokens"] for row in rows) / len(rows), 1),
            "mean_turns": round(sum(row["turns"] for row in rows) / len(rows), 2),
            "mean_wall_s": round(sum(row["wall_ms"] for row in rows) / len(rows) / 1000, 2),
        }

    report = {
        "track": "C: agent utility + D: end-to-end economics",
        "repetitions": max(1, arguments.repetitions),
        "run_kind": "empirical",
        "oracle": "CPython ast",
        "consumer_model": arguments.model,
        "repository": repo.name,
        "revision": subprocess.run(
            ["git", "rev-parse", "HEAD"], cwd=repo, capture_output=True, text=True, check=False
        ).stdout.strip(),
        "tasks": len(tasks),
        "summary": summary,
        "records": records,
    }
    print(json.dumps(summary, indent=2))
    if arguments.json:
        arguments.json.parent.mkdir(parents=True, exist_ok=True)
        arguments.json.write_text(json.dumps(report, indent=2) + "\n")
        print(f"wrote {arguments.json}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
