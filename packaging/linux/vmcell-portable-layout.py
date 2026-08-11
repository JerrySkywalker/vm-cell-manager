#!/usr/bin/env python3
"""Install or remove one exact vmcell portable-package layout."""

from __future__ import annotations

import argparse
import ctypes
import hashlib
import os
from pathlib import Path, PurePosixPath
import secrets
import stat
import sys


MAX_FILE_BYTES = 256 * 1024 * 1024
RENAME_NOREPLACE = 1  # Linux renameat2 flag.
FILE_MODES = {
    "BUILD-PROVENANCE.json": 0o644,
    "INSTALL.txt": 0o644,
    "LICENSE.txt": 0o644,
    "NOTICE.txt": 0o644,
    "PACKAGE-CONTENTS.sha256": 0o644,
    "PACKAGE-METADATA.json": 0o644,
    "README.txt": 0o644,
    "completions/_vmcell": 0o644,
    "completions/vmcell.bash": 0o644,
    "vmcell": 0o755,
    "vmcell-portable-layout.py": 0o755,
}


class LayoutError(RuntimeError):
    pass


def sha256(content: bytes) -> str:
    return hashlib.sha256(content).hexdigest()


def read_regular_at(directory: int, name: str, mode: int) -> tuple[bytes, tuple[int, int]]:
    flags = os.O_RDONLY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(name, flags, dir_fd=directory)
    try:
        opened = os.fstat(descriptor)
        current = os.stat(name, dir_fd=directory, follow_symlinks=False)
        if (
            not stat.S_ISREG(opened.st_mode)
            or stat.S_ISLNK(current.st_mode)
            or opened.st_uid != os.geteuid()
            or stat.S_IMODE(opened.st_mode) != mode
            or (opened.st_dev, opened.st_ino) != (current.st_dev, current.st_ino)
            or opened.st_size < 1
            or opened.st_size > MAX_FILE_BYTES
        ):
            raise LayoutError(f"package file identity or mode was invalid: {name}")
        chunks: list[bytes] = []
        remaining = opened.st_size
        while remaining:
            chunk = os.read(descriptor, min(remaining, 1024 * 1024))
            if not chunk:
                raise LayoutError(f"package file changed while read: {name}")
            chunks.append(chunk)
            remaining -= len(chunk)
        if os.read(descriptor, 1):
            raise LayoutError(f"package file exceeded its opened size: {name}")
        after = os.fstat(descriptor)
        current_after = os.stat(name, dir_fd=directory, follow_symlinks=False)
        if (
            (opened.st_dev, opened.st_ino, opened.st_size)
            != (after.st_dev, after.st_ino, after.st_size)
            or (after.st_dev, after.st_ino)
            != (current_after.st_dev, current_after.st_ino)
        ):
            raise LayoutError(f"package file identity changed while read: {name}")
        return b"".join(chunks), (opened.st_dev, opened.st_ino)
    finally:
        os.close(descriptor)


def open_exact_directory_at(directory: int, name: str, mode: int) -> int:
    flags = os.O_RDONLY | os.O_DIRECTORY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(name, flags, dir_fd=directory)
    opened = os.fstat(descriptor)
    current = os.stat(name, dir_fd=directory, follow_symlinks=False)
    if (
        not stat.S_ISDIR(opened.st_mode)
        or stat.S_ISLNK(current.st_mode)
        or opened.st_uid != os.geteuid()
        or stat.S_IMODE(opened.st_mode) != mode
        or (opened.st_dev, opened.st_ino) != (current.st_dev, current.st_ino)
    ):
        os.close(descriptor)
        raise LayoutError(f"directory identity or mode was invalid: {name}")
    return descriptor


def create_exact_directory_at(directory: int, name: str, mode: int) -> int:
    os.mkdir(name, 0o700, dir_fd=directory)
    flags = os.O_RDONLY | os.O_DIRECTORY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(name, flags, dir_fd=directory)
    try:
        opened = os.fstat(descriptor)
        current = os.stat(name, dir_fd=directory, follow_symlinks=False)
        if (
            not stat.S_ISDIR(opened.st_mode)
            or stat.S_ISLNK(current.st_mode)
            or opened.st_uid != os.geteuid()
            or (opened.st_dev, opened.st_ino) != (current.st_dev, current.st_ino)
        ):
            raise LayoutError(f"created directory identity was invalid: {name}")
        os.fchmod(descriptor, mode)
        if stat.S_IMODE(os.fstat(descriptor).st_mode) != mode:
            raise LayoutError(f"created directory mode could not be normalized: {name}")
        os.fsync(descriptor)
        return descriptor
    except BaseException:
        os.close(descriptor)
        raise


def exact_names(directory: int) -> set[str]:
    return {entry.name for entry in os.scandir(directory)}


def validate_layout(directory: int, *, root_mode: int) -> tuple[dict[str, bytes], dict[str, tuple[int, int]]]:
    opened = os.fstat(directory)
    if not stat.S_ISDIR(opened.st_mode) or opened.st_uid != os.geteuid() or stat.S_IMODE(opened.st_mode) != root_mode:
        raise LayoutError("package root ownership or mode was invalid")
    expected_root = {PurePosixPath(name).parts[0] for name in FILE_MODES}
    expected_root.add("completions")
    if exact_names(directory) != expected_root:
        raise LayoutError("package root contained missing or foreign entries")
    completions = open_exact_directory_at(directory, "completions", 0o755)
    try:
        expected_completions = {
            PurePosixPath(name).name for name in FILE_MODES if name.startswith("completions/")
        }
        if exact_names(completions) != expected_completions:
            raise LayoutError("completion directory contained missing or foreign entries")
        contents: dict[str, bytes] = {}
        identities: dict[str, tuple[int, int]] = {}
        for name, mode in FILE_MODES.items():
            parent = completions if name.startswith("completions/") else directory
            leaf = PurePosixPath(name).name
            contents[name], identities[name] = read_regular_at(parent, leaf, mode)
    finally:
        os.close(completions)
    try:
        lines = contents["PACKAGE-CONTENTS.sha256"].decode("ascii", errors="strict").splitlines()
    except UnicodeDecodeError as error:
        raise LayoutError("package content manifest was not ASCII") from error
    expected_manifest = sorted(name for name in FILE_MODES if name != "PACKAGE-CONTENTS.sha256")
    if len(lines) != len(expected_manifest):
        raise LayoutError("package content manifest entry count changed")
    for line, name in zip(lines, expected_manifest, strict=True):
        parts = line.split("  ", 1)
        if len(parts) != 2 or parts[1] != name or parts[0] != sha256(contents[name]):
            raise LayoutError(f"package content manifest mismatch: {name}")
    return contents, identities


def open_source() -> tuple[int, Path, dict[str, bytes]]:
    source = Path(__file__).absolute().parent
    status = os.lstat(source)
    if stat.S_ISLNK(status.st_mode) or not stat.S_ISDIR(status.st_mode) or status.st_uid != os.geteuid():
        raise LayoutError("source layout must be an ordinary current-user-owned directory")
    if stat.S_IMODE(status.st_mode) != 0o755:
        raise LayoutError("source layout mode must be 0755")
    flags = os.O_RDONLY | os.O_DIRECTORY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(source, flags)
    opened = os.fstat(descriptor)
    current = os.lstat(source)
    if (opened.st_dev, opened.st_ino) != (current.st_dev, current.st_ino):
        os.close(descriptor)
        raise LayoutError("source layout identity changed")
    contents, _ = validate_layout(descriptor, root_mode=0o755)
    return descriptor, source, contents


def open_private_parent(requested: Path, *, create_missing: bool) -> int:
    home_text = os.environ.get("HOME", "")
    if not home_text or not requested.is_absolute():
        raise LayoutError("install parent and HOME must be absolute")
    home = Path(home_text)
    try:
        relative = requested.relative_to(home)
    except ValueError as error:
        raise LayoutError("install parent must be within HOME") from error
    if not relative.parts or any(part in {"", ".", ".."} for part in relative.parts):
        raise LayoutError("install parent must be a named HOME descendant")
    home_status = os.lstat(home)
    if (
        stat.S_ISLNK(home_status.st_mode)
        or not stat.S_ISDIR(home_status.st_mode)
        or home_status.st_uid != os.geteuid()
        or stat.S_IMODE(home_status.st_mode) & 0o022
    ):
        raise LayoutError("HOME must be ordinary, current-user-owned, and not group/world writable")
    flags = os.O_RDONLY | os.O_DIRECTORY | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    current = os.open(home, flags)
    try:
        for index, component in enumerate(relative.parts):
            if create_missing:
                try:
                    os.mkdir(component, 0o700, dir_fd=current)
                except FileExistsError:
                    pass
            child = os.open(component, flags, dir_fd=current)
            opened = os.fstat(child)
            named = os.stat(component, dir_fd=current, follow_symlinks=False)
            leaf = index == len(relative.parts) - 1
            if (
                not stat.S_ISDIR(opened.st_mode)
                or stat.S_ISLNK(named.st_mode)
                or opened.st_uid != os.geteuid()
                or stat.S_IMODE(opened.st_mode) & 0o022
                or (leaf and stat.S_IMODE(opened.st_mode) != 0o700)
                or (opened.st_dev, opened.st_ino) != (named.st_dev, named.st_ino)
            ):
                os.close(child)
                raise LayoutError(f"install parent component was not admitted: {component}")
            os.close(current)
            current = child
        return current
    except BaseException:
        os.close(current)
        raise


def write_new(directory: int, name: str, content: bytes, mode: int) -> None:
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_CLOEXEC", 0) | getattr(os, "O_NOFOLLOW", 0)
    descriptor = os.open(name, flags, 0o600, dir_fd=directory)
    try:
        view = memoryview(content)
        while view:
            written = os.write(descriptor, view)
            if written <= 0:
                raise LayoutError(f"installed file write did not progress: {name}")
            view = view[written:]
        os.fchmod(descriptor, mode)
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def rename_noreplace(directory: int, source: str, destination: str) -> None:
    libc = ctypes.CDLL(None, use_errno=True)
    renameat2 = getattr(libc, "renameat2", None)
    if renameat2 is None:
        raise LayoutError("atomic no-clobber installation requires renameat2")
    renameat2.argtypes = [ctypes.c_int, ctypes.c_char_p, ctypes.c_int, ctypes.c_char_p, ctypes.c_uint]
    renameat2.restype = ctypes.c_int
    if renameat2(directory, os.fsencode(source), directory, os.fsencode(destination), RENAME_NOREPLACE) != 0:
        error = ctypes.get_errno()
        raise LayoutError(f"fresh installation target could not be published: errno {error}")


def install(parent: int, target: str, contents: dict[str, bytes]) -> None:
    stage = f".vmcell-install-{secrets.token_hex(12)}"
    os.mkdir(stage, 0o700, dir_fd=parent)
    stage_descriptor = open_exact_directory_at(parent, stage, 0o700)
    published = False
    try:
        completions = create_exact_directory_at(stage_descriptor, "completions", 0o755)
        try:
            for name, mode in FILE_MODES.items():
                destination = completions if name.startswith("completions/") else stage_descriptor
                write_new(destination, PurePosixPath(name).name, contents[name], mode)
            os.fsync(completions)
        finally:
            os.close(completions)
        os.fsync(stage_descriptor)
        validate_layout(stage_descriptor, root_mode=0o700)
        rename_noreplace(parent, stage, target)
        published = True
    finally:
        os.close(stage_descriptor)
        if not published:
            try:
                cleanup_layout(parent, stage, require_valid=False)
            except (OSError, LayoutError):
                pass


def cleanup_layout(
    parent: int,
    target: str,
    *,
    require_valid: bool,
    expected_contents: dict[str, bytes] | None = None,
    forbidden_root_identity: tuple[int, int] | None = None,
) -> None:
    root = open_exact_directory_at(parent, target, 0o700)
    try:
        opened_root = os.fstat(root)
        if forbidden_root_identity == (opened_root.st_dev, opened_root.st_ino):
            raise LayoutError("removal source must be independent of the installed target")
        if require_valid:
            installed_contents, _ = validate_layout(root, root_mode=0o700)
            if expected_contents is None or installed_contents != expected_contents:
                raise LayoutError("installed layout did not match the retained verified package")
        completions = open_exact_directory_at(root, "completions", 0o755)
        try:
            for name in sorted(FILE_MODES):
                directory = completions if name.startswith("completions/") else root
                leaf = PurePosixPath(name).name
                if not require_valid:
                    try:
                        status = os.stat(leaf, dir_fd=directory, follow_symlinks=False)
                    except FileNotFoundError:
                        continue
                    if not stat.S_ISREG(status.st_mode) or status.st_uid != os.geteuid():
                        raise LayoutError("partial installation contained a foreign entry")
                os.unlink(leaf, dir_fd=directory)
        finally:
            os.close(completions)
        os.rmdir("completions", dir_fd=root)
    finally:
        os.close(root)
    os.rmdir(target, dir_fd=parent)
    os.fsync(parent)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("action", choices=("install", "remove"))
    parser.add_argument("--parent", required=True)
    args = parser.parse_args()
    source_descriptor, source, contents = open_source()
    try:
        source_status = os.fstat(source_descriptor)
        source_identity = (source_status.st_dev, source_status.st_ino)
        target = source.name
        if not target.startswith("vmcell-v") or not target.endswith("-linux-x86_64"):
            raise LayoutError("source layout name was not a versioned vmcell package")
        parent = open_private_parent(
            Path(args.parent),
            create_missing=args.action == "install",
        )
        try:
            if args.action == "install":
                install(parent, target, contents)
            else:
                cleanup_layout(
                    parent,
                    target,
                    require_valid=True,
                    expected_contents=contents,
                    forbidden_root_identity=source_identity,
                )
        finally:
            os.close(parent)
    finally:
        os.close(source_descriptor)
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (LayoutError, OSError, ValueError) as error:
        print(f"vmcell-portable-layout: {error}", file=sys.stderr)
        raise SystemExit(1)
