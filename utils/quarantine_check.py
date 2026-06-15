#!/usr/bin/env python3
"""Supply-chain age gate for Cargo dependencies.

This is a zero-dependency stand-in for `cargo-quarantine-core`. It reads
`Cargo.lock`, asks crates.io when each registry crate version was published,
and fails the build if any version is younger than a configured cooldown
window. The premise: most supply-chain attacks live in the narrow window
between a malicious publish and its detection, so refusing brand-new versions
buys time for security researchers and automated scanners to catch them.

Only crates.io registry packages are age-checked. Git, local path, and
alternative-registry dependencies are reported separately because their
provenance is governed by commit pinning, not publish age, and crates.io has
no publish timestamp for them.

A published version's `created_at` is immutable, so results are cached on disk
and only previously unseen versions ever hit the network. That keeps repeat CI
runs cheap and keeps us within crates.io's crawler etiquette.

Usage:
    python3 utils/quarantine_check.py [--min-age-days N] [--lockfile PATH]
                                      [--allowlist PATH] [--cache PATH]
                                      [--request-delay-seconds F]

Exit status is 0 when every checked version clears the gate (or is allowlisted),
and 1 when any version is too young or its publish date cannot be verified.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
import time
import urllib.error
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

# --- Configuration constants -------------------------------------------------

# The cooldown window. Seven days is the floor; fourteen is preferred for
# runtime, build-tooling, crypto, networking, and other high-blast-radius
# crates. The default here is the floor; raise it per project via --min-age-days.
MIN_AGE_DAYS_DEFAULT = 7

# crates.io asks crawlers to identify themselves and to stay around one request
# per second. We honor both: a descriptive User-Agent with a contact URL, and a
# default inter-request delay. Only uncached versions incur a request.
USER_AGENT = "drafftink-quarantine-check (+https://github.com/PatWie/drafft-ink)"
REQUEST_DELAY_SECONDS_DEFAULT = 1.0
HTTP_TIMEOUT_SECONDS = 30

# Bounded retry on rate limiting so a transient 429 does not fail CI outright.
RATE_LIMIT_RETRY_MAX = 3
RATE_LIMIT_BACKOFF_SECONDS = 5.0

HTTP_OK = 200
HTTP_NOT_FOUND = 404
HTTP_TOO_MANY_REQUESTS = 429

SECONDS_PER_DAY = 86400

# Only this source string denotes a public crates.io dependency. Everything
# else (git+, path-style absence of source, alternative registries) is out of
# scope for an age gate.
CRATES_IO_REGISTRY_SOURCE = "registry+https://github.com/rust-lang/crates.io-index"

CRATES_IO_VERSION_URL = "https://crates.io/api/v1/crates/{name}/{version}"

DEFAULT_LOCKFILE = "Cargo.lock"
DEFAULT_ALLOWLIST = "utils/quarantine_allow.txt"
DEFAULT_CACHE = str(Path.home() / ".cache" / "drafftink-quarantine" / "versions.json")


def main(argv: list[str]) -> int:
    """Parse arguments, run the gate, and return a process exit code.

    Returns 0 when all checked versions clear the cooldown window (or are
    explicitly allowlisted) and 1 on any violation or unverifiable version.
    """
    arguments = parse_arguments(argv)
    assert arguments.min_age_days >= 0, "cooldown window cannot be negative"
    assert arguments.request_delay_seconds >= 0, "request delay cannot be negative"

    lockfile_path = Path(arguments.lockfile)
    if not lockfile_path.is_file():
        print(f"error: lockfile not found: {lockfile_path}", file=sys.stderr)
        return 1

    registry_packages, skipped_packages = load_cargo_lock_packages(lockfile_path)
    allowlist = load_allowlist(Path(arguments.allowlist))
    cache = cache_load(Path(arguments.cache))

    print(
        f"Quarantine gate: {len(registry_packages)} crates.io packages, "
        f"cooldown {arguments.min_age_days} day(s), "
        f"{len(skipped_packages)} non-registry skipped."
    )

    now = datetime.now(timezone.utc)
    violations = check_packages(registry_packages, allowlist, cache, now, arguments)

    cache_save(Path(arguments.cache), cache)
    return report_and_exit_code(violations, skipped_packages, arguments.min_age_days)


def parse_arguments(argv: list[str]) -> argparse.Namespace:
    """Build the CLI surface; environment variables provide CI-friendly defaults."""
    assert isinstance(argv, list), "argv must be a list"

    parser = argparse.ArgumentParser(description="Cargo dependency supply-chain age gate.")
    parser.add_argument(
        "--min-age-days",
        type=int,
        default=int(os.environ.get("QUARANTINE_MIN_AGE_DAYS", MIN_AGE_DAYS_DEFAULT)),
        help="Minimum age in days a published version must have to pass.",
    )
    parser.add_argument("--lockfile", default=DEFAULT_LOCKFILE, help="Path to Cargo.lock.")
    parser.add_argument(
        "--allowlist",
        default=DEFAULT_ALLOWLIST,
        help="Path to allowlist file (name or name==version per line).",
    )
    parser.add_argument("--cache", default=DEFAULT_CACHE, help="Path to the publish-date cache.")
    parser.add_argument(
        "--request-delay-seconds",
        type=float,
        default=float(os.environ.get("QUARANTINE_REQUEST_DELAY", REQUEST_DELAY_SECONDS_DEFAULT)),
        help="Polite delay between uncached crates.io requests.",
    )

    arguments = parser.parse_args(argv)
    assert arguments.lockfile, "lockfile path must be non-empty"
    return arguments


def load_cargo_lock_packages(lockfile_path: Path) -> tuple[list[tuple[str, str]], list[tuple[str, str, str]]]:
    """Parse Cargo.lock into registry and non-registry package lists.

    Cargo.lock is TOML, but we parse the `[[package]]` blocks by hand so the
    script stays free of any dependency (tomllib only exists on Python 3.11+).
    Returns (registry_packages, skipped_packages) where registry entries are
    (name, version) and skipped entries are (name, version, source_label).
    """
    assert lockfile_path.is_file(), "lockfile must exist before parsing"

    text = lockfile_path.read_text(encoding="utf-8")
    registry_packages: list[tuple[str, str]] = []
    skipped_packages: list[tuple[str, str, str]] = []

    # Each package is a `[[package]]` block; split on it and read the fields.
    for block in text.split("[[package]]")[1:]:
        name_match = re.search(r'^\s*name\s*=\s*"([^"]+)"', block, re.MULTILINE)
        version_match = re.search(r'^\s*version\s*=\s*"([^"]+)"', block, re.MULTILINE)
        source_match = re.search(r'^\s*source\s*=\s*"([^"]+)"', block, re.MULTILINE)
        if not name_match or not version_match:
            continue

        name = name_match.group(1)
        version = version_match.group(1)
        source = source_match.group(1) if source_match else ""

        if source == CRATES_IO_REGISTRY_SOURCE:
            registry_packages.append((name, version))
        else:
            # No source = local workspace/path member; git+ or other = pinned elsewhere.
            label = source if source else "local path / workspace member"
            skipped_packages.append((name, version, label))

    assert registry_packages or skipped_packages, "lockfile parsed to zero packages"
    return registry_packages, skipped_packages


def load_allowlist(allowlist_path: Path) -> set[str]:
    """Read consciously-accepted exemptions.

    Each non-comment line is either a bare crate name (exempts every version of
    that crate) or `name==version` (exempts one specific version). An empty or
    missing file means no exemptions, which is the safe default.
    """
    assert isinstance(allowlist_path, Path), "allowlist path must be a Path"

    entries: set[str] = set()
    if not allowlist_path.is_file():
        return entries

    for raw_line in allowlist_path.read_text(encoding="utf-8").splitlines():
        line = raw_line.split("#", 1)[0].strip()
        if line:
            entries.add(line)
    return entries


def is_allowed(name: str, version: str, allowlist: set[str]) -> bool:
    """Return True when this crate or this exact version is exempt."""
    assert name, "crate name must be non-empty"
    assert version, "crate version must be non-empty"
    return name in allowlist or f"{name}=={version}" in allowlist


def check_packages(
    registry_packages: list[tuple[str, str]],
    allowlist: set[str],
    cache: dict[str, str],
    now: datetime,
    arguments: argparse.Namespace,
) -> list[tuple[str, str, float | None]]:
    """Resolve each package's age and collect violations.

    A violation is (name, version, age_days) where age_days is None when the
    publish date could not be verified at all. Per policy, unverifiable
    versions are treated as violations rather than silently trusted.
    """
    assert now.tzinfo is not None, "comparison time must be timezone-aware"

    violations: list[tuple[str, str, float | None]] = []
    for name, version in registry_packages:
        if is_allowed(name, version, allowlist):
            continue

        created_at = fetch_created_at(name, version, cache, arguments.request_delay_seconds)
        if created_at is None:
            violations.append((name, version, None))
            continue

        age_days = (now - created_at).total_seconds() / SECONDS_PER_DAY
        assert age_days >= -1, "publish date is implausibly in the future"
        if age_days < arguments.min_age_days:
            violations.append((name, version, age_days))

    return violations


def fetch_created_at(
    name: str,
    version: str,
    cache: dict[str, str],
    request_delay_seconds: float,
) -> datetime | None:
    """Return the publish time for one crate version, using and updating the cache.

    Publish times never change, so a cache hit is authoritative and free. On a
    miss we query crates.io's single-version endpoint, honor a polite delay, and
    retry a bounded number of times on rate limiting. Returns None when the
    version cannot be located (e.g. 404 or repeated failure).
    """
    assert name and version, "name and version are required"

    cache_key = f"{name}=={version}"
    if cache_key in cache:
        return parse_iso8601_utc(cache[cache_key])

    url = CRATES_IO_VERSION_URL.format(name=name, version=version)
    request = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})

    for attempt in range(RATE_LIMIT_RETRY_MAX):
        # Be polite: delay before every network call, not just between successes.
        time.sleep(request_delay_seconds)
        try:
            with urllib.request.urlopen(request, timeout=HTTP_TIMEOUT_SECONDS) as response:
                if response.status != HTTP_OK:
                    continue
                payload = json.load(response)
        except urllib.error.HTTPError as error:
            if error.code == HTTP_NOT_FOUND:
                return None  # Version genuinely absent from crates.io.
            if error.code == HTTP_TOO_MANY_REQUESTS:
                time.sleep(RATE_LIMIT_BACKOFF_SECONDS)
                continue
            return None
        except (urllib.error.URLError, json.JSONDecodeError, TimeoutError):
            time.sleep(RATE_LIMIT_BACKOFF_SECONDS)
            continue

        created_at = payload.get("version", {}).get("created_at")
        if not created_at:
            return None

        cache[cache_key] = created_at
        assert parse_iso8601_utc(created_at) is not None, "cached date must be parseable"
        return parse_iso8601_utc(created_at)

    return None  # Exhausted retries without a definitive answer.


def parse_iso8601_utc(value: str) -> datetime | None:
    """Parse a crates.io timestamp into a UTC datetime.

    crates.io returns values like `2026-06-12T12:22:39.813180Z`. We extract the
    calendar fields with a regex rather than rely on `fromisoformat`, whose
    handling of the trailing `Z` and fractional seconds varies across Python
    versions. Returns None on an unrecognized shape.
    """
    assert isinstance(value, str), "timestamp must be a string"

    match = re.match(r"(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})", value)
    if not match:
        return None

    year, month, day, hour, minute, second = (int(group) for group in match.groups())
    return datetime(year, month, day, hour, minute, second, tzinfo=timezone.utc)


def cache_load(cache_path: Path) -> dict[str, str]:
    """Load the publish-date cache, tolerating a missing or corrupt file."""
    assert isinstance(cache_path, Path), "cache path must be a Path"

    if not cache_path.is_file():
        return {}
    try:
        data = json.loads(cache_path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, OSError):
        # A damaged cache is not fatal; rebuild it from scratch this run.
        return {}
    assert isinstance(data, dict), "cache file must contain a JSON object"
    return {str(key): str(val) for key, val in data.items()}


def cache_save(cache_path: Path, cache: dict[str, str]) -> None:
    """Persist the publish-date cache, creating parent directories as needed."""
    assert isinstance(cache, dict), "cache must be a dict"

    cache_path.parent.mkdir(parents=True, exist_ok=True)
    cache_path.write_text(json.dumps(cache, indent=2, sort_keys=True), encoding="utf-8")
    assert cache_path.is_file(), "cache must exist after saving"


def report_and_exit_code(
    violations: list[tuple[str, str, float | None]],
    skipped_packages: list[tuple[str, str, str]],
    min_age_days: int,
) -> int:
    """Print a human-readable report and return the process exit code."""
    assert min_age_days >= 0, "cooldown window cannot be negative"

    if skipped_packages:
        print(f"\nSkipped {len(skipped_packages)} non-crates.io dependencies (review manually):")
        for name, version, label in sorted(skipped_packages):
            print(f"  - {name} {version}  [{label}]")

    if not violations:
        print(f"\nPASS: every crates.io dependency is at least {min_age_days} day(s) old.")
        return 0

    print(f"\nFAIL: {len(violations)} dependency version(s) violate the {min_age_days}-day gate:")
    for name, version, age_days in sorted(violations, key=lambda item: (item[2] is not None, item[2] or 0)):
        if age_days is None:
            print(f"  - {name} {version}  (publish date UNVERIFIABLE)")
        else:
            print(f"  - {name} {version}  ({age_days:.1f} days old)")
    print(
        "\nResolve by pinning an older compatible version, or add a reviewed "
        "exemption to the allowlist (name or name==version)."
    )
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
