#!/usr/bin/env python3

import os
import secrets
import stat
import sys


class ReportError(Exception):
    pass


def open_report_root(root: str) -> int:
    flags = os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW | os.O_CLOEXEC
    current = os.open(os.path.sep, flags)
    try:
        for component in root.removeprefix(os.path.sep).split(os.path.sep):
            if not component:
                continue
            try:
                os.mkdir(component, mode=0o700, dir_fd=current)
            except FileExistsError:
                pass
            following = os.open(component, flags, dir_fd=current)
            os.close(current)
            current = following
        return current
    except Exception:
        os.close(current)
        raise ReportError(
            f"report root must have only real directory ancestors: {root}"
        ) from None


def write_report(root: str, output: str) -> None:
    name = os.path.basename(output)
    if not name or output != os.path.join(root, name):
        raise ReportError(f"report output must be a direct child of {root}")

    root_descriptor = open_report_root(root)
    temporary = f".{name}.{os.getpid()}.{secrets.token_hex(16)}.tmp"
    try:
        try:
            metadata = os.stat(name, dir_fd=root_descriptor, follow_symlinks=False)
        except FileNotFoundError:
            metadata = None
        if metadata is not None and not stat.S_ISREG(metadata.st_mode):
            raise ReportError(
                f"report output must be absent or a regular file: {output}"
            )

        print("READY", flush=True)
        descriptor = os.open(
            temporary,
            os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW | os.O_CLOEXEC,
            0o600,
            dir_fd=root_descriptor,
        )
        try:
            with os.fdopen(descriptor, "wb", closefd=False) as report:
                while chunk := sys.stdin.buffer.read(64 * 1024):
                    report.write(chunk)
                report.flush()
                os.fsync(descriptor)
        finally:
            os.close(descriptor)

        os.replace(
            temporary,
            name,
            src_dir_fd=root_descriptor,
            dst_dir_fd=root_descriptor,
        )
        os.fsync(root_descriptor)
    except Exception:
        try:
            os.unlink(temporary, dir_fd=root_descriptor)
        except FileNotFoundError:
            pass
        raise
    finally:
        os.close(root_descriptor)


def main() -> int:
    if len(sys.argv) != 3:
        print("usage: private-report-writer.py ROOT OUTPUT", file=sys.stderr)
        return 2
    try:
        write_report(os.path.abspath(sys.argv[1]), os.path.abspath(sys.argv[2]))
        return 0
    except (OSError, ReportError) as error:
        print(error, file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
