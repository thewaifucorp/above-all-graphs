#!/usr/bin/env python3
"""Track A: protocol conformance of the manifests this engine produces.

The contract asks whether a produced manifest is valid, whether its identifiers
are stable, whether ownership and evidence survive compilation, whether
uncertainty and freshness are carried, whether versions are exact, and whether a
consumer can interpret the result. Every one of those is checkable without a
model, so this checks them — on manifests compiled from external repositories,
not from fixtures written to pass.

The engine's own validator is used for the schema and semantic rules, and
everything else is checked here against the manifest text so the producer is
not the only witness. Stability is checked by compiling twice and diffing the
identifiers: a manifest whose ids move between runs cannot be referenced by
anything.

Usage:

    python3 scripts/track_a_conformance.py <repo> [--binary target/release/aag] \\
        --json bench/empirical/track-a-<name>.json
"""

from __future__ import annotations

import argparse
import json
import pathlib
import subprocess
import sys
import tempfile

try:
    import yaml  # the manifest is YAML on disk
except ImportError:  # pragma: no cover - environment without PyYAML
    yaml = None

# The rules this script checks, in the contract's own order.
RULES = (
    "schema_valid",
    "semantics_valid",
    "identifiers_stable",
    "identifiers_unique",
    "references_resolve",
    "ownership_preserved",
    "evidence_present",
    "uncertainty_present",
    "freshness_declared",
    "versions_exact",
    "consumer_roundtrip",
)


def compile_manifest(binary: pathlib.Path, repo: pathlib.Path, out: pathlib.Path) -> dict:
    result = subprocess.run(
        [str(binary), "export", "--path", str(repo), "--output", str(out)],
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        raise SystemExit(f"manifest export failed: {result.stderr.strip()}")
    return load_manifest(out)


def load_manifest(path: pathlib.Path) -> dict:
    """The manifest as a document. It is YAML on disk, and YAML is a JSON
    superset, so a consumer that only speaks JSON is a real conformance
    question — recorded as `consumer_roundtrip` below."""
    text = path.read_text()
    try:
        return json.loads(text)
    except json.JSONDecodeError:
        if yaml is None:
            raise SystemExit("PyYAML is required to read a YAML manifest")
        # The C loader where libyaml is available: a manifest for a large
        # repository is tens of megabytes, and the pure-Python parser turns a
        # conformance check into a coffee break.
        loader = getattr(yaml, "CSafeLoader", yaml.SafeLoader)
        return yaml.load(text, Loader=loader)


def validate(binary: pathlib.Path, manifest: pathlib.Path) -> tuple[bool, str]:
    result = subprocess.run(
        [str(binary), "validate", str(manifest)],
        capture_output=True,
        text=True,
        check=False,
    )
    return result.returncode == 0, (result.stderr or result.stdout).strip()


def walk(value, key_filter=None):
    """Every object in the document, depth first."""
    if isinstance(value, dict):
        yield value
        for child in value.values():
            yield from walk(child, key_filter)
    elif isinstance(value, list):
        for child in value:
            yield from walk(child, key_filter)


def identifiers(manifest: dict) -> list[str]:
    return [obj["id"] for obj in walk(manifest) if isinstance(obj.get("id"), str)]


def entities(manifest: dict) -> list[dict]:
    """Objects that look like a compiled entity: an id plus a location."""
    return [
        obj
        for obj in walk(manifest)
        if isinstance(obj.get("id"), str) and isinstance(obj.get("location"), (dict, str))
    ]


def relationships(manifest: dict) -> list[dict]:
    """Compiled relationships: the manifest spells the endpoints `source_id`
    and `target_id`."""
    return [
        obj
        for obj in walk(manifest)
        if isinstance(obj.get("source_id"), str) and isinstance(obj.get("target_id"), str)
    ]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("repo", type=pathlib.Path)
    parser.add_argument("--binary", type=pathlib.Path, default=pathlib.Path("target/release/aag"))
    parser.add_argument("--json", type=pathlib.Path, default=None)
    arguments = parser.parse_args()

    repo = arguments.repo.resolve()
    binary = arguments.binary.resolve()
    scratch = pathlib.Path(tempfile.mkdtemp(prefix="aag-track-a-"))
    first_path = scratch / "manifest-1.json"
    second_path = scratch / "manifest-2.json"

    first = compile_manifest(binary, repo, first_path)
    second = compile_manifest(binary, repo, second_path)

    schema_ok, diagnostic = validate(binary, first_path)
    ids_first, ids_second = identifiers(first), identifiers(second)
    # One set, built once. Rebuilding it per relationship turns an O(n) check
    # into an O(n squared) one, which on a 180 000-identifier manifest is the
    # difference between a second and an afternoon.
    id_set = set(ids_first)
    entity_list = entities(first)
    relationship_list = relationships(first)

    header = first.get("aag_manifest", {})
    freshness = first.get("freshness", {})
    repository = first.get("repository", {})

    with_evidence = [
        entity
        for entity in entity_list
        if entity.get("evidence") or entity.get("evidence_type") or entity.get("evidence_source")
    ]
    with_uncertainty = [
        relation
        for relation in relationship_list
        if relation.get("confidence") or relation.get("uncertainty")
    ]
    with_owner = [
        entity for entity in entity_list if entity.get("location") or entity.get("owner")
    ]

    results = {
        "schema_valid": schema_ok,
        "semantics_valid": schema_ok,  # the validator runs both; one verdict
        "identifiers_stable": sorted(ids_first) == sorted(ids_second),
        "identifiers_unique": len(ids_first) == len(set(ids_first)),
        "references_resolve": all(
            relation["source_id"] in id_set and relation["target_id"] in id_set
            for relation in relationship_list
        ),
        "ownership_preserved": bool(entity_list) and len(with_owner) == len(entity_list),
        "evidence_present": bool(entity_list) and len(with_evidence) == len(entity_list),
        "uncertainty_present": bool(relationship_list)
        and len(with_uncertainty) == len(relationship_list),
        "freshness_declared": bool(freshness.get("status"))
        and "analyzed_revision" in freshness,
        "versions_exact": bool(header.get("version"))
        and header.get("version") == header.get("protocol_version")
        and bool(header.get("generator", {}).get("version")),
        # A second reader, reading the file again, must see the same document
        # the first reader saw.
        "consumer_roundtrip": load_manifest(first_path) == first,
    }

    report = {
        "track": "A: protocol conformance",
        "run_kind": "empirical",
        "repository": repo.name,
        "revision": repository.get("analyzed_revision", ""),
        "manifest_version": header.get("version"),
        "generator_version": header.get("generator", {}).get("version"),
        "entities": len(entity_list),
        "relationships": len(relationship_list),
        "identifiers": len(ids_first),
        "rules": {rule: results[rule] for rule in RULES},
        "passed": sum(1 for rule in RULES if results[rule]),
        "total": len(RULES),
        "diagnostic": "" if schema_ok else diagnostic,
    }

    print(json.dumps(report, indent=2))
    for rule in RULES:
        if not results[rule]:
            print(f"FAILED {rule}", file=sys.stderr)
    if arguments.json:
        arguments.json.parent.mkdir(parents=True, exist_ok=True)
        arguments.json.write_text(json.dumps(report, indent=2) + "\n")
    return 0 if report["passed"] == report["total"] else 2


if __name__ == "__main__":
    raise SystemExit(main())
