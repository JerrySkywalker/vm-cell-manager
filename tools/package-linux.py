#!/usr/bin/env python3
"""Assemble the deterministic vmcell portable GNU/Linux candidate archive."""

from __future__ import annotations

import argparse
import ctypes
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import re
import stat
import subprocess
import sys
import tarfile
import tempfile


MAX_BINARY_BYTES = 256 * 1024 * 1024
MAX_TEXT_BYTES = 4 * 1024 * 1024
VERSION_RE = re.compile(r"^[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?$")
SHA_RE = re.compile(r"^[0-9a-f]{40}$")
GLIBC_RE = re.compile(r"^GLIBC_[0-9]+\.[0-9]+(?:\.[0-9]+)?$")
GLIBC_SYMBOL_RE = re.compile(r"\bGLIBC_([0-9]+)\.([0-9]+)(?:\.([0-9]+))?\b")
TARGET = "x86_64-unknown-linux-gnu"
BASELINE = "ubuntu-24.04-x86_64-glibc"


class PackageError(RuntimeError):
    pass


def read_regular(path: Path, label: str, maximum: int) -> bytes:
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    try:
        descriptor = os.open(path, flags)
    except OSError as error:
        raise PackageError(f"{label} must be an accessible ordinary non-symlink file") from error
    try:
        opened = os.fstat(descriptor)
        if not stat.S_ISREG(opened.st_mode) or opened.st_size <= 0 or opened.st_size > maximum:
            raise PackageError(f"{label} must be a non-empty ordinary file within its size bound")
        try:
            current = os.lstat(path)
        except OSError as error:
            raise PackageError(f"{label} path identity changed") from error
        if stat.S_ISLNK(current.st_mode) or (opened.st_dev, opened.st_ino) != (
            current.st_dev,
            current.st_ino,
        ):
            raise PackageError(f"{label} path identity changed")
        chunks: list[bytes] = []
        remaining = opened.st_size
        while remaining:
            chunk = os.read(descriptor, min(remaining, 1024 * 1024))
            if not chunk:
                raise PackageError(f"{label} changed while it was read")
            chunks.append(chunk)
            remaining -= len(chunk)
        if os.read(descriptor, 1):
            raise PackageError(f"{label} exceeded its declared size while it was read")
        after = os.fstat(descriptor)
        current_after = os.lstat(path)
        if (
            (opened.st_dev, opened.st_ino, opened.st_size)
            != (after.st_dev, after.st_ino, after.st_size)
            or (after.st_dev, after.st_ino)
            != (current_after.st_dev, current_after.st_ino)
        ):
            raise PackageError(f"{label} identity changed while it was read")
        return b"".join(chunks)
    finally:
        os.close(descriptor)


def run_bounded(command: list[str], label: str, environment: dict[str, str] | None = None) -> str:
    try:
        result = subprocess.run(
            command,
            check=False,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=15,
            env=environment,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        raise PackageError(f"{label} was unavailable or timed out") from error
    if len(result.stdout) > MAX_TEXT_BYTES or len(result.stderr) > MAX_TEXT_BYTES:
        raise PackageError(f"{label} exceeded its output bound")
    if result.returncode != 0:
        raise PackageError(f"{label} failed")
    try:
        return result.stdout.decode("utf-8", errors="strict").replace("\r\n", "\n")
    except UnicodeDecodeError as error:
        raise PackageError(f"{label} did not emit UTF-8") from error


def json_bytes(value: object) -> bytes:
    return (json.dumps(value, indent=2, sort_keys=True, ensure_ascii=True) + "\n").encode("utf-8")


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def glibc_symbol_floor(version_output: str) -> str:
    versions = {
        (int(major), int(minor), int(patch or "0"))
        for major, minor, patch in GLIBC_SYMBOL_RE.findall(version_output)
    }
    if not versions:
        raise PackageError("binary declared no measurable GLIBC symbol requirement")
    major, minor, patch = max(versions)
    return f"GLIBC_{major}.{minor}" + (f".{patch}" if patch else "")


def add_tar_directory(archive: tarfile.TarFile, name: str, epoch: int) -> None:
    entry = tarfile.TarInfo(name=name)
    entry.type = tarfile.DIRTYPE
    entry.mode = 0o755
    entry.uid = 0
    entry.gid = 0
    entry.uname = ""
    entry.gname = ""
    entry.mtime = epoch
    archive.addfile(entry)


def add_tar_file(archive: tarfile.TarFile, name: str, content: bytes, mode: int, epoch: int) -> None:
    entry = tarfile.TarInfo(name=name)
    entry.size = len(content)
    entry.mode = mode
    entry.uid = 0
    entry.gid = 0
    entry.uname = ""
    entry.gname = ""
    entry.mtime = epoch
    archive.addfile(entry, io.BytesIO(content))


def build_archive(args: argparse.Namespace) -> tuple[str, bytes, str, str]:
    if sys.platform != "linux" or platform.machine() != "x86_64":
        raise PackageError("Linux packaging requires a native x86_64 Linux userspace")
    if not VERSION_RE.fullmatch(args.version):
        raise PackageError("version must be an exact semantic candidate version")
    if not SHA_RE.fullmatch(args.source_commit):
        raise PackageError("source commit must be exactly 40 lowercase hexadecimal characters")
    if not 315_532_800 <= args.source_date_epoch <= 4_354_819_199:
        raise PackageError("source date epoch is outside the admitted range")
    if args.target != TARGET:
        raise PackageError(f"target must be exactly {TARGET}")
    if args.build_baseline != BASELINE:
        raise PackageError(f"build baseline must be exactly {BASELINE}")
    if not GLIBC_RE.fullmatch(args.glibc_floor):
        raise PackageError("glibc floor must be an observed GLIBC_X.Y symbol version")

    repository_root = Path(__file__).resolve(strict=True).parent.parent
    try:
        os_release = Path("/etc/os-release").read_text(encoding="utf-8", errors="strict")
    except (OSError, UnicodeDecodeError) as error:
        raise PackageError("declared build OS identity was unavailable") from error
    if '\nID=ubuntu\n' not in f"\n{os_release}" or '\nVERSION_ID="24.04"\n' not in f"\n{os_release}":
        raise PackageError("declared build baseline requires Ubuntu 24.04")
    repository_head = run_bounded(
        ["git", "-C", str(repository_root), "rev-parse", "HEAD"], "repository source identity"
    ).strip()
    repository_status = run_bounded(
        [
            "git",
            "-C",
            str(repository_root),
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
        ],
        "repository cleanliness",
    )
    if repository_head != args.source_commit or repository_status:
        raise PackageError("repository must be clean and exactly at the declared source commit")
    committed_epoch = run_bounded(
        [
            "git",
            "-C",
            str(repository_root),
            "show",
            "-s",
            "--format=%ct",
            args.source_commit,
        ],
        "committed source timestamp",
    ).strip()
    if not committed_epoch.isdigit() or int(committed_epoch) != args.source_date_epoch:
        raise PackageError("source date epoch must equal the declared commit timestamp")
    try:
        cargo_metadata = json.loads(
            run_bounded(
                [
                    "cargo",
                    "metadata",
                    "--locked",
                    "--offline",
                    "--no-deps",
                    "--format-version",
                    "1",
                    "--manifest-path",
                    str(repository_root / "Cargo.toml"),
                ],
                "Cargo package identity",
            )
        )
    except json.JSONDecodeError as error:
        raise PackageError("Cargo package identity was not strict JSON") from error
    packages = [
        package
        for package in cargo_metadata.get("packages", [])
        if package.get("name") == "vm-cell-manager"
    ]
    if len(packages) != 1 or packages[0].get("version") != args.version:
        raise PackageError("package version must equal the exact Cargo source identity")
    binary_bytes = read_regular(Path(args.binary), "binary", MAX_BINARY_BYTES)
    source_inputs = {
        "README.txt": ("packaging/linux/README.txt", "README"),
        "LICENSE.txt": ("LICENSE", "license"),
        "NOTICE.txt": ("NOTICE", "notice"),
        "INSTALL.txt": ("packaging/linux/INSTALL.txt", "install instructions"),
        "vmcell-portable-layout.py": (
            "packaging/linux/vmcell-portable-layout.py",
            "portable layout installer",
        ),
    }
    package_inputs: dict[str, bytes] = {}
    for package_name, (source_name, label) in source_inputs.items():
        content = read_regular(repository_root / source_name, label, MAX_TEXT_BYTES)
        committed = run_bounded(
            ["git", "-C", str(repository_root), "show", f"{args.source_commit}:{source_name}"],
            f"committed {label}",
        ).encode("utf-8")
        if content != committed:
            raise PackageError(f"{label} did not match the declared source commit")
        package_inputs[package_name] = content

    with tempfile.TemporaryDirectory(prefix="vmcell-linux-package-") as temporary:
        staged_binary = Path(temporary) / "vmcell"
        descriptor = os.open(staged_binary, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o700)
        try:
            view = memoryview(binary_bytes)
            while view:
                written = os.write(descriptor, view)
                if written <= 0:
                    raise PackageError("staged binary write did not progress")
                view = view[written:]
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
        os.chmod(staged_binary, 0o755)
        deterministic_env = os.environ.copy()
        deterministic_env.update({"LC_ALL": "C", "LANG": "C", "TZ": "UTC"})
        elf_header = run_bounded(
            ["readelf", "--file-header", str(staged_binary)], "binary ELF identity", deterministic_env
        )
        if "Machine:" not in elf_header or "X86-64" not in elf_header:
            raise PackageError("binary must be an x86_64 ELF executable")
        version_info = run_bounded(
            ["readelf", "--version-info", str(staged_binary)],
            "binary GLIBC symbol identity",
            deterministic_env,
        )
        if glibc_symbol_floor(version_info) != args.glibc_floor:
            raise PackageError("declared GLIBC floor did not match the exact binary")
        version_output = run_bounded([str(staged_binary), "--version"], "binary version", deterministic_env).strip()
        if version_output != f"vmcell {args.version}":
            raise PackageError(f"binary version mismatch: expected vmcell {args.version}")
        bash_completion = run_bounded(
            [str(staged_binary), "completion", "bash"], "Bash completion", deterministic_env
        ).rstrip("\n") + "\n"
        zsh_completion = run_bounded(
            [str(staged_binary), "completion", "zsh"], "Zsh completion", deterministic_env
        ).rstrip("\n") + "\n"
        if "_vmcell" not in bash_completion or "complete" not in bash_completion:
            raise PackageError("Bash completion generation failed")
        if "#compdef vmcell" not in zsh_completion or "_vmcell" not in zsh_completion:
            raise PackageError("Zsh completion generation failed")

    rustc_version = run_bounded(["rustc", "--version"], "rustc version").strip()
    cargo_version = run_bounded(["cargo", "--version"], "cargo version").strip()
    build_glibc_version = run_bounded(["ldd", "--version"], "build glibc version").splitlines()[0]
    rust_host = run_bounded(["rustc", "-vV"], "Rust host identity")
    if (
        not rustc_version.startswith("rustc 1.85.0 (")
        or not cargo_version.startswith("cargo 1.85.0 (")
        or "release: 1.85.0" not in rust_host
        or f"host: {TARGET}" not in rust_host
    ):
        raise PackageError("build provenance requires Rust/Cargo 1.85.0 for the declared target")

    layout_root = f"vmcell-v{args.version}-linux-x86_64"
    archive_name = f"{layout_root}.tar.gz"
    file_names = [
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
    archive_entries = [layout_root, f"{layout_root}/completions"] + [
        f"{layout_root}/{name}" for name in file_names
    ]
    metadata = {
        "archive_name": archive_name,
        "binary_relative_path": "vmcell",
        "checksum_relative_path": "PACKAGE-CONTENTS.sha256",
        "commands": ["vmcell"],
        "completion_relative_paths": ["completions/vmcell.bash", "completions/_vmcell"],
        "install_scope": "user",
        "libc": "glibc",
        "observed_required_glibc": args.glibc_floor,
        "package_identifier": "JerrySkywalker.vmcell",
        "package_name": "VM Cell Manager",
        "package_version": args.version,
        "portable": True,
        "publication_status": "candidate_only",
        "schema_version": 1,
        "target": args.target,
    }
    provenance = {
        "archive_entries": archive_entries,
        "assembly_reproducibility": "byte-identical with identical declared inputs and baseline tools",
        "binary_reproducibility": "not claimed",
        "binary_sha256": sha256(binary_bytes),
        "build_baseline": args.build_baseline,
        "build_glibc_version": build_glibc_version,
        "build_profile": "release",
        "cargo_version": cargo_version,
        "compression": "gzip-9 with zero header timestamp",
        "libc_implementation": "glibc",
        "observed_required_glibc": args.glibc_floor,
        "package": "vmcell",
        "python_version": platform.python_version(),
        "rustc_version": rustc_version,
        "schema_version": 1,
        "source_commit": args.source_commit,
        "source_date_epoch": args.source_date_epoch,
        "tar_format": "ustar",
        "target": args.target,
        "version": args.version,
    }
    payloads = {
        "BUILD-PROVENANCE.json": json_bytes(provenance),
        "INSTALL.txt": package_inputs["INSTALL.txt"],
        "LICENSE.txt": package_inputs["LICENSE.txt"],
        "NOTICE.txt": package_inputs["NOTICE.txt"],
        "PACKAGE-METADATA.json": json_bytes(metadata),
        "README.txt": package_inputs["README.txt"],
        "completions/_vmcell": zsh_completion.encode("utf-8"),
        "completions/vmcell.bash": bash_completion.encode("utf-8"),
        "vmcell": binary_bytes,
        "vmcell-portable-layout.py": package_inputs["vmcell-portable-layout.py"],
    }
    checksum_lines = [f"{sha256(payloads[name])}  {name}\n" for name in sorted(payloads)]
    payloads["PACKAGE-CONTENTS.sha256"] = "".join(checksum_lines).encode("ascii")

    tar_buffer = io.BytesIO()
    with tarfile.open(fileobj=tar_buffer, mode="w", format=tarfile.USTAR_FORMAT) as archive:
        add_tar_directory(archive, layout_root, args.source_date_epoch)
        add_tar_directory(archive, f"{layout_root}/completions", args.source_date_epoch)
        for name in file_names:
            mode = 0o755 if name in {"vmcell", "vmcell-portable-layout.py"} else 0o644
            add_tar_file(
                archive,
                f"{layout_root}/{name}",
                payloads[name],
                mode,
                args.source_date_epoch,
            )
    compressed = io.BytesIO()
    with gzip.GzipFile(filename="", mode="wb", compresslevel=9, fileobj=compressed, mtime=0) as stream:
        stream.write(tar_buffer.getvalue())
    archive_bytes = compressed.getvalue()
    return archive_name, archive_bytes, sha256(archive_bytes), sha256(binary_bytes)


def publish(args: argparse.Namespace) -> dict[str, object]:
    archive_name, archive_bytes, archive_sha, binary_sha = build_archive(args)
    requested = Path(args.output_directory)
    if requested.name in {"", ".", ".."} or requested.exists() or requested.is_symlink():
        raise PackageError("output directory must be one new exact directory")
    parent = requested.parent if str(requested.parent) else Path(".")
    try:
        parent_absolute = parent.absolute()
        parent_canonical = parent.resolve(strict=True)
        parent_status = os.lstat(parent)
    except OSError as error:
        raise PackageError("output parent must be an existing ordinary directory") from error
    if not stat.S_ISDIR(parent_status.st_mode) or stat.S_ISLNK(parent_status.st_mode):
        raise PackageError("output parent must be an ordinary non-symlink directory")
    if parent_absolute != parent_canonical:
        raise PackageError("output parent must not traverse a symlink")
    if parent_status.st_uid != os.geteuid() or stat.S_IMODE(parent_status.st_mode) & 0o022:
        raise PackageError("output parent must be current-user-owned and not group/world writable")
    output = parent_canonical / requested.name
    checksum_name = "SHA256SUMS.txt"
    parent_flags = os.O_RDONLY | os.O_DIRECTORY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    parent_descriptor = os.open(parent_canonical, parent_flags)
    pinned_parent = os.fstat(parent_descriptor)
    if (pinned_parent.st_dev, pinned_parent.st_ino) != (parent_status.st_dev, parent_status.st_ino):
        os.close(parent_descriptor)
        raise PackageError("output parent identity changed before staging")
    try:
        stage = Path(tempfile.mkdtemp(prefix=".vmcell-linux-package-", dir=parent_canonical))
    except BaseException:
        os.close(parent_descriptor)
        raise
    stage_status = os.lstat(stage)
    archive_path = stage / archive_name
    checksum_path = stage / checksum_name
    stage_committed = False
    try:
        with archive_path.open("xb") as stream:
            stream.write(archive_bytes)
            stream.flush()
            os.fsync(stream.fileno())
        with checksum_path.open("x", encoding="ascii", newline="\n") as stream:
            stream.write(f"{archive_sha}  {archive_name}\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.chmod(archive_path, 0o644)
        os.chmod(checksum_path, 0o644)
        os.chmod(stage, 0o755)
        if sha256(archive_path.read_bytes()) != archive_sha:
            raise PackageError("staged archive identity changed before publication")
        if checksum_path.read_text(encoding="ascii") != f"{archive_sha}  {archive_name}\n":
            raise PackageError("staged checksum identity changed before publication")
        current_parent = os.lstat(parent_canonical)
        current_stage = os.lstat(stage)
        if (current_parent.st_dev, current_parent.st_ino) != (
            pinned_parent.st_dev,
            pinned_parent.st_ino,
        ) or (current_stage.st_dev, current_stage.st_ino) != (
            stage_status.st_dev,
            stage_status.st_ino,
        ):
            raise PackageError("output staging identity changed before publication")
        libc = ctypes.CDLL(None, use_errno=True)
        renameat2 = getattr(libc, "renameat2", None)
        if renameat2 is None:
            raise PackageError("atomic no-replace directory publication is unavailable")
        renameat2.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
        renameat2.restype = ctypes.c_int
        if renameat2(
            parent_descriptor,
            os.fsencode(stage.name),
            parent_descriptor,
            os.fsencode(output.name),
            1,
        ) != 0:
            error = ctypes.get_errno()
            raise PackageError(f"atomic no-replace directory publication failed with errno {error}")
        stage_committed = True
    except BaseException:
        if not stage_committed:
            try:
                current_parent = os.lstat(parent_canonical)
                current_stage = os.lstat(stage)
                cleanup_exact = (current_parent.st_dev, current_parent.st_ino) == (
                    pinned_parent.st_dev,
                    pinned_parent.st_ino,
                ) and (current_stage.st_dev, current_stage.st_ino) == (
                    stage_status.st_dev,
                    stage_status.st_ino,
                )
            except OSError:
                cleanup_exact = False
            if cleanup_exact:
                for name in (archive_name, checksum_name):
                    path = stage / name
                    try:
                        path.unlink()
                    except FileNotFoundError:
                        pass
                try:
                    stage.rmdir()
                except OSError:
                    pass
        raise
    finally:
        os.close(parent_descriptor)
    archive_path = output / archive_name
    checksum_path = output / checksum_name
    return {
        "archive_path": str(archive_path),
        "archive_sha256": archive_sha,
        "binary_sha256": binary_sha,
        "checksum_path": str(checksum_path),
        "source_commit": args.source_commit,
        "source_date_epoch": args.source_date_epoch,
    }


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", required=True)
    parser.add_argument("--output-directory", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--source-commit", required=True)
    parser.add_argument("--source-date-epoch", required=True, type=int)
    parser.add_argument("--glibc-floor", required=True)
    parser.add_argument("--target", default=TARGET)
    parser.add_argument("--build-baseline", default=BASELINE)
    return parser.parse_args()


def main() -> int:
    try:
        result = publish(parse_args())
    except (PackageError, OSError, ValueError) as error:
        print(f"package-linux: {error}", file=sys.stderr)
        return 1
    print(json.dumps(result, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
