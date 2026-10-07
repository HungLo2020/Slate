#!/usr/bin/env python3
"""Build Slate's Debian package and hand it to the repository manager."""

from __future__ import annotations

import argparse
import ast
import gzip
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import tomllib
from typing import Callable, Iterable, Mapping
from urllib.error import URLError
from urllib.request import Request, urlopen


REPOSITORY_SCRIPT_URL = (
    "https://raw.githubusercontent.com/HungLo2020/LinuxScripts/master/"
    "GenericScripts/ManageMattOSRepository.py"
)
SCRIPT_RELATIVE_PATH = Path("DevUtils/.downloaded/ManageMattOSRepository.py")
BUILD_METADATA_RELATIVE_PATH = Path("builds/latest-build.env")
PACKAGE_NAME = "slate"
# The repository's package index, used to refuse re-publishing an existing version: apt ignores a
# package whose version it already has, so a re-upload would never reach anyone.
REPOSITORY_PACKAGES_URL = (
    "https://mattpackages.mattsherfey.com/dists/stable/main/binary-amd64/Packages"
)


def repository_root() -> Path:
    return Path(__file__).resolve().parents[1]


def validate_script_content(content: bytes) -> str:
    """Return decoded script text, or raise if the download is not Python."""
    if not content.strip():
        raise ValueError("downloaded repository-management script is empty")
    try:
        text = content.decode("utf-8")
        tree = ast.parse(text, filename="ManageMattOSRepository.py")
    except (UnicodeDecodeError, SyntaxError) as error:
        raise ValueError("downloaded repository-management script is not valid UTF-8 Python") from error
    if not tree.body:
        raise ValueError("downloaded repository-management script has no Python statements")
    return text


def download_latest_script(target: Path, opener: Callable[..., object] = urlopen) -> None:
    """Download the authoritative script and atomically replace *target*."""
    target.parent.mkdir(parents=True, exist_ok=True)
    temporary_path: Path | None = None
    try:
        request = Request(
            REPOSITORY_SCRIPT_URL,
            headers={
                "Cache-Control": "no-cache",
                "Pragma": "no-cache",
                "User-Agent": "SlatePublishMattOSPackage/1.0",
            },
        )
        with opener(request, timeout=30) as response:  # type: ignore[union-attr]
            text = validate_script_content(response.read())  # type: ignore[union-attr]
        descriptor, temporary_name = tempfile.mkstemp(
            prefix=f".{target.name}.", suffix=".tmp", dir=target.parent
        )
        temporary_path = Path(temporary_name)
        with os.fdopen(descriptor, "w", encoding="utf-8") as temporary_file:
            temporary_file.write(text)
            temporary_file.flush()
            os.fsync(temporary_file.fileno())
        os.replace(temporary_path, target)
        temporary_path = None
    finally:
        if temporary_path is not None:
            temporary_path.unlink(missing_ok=True)


def parse_build_metadata(metadata_text: str) -> Mapping[str, str]:
    """Parse KEY=value build metadata without evaluating it as shell."""
    values: dict[str, str] = {}
    for line_number, raw_line in enumerate(metadata_text.splitlines(), start=1):
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue
        if "=" not in line:
            raise ValueError(f"invalid build metadata on line {line_number}: {raw_line!r}")
        key, value = line.split("=", 1)
        key = key.strip()
        if not key or any(character.isspace() for character in key):
            raise ValueError(f"invalid build metadata key on line {line_number}")
        values[key] = value.strip()
    return values


def resolve_deb_artifact(metadata: Mapping[str, str], root: Path) -> Path:
    if metadata.get("BUILD_ARTIFACT_TYPE") != "deb":
        raise ValueError("build did not produce a Debian artifact (BUILD_ARTIFACT_TYPE=deb required)")
    artifact = Path(metadata.get("BUILD_ARTIFACT_PATH", ""))
    if not artifact.is_absolute():
        artifact = root / artifact
    artifact = artifact.resolve()
    if not artifact.is_file():
        raise ValueError(f"package path does not exist: {artifact}")
    if artifact.suffix != ".deb":
        raise ValueError(f"package path is not a .deb file: {artifact}")
    return artifact


def validate_deb_artifact(artifact: Path, version: str) -> None:
    """Don't upload a stale artifact or a different program's package."""
    result = subprocess.run(
        ["dpkg-deb", "--show", "--showformat=${Package}\n${Version}\n${Architecture}\n", "--", str(artifact)],
        check=True, capture_output=True, text=True,
    )
    if result.stdout.splitlines() != [PACKAGE_NAME, version, "amd64"]:
        raise ValueError(f"Expected {PACKAGE_NAME} {version} (amd64); package metadata does not match")


def read_workspace_version(root: Path) -> str:
    """The version every crate (and the .deb) uses: [workspace.package] in Cargo.toml."""
    with (root / "Cargo.toml").open("rb") as cargo_toml:
        manifest = tomllib.load(cargo_toml)
    version = manifest.get("workspace", {}).get("package", {}).get("version", "")
    if not version:
        raise ValueError("Cargo.toml has no [workspace.package] version")
    return version


def fetch_packages_index(opener: Callable[..., object] = urlopen) -> str:
    """Download the repository's Packages index (plain, or .gz if that is all it serves)."""
    errors = []
    for url, compressed in ((REPOSITORY_PACKAGES_URL, False), (REPOSITORY_PACKAGES_URL + ".gz", True)):
        request = Request(url, headers={"Cache-Control": "no-cache", "User-Agent": "SlatePublishMattOSPackage/1.0"})
        try:
            with opener(request, timeout=30) as response:  # type: ignore[union-attr]
                content = response.read()  # type: ignore[union-attr]
        except (OSError, URLError) as error:
            errors.append(f"{url}: {error}")
            continue
        return (gzip.decompress(content) if compressed else content).decode("utf-8")
    raise ValueError("could not read the repository package index: " + "; ".join(errors))


def published_versions(packages_index: str, package: str = PACKAGE_NAME) -> list[str]:
    """Versions of `package` listed in a Debian Packages index."""
    versions = []
    for stanza in packages_index.split("\n\n"):
        fields = {}
        for line in stanza.splitlines():
            if ":" in line and not line.startswith((" ", "\t")):
                key, value = line.split(":", 1)
                fields[key.strip()] = value.strip()
        if fields.get("Package") == package and fields.get("Version"):
            versions.append(fields["Version"])
    return versions


def dpkg_version_greater(candidate: str, existing: str) -> bool:
    """True when `candidate` sorts after `existing` the way apt compares versions."""
    result = subprocess.run(
        ["dpkg", "--compare-versions", candidate, "gt", existing], check=False
    )
    return result.returncode == 0


def ensure_version_is_new(
    version: str,
    published: Iterable[str],
    greater: Callable[[str, str], bool] = dpkg_version_greater,
) -> None:
    """Raise unless `version` is newer than every published version."""
    blocking = [existing for existing in published if not greater(version, existing)]
    if blocking:
        raise ValueError(
            f"{PACKAGE_NAME} {version} is not newer than the published version(s) "
            f"{', '.join(sorted(set(blocking)))}; bump [workspace.package] version in Cargo.toml"
        )


def run(command: list[str], root: Path) -> int:
    try:
        return subprocess.run(command, cwd=root, check=False).returncode
    except OSError as error:
        print(f"[publish] ERROR: unable to run {' '.join(command)}: {error}", file=sys.stderr)
        return 127


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("doctor", "publish"))
    parser.add_argument("--dry-run", action="store_true", help="Build and validate without uploading")
    parser.add_argument("--package", type=Path, help="Publish an already built .deb after checking its metadata")
    args = parser.parse_args(argv)
    if args.command == "doctor" and (args.dry_run or args.package):
        parser.error("--dry-run and --package apply only to publish")

    root = repository_root()
    downloaded_script = root / SCRIPT_RELATIVE_PATH
    try:
        print(f"[publish] Downloading latest repository-management script from {REPOSITORY_SCRIPT_URL}", flush=True)
        download_latest_script(downloaded_script)
    except (OSError, ValueError) as error:
        print(f"[publish] ERROR: download failed; cached scripts are not used: {error}", file=sys.stderr)
        return 1

    manager = [sys.executable, str(downloaded_script), "--repo", "mattpackages"]
    if args.command == "doctor":
        print("[publish] Running repository doctor")
        return run(manager + ["doctor"], root)

    try:
        version = read_workspace_version(root)
        print(f"[publish] Checking that {PACKAGE_NAME} {version} is not already published")
        ensure_version_is_new(version, published_versions(fetch_packages_index()))
    except (OSError, ValueError) as error:
        print(f"[publish] ERROR: {error}", file=sys.stderr)
        return 1

    if args.package is None:
        print("[publish] Building Debian package", flush=True)
        status = run(["bash", str(root / "DevUtils/Build.sh")], root)
        if status != 0:
            return status

    try:
        if args.package is None:
            metadata_path = root / BUILD_METADATA_RELATIVE_PATH
            metadata = parse_build_metadata(metadata_path.read_text(encoding="utf-8"))
            artifact = resolve_deb_artifact(metadata, root)
        else:
            artifact = resolve_deb_artifact({"BUILD_ARTIFACT_TYPE": "deb", "BUILD_ARTIFACT_PATH": str(args.package)}, Path.cwd())
        validate_deb_artifact(artifact, version)
        # Repeat the version check after building to catch concurrent publication.
        ensure_version_is_new(version, published_versions(fetch_packages_index()))
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"[publish] ERROR: {error}", file=sys.stderr)
        return 1

    print(f"[publish] Handing package to authoritative repository manager: {artifact}")
    if args.dry_run:
        manager += ["--dry-run"]
    return run(manager + ["upload", "--no-overwrites", str(artifact)], root)


if __name__ == "__main__":
    raise SystemExit(main())
