#!/usr/bin/env python3
"""Validate Linux package determinism, safety, provenance, and user-prefix install."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import stat
import subprocess
import sys
import tarfile
import tempfile


TARGET = "x86_64-unknown-linux-gnu"
BASELINE = "ubuntu-24.04-x86_64-glibc"
SHA_RE = re.compile(r"^[0-9a-f]{64}$")
GLIBC_RE = re.compile(rb"GLIBC_([0-9]+)\.([0-9]+)(?:\.([0-9]+))?")
MAX_OUTPUT = 4 * 1024 * 1024


class ContractError(RuntimeError):
    pass


def run(
    command: list[str],
    *,
    timeout: int = 30,
    environment: dict[str, str] | None = None,
    process_umask: int | None = None,
) -> subprocess.CompletedProcess[bytes]:
    try:
        result = subprocess.run(
            command,
            check=False,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=timeout,
            env=environment,
            preexec_fn=(lambda: os.umask(process_umask)) if process_umask is not None else None,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise ContractError(f"command unavailable or timed out: {command[0]}") from error
    if len(result.stdout) > MAX_OUTPUT or len(result.stderr) > MAX_OUTPUT:
        raise ContractError(f"command exceeded output bound: {command[0]}")
    return result


def run_text(command: list[str], *, timeout: int = 30) -> str:
    result = run(command, timeout=timeout)
    if result.returncode != 0:
        raise ContractError(f"command failed: {command[0]}")
    try:
        return result.stdout.decode("utf-8", errors="strict").strip()
    except UnicodeDecodeError as error:
        raise ContractError(f"command output was not UTF-8: {command[0]}") from error


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def observed_glibc_floor(binary: Path) -> str:
    result = run(["readelf", "--version-info", str(binary)])
    if result.returncode != 0:
        raise ContractError("readelf could not measure the binary GLIBC symbol floor")
    versions = {
        (int(major), int(minor), int(patch or b"0"))
        for major, minor, patch in GLIBC_RE.findall(result.stdout)
    }
    if not versions:
        raise ContractError("binary declared no measurable GLIBC symbol requirement")
    major, minor, patch = max(versions)
    return f"GLIBC_{major}.{minor}" + (f".{patch}" if patch else "")


def validate_members(members: list[tarfile.TarInfo], root: str) -> None:
    names: set[str] = set()
    expected_directories = {root, f"{root}/completions"}
    for member in members:
        path = PurePosixPath(member.name)
        if path.is_absolute() or ".." in path.parts or not path.parts or path.parts[0] != root:
            raise ContractError(f"unsafe archive path: {member.name}")
        if member.name in names:
            raise ContractError(f"duplicate archive path: {member.name}")
        names.add(member.name)
        if not (member.isfile() or member.isdir()):
            raise ContractError(f"archive contains a link or special entry: {member.name}")
        if (member.name in expected_directories and not member.isdir()) or (
            member.name not in expected_directories and not member.isfile()
        ):
            raise ContractError(f"archive entry type was invalid: {member.name}")
        if member.pax_headers:
            raise ContractError(f"archive contains unbounded PAX metadata: {member.name}")


def archive_contract(archive_path: Path, version: str, source: str, epoch: int, floor: str) -> dict[str, bytes]:
    root = f"vmcell-v{version}-linux-x86_64"
    relative_files = [
        "BUILD-PROVENANCE.json",
        "INSTALL.txt",
        "LICENSE.txt",
        "NOTICE.txt",
        "PACKAGE-CONTENTS.sha256",
        "PACKAGE-METADATA.json",
        "README.txt",
        "completions/_vmcell",
        "completions/vmcell.bash",
        "vmcell",
        "vmcell-portable-layout.py",
    ]
    expected = [root, f"{root}/completions"] + [f"{root}/{name}" for name in relative_files]
    archive_bytes = archive_path.read_bytes()
    if len(archive_bytes) < 10 or archive_bytes[:2] != b"\x1f\x8b" or archive_bytes[4:8] != b"\0\0\0\0":
        raise ContractError("gzip header or timestamp was not normalized")
    with tarfile.open(archive_path, mode="r:gz") as archive:
        members = archive.getmembers()
        validate_members(members, root)
        if [member.name for member in members] != expected:
            raise ContractError("portable archive layout or deterministic ordering changed")
        for index, member in enumerate(members):
            if (index < 2 and not member.isdir()) or (index >= 2 and not member.isfile()):
                raise ContractError(f"archive entry type changed unexpectedly: {member.name}")
            if member.uid != 0 or member.gid != 0 or member.uname or member.gname:
                raise ContractError(f"archive ownership metadata was not normalized: {member.name}")
            if member.mtime != epoch:
                raise ContractError(f"archive timestamp was not normalized: {member.name}")
            expected_mode = (
                0o755
                if index < 2
                or member.name in {f"{root}/vmcell", f"{root}/vmcell-portable-layout.py"}
                else 0o644
            )
            if member.mode != expected_mode:
                raise ContractError(f"archive mode was not normalized: {member.name}")
        contents: dict[str, bytes] = {}
        for name in relative_files:
            stream = archive.extractfile(f"{root}/{name}")
            if stream is None:
                raise ContractError(f"archive file was unreadable: {name}")
            contents[name] = stream.read()

    try:
        metadata = json.loads(contents["PACKAGE-METADATA.json"].decode("utf-8", errors="strict"))
        provenance = json.loads(contents["BUILD-PROVENANCE.json"].decode("utf-8", errors="strict"))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise ContractError("package metadata or provenance was not strict UTF-8 JSON") from error
    if metadata != {
        "archive_name": f"{root}.tar.gz",
        "binary_relative_path": "vmcell",
        "checksum_relative_path": "PACKAGE-CONTENTS.sha256",
        "commands": ["vmcell"],
        "completion_relative_paths": ["completions/vmcell.bash", "completions/_vmcell"],
        "install_scope": "user",
        "libc": "glibc",
        "observed_required_glibc": floor,
        "package_identifier": "JerrySkywalker.vmcell",
        "package_name": "VM Cell Manager",
        "package_version": version,
        "portable": True,
        "publication_status": "candidate_only",
        "schema_version": 1,
        "target": TARGET,
    }:
        raise ContractError("package metadata contract changed unexpectedly")
    required_provenance = {
        "binary_reproducibility": "not claimed",
        "build_baseline": BASELINE,
        "build_profile": "release",
        "build_glibc_version": run_text(["ldd", "--version"]).splitlines()[0],
        "libc_implementation": "glibc",
        "observed_required_glibc": floor,
        "package": "vmcell",
        "schema_version": 1,
        "source_commit": source,
        "source_date_epoch": epoch,
        "tar_format": "ustar",
        "target": TARGET,
        "version": version,
    }
    if any(provenance.get(key) != value for key, value in required_provenance.items()):
        raise ContractError("package provenance did not preserve its exact build identity")
    if provenance.get("binary_sha256") != sha256(contents["vmcell"]):
        raise ContractError("package provenance binary hash mismatch")
    if not str(provenance.get("rustc_version", "")).startswith("rustc 1.85.0 "):
        raise ContractError("package provenance did not retain the MSRV compiler identity")
    if not str(provenance.get("cargo_version", "")).startswith("cargo 1.85.0 "):
        raise ContractError("package provenance did not retain the MSRV Cargo identity")
    if provenance.get("archive_entries") != expected:
        raise ContractError("package provenance did not bind the exact archive layout")

    manifest_names = sorted(name for name in relative_files if name != "PACKAGE-CONTENTS.sha256")
    try:
        manifest_lines = contents["PACKAGE-CONTENTS.sha256"].decode("ascii", errors="strict").splitlines()
    except UnicodeDecodeError as error:
        raise ContractError("package content manifest was not ASCII") from error
    if len(manifest_lines) != len(manifest_names):
        raise ContractError("package content manifest entry count changed")
    for line, name in zip(manifest_lines, manifest_names, strict=True):
        parts = line.split("  ", 1)
        if len(parts) != 2 or not SHA_RE.fullmatch(parts[0]) or parts[1] != name:
            raise ContractError("package content manifest was malformed or unordered")
        if parts[0] != sha256(contents[name]):
            raise ContractError(f"package content checksum mismatch: {name}")
    return contents


def negative_archive_regressions(root: str, epoch: int) -> None:
    fixtures: list[list[tuple[str, bytes, bytes | None]]] = [
        [("../escape", tarfile.REGTYPE, b"x")],
        [(f"/{root}/absolute", tarfile.REGTYPE, b"x")],
        [(root, tarfile.REGTYPE, b"not-a-directory")],
        [(root, tarfile.DIRTYPE, None), (root, tarfile.DIRTYPE, None)],
        [(root, tarfile.DIRTYPE, None), (f"{root}/vmcell", tarfile.SYMTYPE, b"target")],
    ]
    for fixture in fixtures:
        buffer = io.BytesIO()
        with tarfile.open(fileobj=buffer, mode="w", format=tarfile.USTAR_FORMAT) as archive:
            for name, entry_type, content in fixture:
                entry = tarfile.TarInfo(name)
                entry.type = entry_type
                entry.mode = 0o755
                entry.uid = 0
                entry.gid = 0
                entry.mtime = epoch
                if entry_type == tarfile.SYMTYPE:
                    entry.linkname = (content or b"").decode("ascii")
                    archive.addfile(entry)
                elif content is None:
                    archive.addfile(entry)
                else:
                    entry.size = len(content)
                    archive.addfile(entry, io.BytesIO(content))
        buffer.seek(0)
        try:
            with tarfile.open(fileobj=buffer, mode="r:") as archive:
                validate_members(archive.getmembers(), root)
        except ContractError:
            continue
        raise ContractError("unsafe archive regression was accepted")


def validate_installed_layout(contents: dict[str, bytes], install_root: Path) -> None:
    expected_root = {Path(name).parts[0] for name in contents}
    expected_root.add("completions")
    actual_root = {entry.name for entry in os.scandir(install_root)}
    if actual_root != expected_root:
        raise ContractError("installed layout contains missing or foreign root entries")
    actual_completions = {entry.name for entry in os.scandir(install_root / "completions")}
    expected_completions = {
        Path(name).name for name in contents if Path(name).parts[0] == "completions"
    }
    if actual_completions != expected_completions:
        raise ContractError("installed layout contains missing or foreign completion entries")
    for directory, expected_mode in ((install_root, 0o700), (install_root / "completions", 0o755)):
        status = os.lstat(directory)
        if not stat.S_ISDIR(status.st_mode) or stat.S_ISLNK(status.st_mode) or status.st_uid != os.geteuid():
            raise ContractError("installed directory identity was not exact current-user ownership")
        if stat.S_IMODE(status.st_mode) != expected_mode:
            raise ContractError("installed directory mode drifted")
    for relative, expected in contents.items():
        path = install_root / relative
        status = os.lstat(path)
        if not stat.S_ISREG(status.st_mode) or stat.S_ISLNK(status.st_mode) or status.st_uid != os.geteuid():
            raise ContractError(f"installed file identity drifted: {relative}")
        if path.read_bytes() != expected:
            raise ContractError(f"installed file content drifted: {relative}")
        expected_mode = 0o755 if relative in {"vmcell", "vmcell-portable-layout.py"} else 0o644
        if stat.S_IMODE(status.st_mode) != expected_mode:
            raise ContractError(f"installed file mode drifted: {relative}")


def install_smoke(
    archive: Path,
    contents: dict[str, bytes],
    version: str,
    test_root: Path,
) -> None:
    home = test_root / "home"
    home.mkdir(mode=0o700)
    extraction = test_root / "extraction"
    extraction.mkdir(mode=0o700)
    extracted = run(
        [
            "sh",
            "-c",
            'umask 077; (umask 022; exec tar -xzf "$1" -C "$2")',
            "vmcell-package-extract",
            str(archive),
            str(extraction),
        ]
    )
    if extracted.returncode != 0:
        raise ContractError("validated archive could not be extracted for install smoke")
    layout = extraction / f"vmcell-v{version}-linux-x86_64"
    installer = layout / "vmcell-portable-layout.py"
    prefix_parent = home / ".local" / "lib" / "vmcell"
    install_root = prefix_parent / f"vmcell-v{version}-linux-x86_64"
    environment = os.environ.copy()
    environment.update({"HOME": str(home), "LC_ALL": "C", "LANG": "C", "TZ": "UTC"})
    installed = run(
        [sys.executable, str(installer), "install", "--parent", str(prefix_parent)],
        environment=environment,
        process_umask=0o077,
    )
    if installed.returncode != 0:
        raise ContractError(
            f"portable installer failed: {installed.stderr.decode('utf-8', errors='replace')}"
        )
    for component in (home / ".local", home / ".local" / "lib", prefix_parent):
        status = os.lstat(component)
        if not stat.S_ISDIR(status.st_mode) or stat.S_IMODE(status.st_mode) != 0o700:
            raise ContractError("portable installer did not create a private parent chain")
    sentinel = prefix_parent / "owner-sentinel"
    sentinel.write_text("retain\n", encoding="utf-8")
    validate_installed_layout(contents, install_root)
    binary_path = install_root / "vmcell"
    if stat.S_IMODE(binary_path.stat().st_mode) != 0o755:
        raise ContractError("installed binary mode was not executable")
    state_root = test_root / "read-only-state"
    commands = [
        ([str(binary_path), "--version"], f"vmcell {version}"),
        ([str(binary_path), "--help"], "Local disposable VM execution cells"),
        ([str(binary_path), "--json", "--state-root", str(state_root), "doctor"], '"schema_version"'),
        ([str(binary_path), "--json", "--state-root", str(state_root), "status"], '"schema_version"'),
    ]
    for command, marker in commands:
        result = run(command, environment=environment)
        if result.returncode != 0 or marker.encode("utf-8") not in result.stdout:
            raise ContractError(f"unprivileged install smoke failed: {command[-1]}")
    if state_root.exists():
        raise ContractError("read-only doctor/status smoke created the isolated state root")

    collision = run(
        [sys.executable, str(installer), "install", "--parent", str(prefix_parent)],
        environment=environment,
    )
    if collision.returncode == 0:
        raise ContractError("fresh installation overwrote an existing target")
    validate_installed_layout(contents, install_root)

    drift_parent = home / "drift-parent"
    drift_parent.mkdir(mode=0o700)
    drift_root = drift_parent / f"vmcell-v{version}-linux-x86_64"
    drift_install = run(
        [sys.executable, str(installer), "install", "--parent", str(drift_parent)],
        environment=environment,
    )
    if drift_install.returncode != 0:
        raise ContractError("portable installer could not create the drift regression layout")
    drifted_binary = drift_root / "vmcell"
    drifted_binary.write_text("foreign replacement\n", encoding="utf-8")
    os.chmod(drifted_binary, 0o755)
    drift_remove = run(
        [sys.executable, str(installer), "remove", "--parent", str(drift_parent)],
        environment=environment,
    )
    if drift_remove.returncode == 0:
        raise ContractError("removal accepted a replaced installed binary")
    if drifted_binary.read_text(encoding="utf-8") != "foreign replacement\n":
        raise ContractError("failed removal deleted a foreign replacement")

    linked_parent = home / "linked-parent"
    linked_parent.symlink_to(prefix_parent, target_is_directory=True)
    linked = run(
        [sys.executable, str(installer), "install", "--parent", str(linked_parent)],
        environment=environment,
    )
    if linked.returncode == 0:
        raise ContractError("portable installer accepted a symlink parent")

    missing_parent = home / "remove-typo" / "vmcell"
    missing_remove = run(
        [sys.executable, str(installer), "remove", "--parent", str(missing_parent)],
        environment=environment,
    )
    if missing_remove.returncode == 0 or (home / "remove-typo").exists():
        raise ContractError("removal created a missing or mistyped parent")

    self_remove = run(
        [
            sys.executable,
            str(install_root / "vmcell-portable-layout.py"),
            "remove",
            "--parent",
            str(prefix_parent),
        ],
        environment=environment,
    )
    if self_remove.returncode == 0:
        raise ContractError("installed helper authorized its own removal")
    validate_installed_layout(contents, install_root)

    spec = importlib.util.spec_from_file_location("vmcell_portable_layout_contract", installer)
    if spec is None or spec.loader is None:
        raise ContractError("portable installer identity regression could not load")
    installer_module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(installer_module)
    identity_parent = home / "identity-parent"
    identity_parent.mkdir(mode=0o700)
    identity_target = identity_parent / "target"
    identity_target.mkdir(mode=0o700)
    identity_parent_descriptor = os.open(identity_parent, os.O_RDONLY | os.O_DIRECTORY)
    try:
        identity_status = os.stat(identity_target, follow_symlinks=False)
        try:
            installer_module.cleanup_layout(
                identity_parent_descriptor,
                identity_target.name,
                require_valid=False,
                forbidden_root_identity=(identity_status.st_dev, identity_status.st_ino),
            )
        except installer_module.LayoutError:
            pass
        else:
            raise ContractError("portable remover accepted its source as the installed target")
    finally:
        os.close(identity_parent_descriptor)
    if not identity_target.is_dir():
        raise ContractError("identity-separation rejection deleted its target")

    removed = run(
        [sys.executable, str(installer), "remove", "--parent", str(prefix_parent)],
        environment=environment,
    )
    if removed.returncode != 0 or install_root.exists():
        raise ContractError("exact portable-package removal failed")
    if not sentinel.is_file() or sentinel.read_text(encoding="utf-8") != "retain\n":
        raise ContractError("package removal touched an unrelated owner file")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    args = parser.parse_args()
    if os.geteuid() == 0:
        raise ContractError("Linux portable-package smoke must run as an unprivileged identity")
    repository_root = Path(__file__).resolve(strict=True).parent.parent
    binary = Path(args.binary).resolve(strict=True)
    metadata = json.loads(
        run_text(["cargo", "metadata", "--locked", "--offline", "--no-deps", "--format-version", "1"])
    )
    packages = [package for package in metadata["packages"] if package["name"] == "vm-cell-manager"]
    if len(packages) != 1:
        raise ContractError("Cargo metadata did not identify exactly one vm-cell-manager package")
    version = packages[0]["version"]
    source = run_text(["git", "rev-parse", "HEAD"])
    epoch_text = run_text(["git", "show", "-s", "--format=%ct", "HEAD"])
    if not re.fullmatch(r"[0-9a-f]{40}", source) or not epoch_text.isdigit():
        raise ContractError("Git source identity was unavailable")
    epoch = int(epoch_text)
    rust_host = run_text(["rustc", "-vV"])
    if "release: 1.85.0" not in rust_host or f"host: {TARGET}" not in rust_host:
        raise ContractError("Linux package gate requires Rust 1.85.0 for x86_64-unknown-linux-gnu")
    os_release = Path("/etc/os-release").read_text(encoding="utf-8")
    if '\nID=ubuntu\n' not in f"\n{os_release}" or '\nVERSION_ID="24.04"\n' not in f"\n{os_release}":
        raise ContractError("Linux package gate requires the declared Ubuntu 24.04 baseline")
    floor = observed_glibc_floor(binary)
    elf_header = run_text(["readelf", "--file-header", str(binary)])
    if "Machine:" not in elf_header or "X86-64" not in elf_header:
        raise ContractError("Linux package binary was not an x86_64 ELF executable")
    package_script = repository_root / "tools/package-linux.py"

    with tempfile.TemporaryDirectory(prefix="vmcell-linux-package-contract-") as temporary:
        test_root = Path(temporary)
        first = test_root / "first"
        second = test_root / "second"
        common = [
            sys.executable,
            str(package_script),
            "--binary",
            str(binary),
            "--version",
            version,
            "--source-commit",
            source,
            "--source-date-epoch",
            str(epoch),
            "--glibc-floor",
            floor,
        ]
        for output in (first, second):
            result = run(common + ["--output-directory", str(output)], timeout=60)
            if result.returncode != 0:
                raise ContractError(f"package assembly failed: {result.stderr.decode('utf-8', errors='replace')}")
        archive_name = f"vmcell-v{version}-linux-x86_64.tar.gz"
        first_archive = first / archive_name
        second_archive = second / archive_name
        if first_archive.read_bytes() != second_archive.read_bytes():
            raise ContractError("repeated package assembly was not byte-identical")
        if (first / "SHA256SUMS.txt").read_bytes() != (second / "SHA256SUMS.txt").read_bytes():
            raise ContractError("repeated checksum manifests were not byte-identical")
        adjacent = (first / "SHA256SUMS.txt").read_text(encoding="ascii").splitlines()
        if adjacent != [f"{sha256(first_archive.read_bytes())}  {archive_name}"]:
            raise ContractError("adjacent checksum did not bind the exact archive")
        contents = archive_contract(first_archive, version, source, epoch, floor)
        negative_archive_regressions(f"vmcell-v{version}-linux-x86_64", epoch)

        mismatch = test_root / "version-mismatch"
        result = run(
            [value if value != version else "999.999.999" for value in common]
            + ["--output-directory", str(mismatch)],
            timeout=60,
        )
        if result.returncode == 0 or mismatch.exists():
            raise ContractError("package creation did not fail closed before a version mismatch write")

        source_mismatch = test_root / "source-mismatch"
        source_mismatch_command = [
            "a" * 40 if value == source else value for value in common
        ]
        result = run(
            source_mismatch_command + ["--output-directory", str(source_mismatch)], timeout=60
        )
        if result.returncode == 0 or source_mismatch.exists():
            raise ContractError("package creation did not bind the declared source commit")

        epoch_mismatch = test_root / "epoch-mismatch"
        epoch_mismatch_command = [
            str(epoch + 1) if value == str(epoch) else value for value in common
        ]
        result = run(
            epoch_mismatch_command + ["--output-directory", str(epoch_mismatch)], timeout=60
        )
        if result.returncode == 0 or epoch_mismatch.exists():
            raise ContractError("package creation accepted a false source commit timestamp")

        floor_mismatch = test_root / "floor-mismatch"
        floor_mismatch_command = [
            "GLIBC_999.0" if value == floor else value for value in common
        ]
        result = run(
            floor_mismatch_command + ["--output-directory", str(floor_mismatch)], timeout=60
        )
        if result.returncode == 0 or floor_mismatch.exists():
            raise ContractError("package creation accepted an inaccurate GLIBC floor")

        collision = test_root / "existing-output"
        collision.mkdir()
        sentinel = collision / "owner-sentinel"
        sentinel.write_text("retain\n", encoding="utf-8")
        result = run(common + ["--output-directory", str(collision)], timeout=60)
        if result.returncode == 0 or sentinel.read_text(encoding="utf-8") != "retain\n":
            raise ContractError("package output collision was overwritten or modified")

        binary_link = test_root / "binary-link"
        binary_link.symlink_to(binary)
        linked_output = test_root / "linked-binary-output"
        linked_command = [str(binary_link) if value == str(binary) else value for value in common]
        result = run(linked_command + ["--output-directory", str(linked_output)], timeout=60)
        if result.returncode == 0 or linked_output.exists():
            raise ContractError("package assembly accepted a symlink binary input")

        wrong_arch_binary = test_root / "wrong-architecture-vmcell"
        wrong_arch_bytes = bytearray(binary.read_bytes())
        if wrong_arch_bytes[:4] != b"\x7fELF" or len(wrong_arch_bytes) < 20:
            raise ContractError("candidate binary was not a bounded ELF test input")
        wrong_arch_bytes[18:20] = (183).to_bytes(2, byteorder="little")
        wrong_arch_binary.write_bytes(wrong_arch_bytes)
        os.chmod(wrong_arch_binary, 0o755)
        wrong_arch_output = test_root / "wrong-architecture-output"
        wrong_arch_command = [
            str(wrong_arch_binary) if value == str(binary) else value for value in common
        ]
        result = run(
            wrong_arch_command + ["--output-directory", str(wrong_arch_output)], timeout=60
        )
        if result.returncode == 0 or wrong_arch_output.exists():
            raise ContractError("package assembly accepted a non-x86_64 ELF identity")

        parent_link = test_root / "output-parent-link"
        real_parent = test_root / "real-output-parent"
        real_parent.mkdir()
        parent_link.symlink_to(real_parent, target_is_directory=True)
        linked_parent_output = parent_link / "candidate"
        result = run(common + ["--output-directory", str(linked_parent_output)], timeout=60)
        if result.returncode == 0 or linked_parent_output.exists():
            raise ContractError("package assembly accepted a symlink output parent")

        insecure_parent = test_root / "insecure-output-parent"
        insecure_parent.mkdir(mode=0o777)
        os.chmod(insecure_parent, 0o777)
        insecure_output = insecure_parent / "candidate"
        result = run(common + ["--output-directory", str(insecure_output)], timeout=60)
        if result.returncode == 0 or insecure_output.exists():
            raise ContractError("package assembly accepted a group/world-writable output parent")
        install_smoke(first_archive, contents, version, test_root)

    print(
        json.dumps(
            {
                "archive": f"vmcell-v{version}-linux-x86_64.tar.gz",
                "assembly_reproducible": True,
                "binary_reproducible": False,
                "build_glibc_version": run_text(["ldd", "--version"]).splitlines()[0],
                "install_smoke": "pass",
                "observed_required_glibc": floor,
                "source_commit": source,
                "target": TARGET,
            },
            sort_keys=True,
        )
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (ContractError, OSError, ValueError, KeyError, json.JSONDecodeError) as error:
        print(f"test-linux-package: {error}", file=sys.stderr)
        raise SystemExit(1)
