#!/usr/bin/env python3
"""Compare release builds on Linux using X11 window mapping, paint, and process-tree PSS."""

import argparse
import ctypes
import os
from pathlib import Path
import re
import signal
import statistics
import subprocess
import tempfile
import time


ROOT = Path(__file__).resolve().parents[1]


class X11PaintProbe:
    def __init__(self):
        x11 = ctypes.CDLL("libX11.so.6")

        def bind(name, result, *args):
            function = getattr(x11, name)
            function.restype = result
            function.argtypes = args
            return function

        ptr = ctypes.c_void_p
        integer = ctypes.c_int
        uint = ctypes.c_uint
        ulong = ctypes.c_ulong
        self.display = bind("XOpenDisplay", ptr, ctypes.c_char_p)(None)
        if not self.display:
            raise RuntimeError("could not connect to the X11 display")
        self.close_display = bind("XCloseDisplay", integer, ptr)
        self.get_image = bind("XGetImage", ptr, ptr, ulong, integer, integer, uint, uint, ulong, integer)
        self.get_pixel = bind("XGetPixel", ulong, ptr, integer, integer)
        self.destroy_image = bind("XDestroyImage", integer, ptr)
        # A window may be briefly unavailable while the compositor maps it.
        error_handler = ctypes.CFUNCTYPE(integer, ptr, ptr)
        self.ignore_error = error_handler(lambda *_: 0)
        bind("XSetErrorHandler", ptr, error_handler)(self.ignore_error)

    def painted(self, window):
        image = self.get_image(self.display, window, 0, 0, 240, 100, ctypes.c_ulong(-1), 2)
        if not image:
            return False
        try:
            # Both apps draw text in this area. A blank startup window has a uniform fill.
            colors = {self.get_pixel(image, x, y) for x in range(4, 240, 4) for y in range(4, 100, 4)}
            return len(colors) >= 12
        finally:
            self.destroy_image(image)


def wait_for_window(process, title, started, probe):
    window = None
    while process.poll() is None and time.monotonic() - started < 15:
        if window is None:
            match = subprocess.run(
                ["xdotool", "search", "--onlyvisible", "--all", "--pid", str(process.pid),
                 "--name", f"^{re.escape(title)}$"],
                capture_output=True,
            )
            if match.returncode == 0 and match.stdout.strip():
                window = int(match.stdout.split()[0])
                mapped = (time.monotonic() - started) * 1000
        if window is not None and probe.painted(window):
            return mapped, (time.monotonic() - started) * 1000
        time.sleep(0.006)
    stage = "painted content" if window is not None else "mapped a window"
    raise RuntimeError(f"{title} never {stage} (exit status {process.poll()})")


def tree_pss(root_pid):
    parents = {}
    pids = {root_pid}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdecimal():
            continue
        try:
            stat = (entry / "stat").read_text().rsplit(")", 1)[1].split()
        except (FileNotFoundError, ProcessLookupError, PermissionError):
            continue
        pid = int(entry.name)
        parents[pid] = int(stat[1])
        if int(stat[2]) == root_pid:
            pids.add(pid)

    while True:
        children = {pid for pid, parent in parents.items() if parent in pids}
        if children <= pids:
            break
        pids |= children

    total = 0
    measured = 0
    for pid in pids:
        try:
            lines = Path(f"/proc/{pid}/smaps_rollup").read_text().splitlines()
        except (FileNotFoundError, ProcessLookupError):
            continue
        for line in lines:
            if line.startswith("Pss:"):
                total += int(line.split()[1])
                measured += 1
                break
    if not measured:
        raise RuntimeError("the app exited before its memory could be measured")
    return total / 1024, measured


def measure(binary, title, probe):
    with tempfile.TemporaryDirectory(prefix="postlite-benchmark-") as profile:
        env = os.environ.copy()
        env.pop("WAYLAND_DISPLAY", None)
        env.pop("WAYLAND_SOCKET", None)
        env.update(GDK_BACKEND="x11", XDG_CONFIG_HOME=profile, XDG_DATA_HOME=profile,
                   XDG_CACHE_HOME=profile)
        with tempfile.TemporaryFile(mode="w+") as log:
            started = time.monotonic()
            process = subprocess.Popen([binary], env=env, stderr=log, start_new_session=True)
            try:
                mapped, painted = wait_for_window(process, title, started, probe)
                time.sleep(2)
                memory, processes = tree_pss(process.pid)
                return mapped, painted, memory, processes
            except RuntimeError as error:
                log.seek(0)
                raise RuntimeError(f"{error}\n{log.read()}") from error
            finally:
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tauri", type=Path, default=ROOT / "src-tauri/target/release/postlite")
    parser.add_argument("--gpui", type=Path, default=ROOT / "src-gpui/target/release/postlite-gpui")
    parser.add_argument("--runs", type=int, default=7)
    args = parser.parse_args()
    if args.runs < 1:
        parser.error("--runs must be positive")
    if not os.environ.get("DISPLAY"):
        parser.error("an X11 display is required")

    targets = [(args.tauri.resolve(), "Postlite"), (args.gpui.resolve(), "Postlite GPUI")]
    samples = {title: [] for _, title in targets}
    probe = X11PaintProbe()
    try:
        for run in range(args.runs + 1):
            for binary, title in targets[:: 1 if run % 2 == 0 else -1]:
                sample = measure(binary, title, probe)
                if run == 0:
                    continue
                samples[title].append(sample)
                mapped, painted, pss, processes = sample
                print(f"{run:>2}  {title:13}  {mapped:7.1f} ms mapped  {painted:7.1f} ms painted  "
                      f"{pss:7.1f} MiB PSS  ({processes} processes)", flush=True)
    finally:
        probe.close_display(probe.display)

    for title, values in samples.items():
        mapped, painted, pss, _ = map(statistics.median, zip(*values))
        print(f"Median {title:13}  {mapped:7.1f} ms mapped  {painted:7.1f} ms painted  {pss:7.1f} MiB PSS")


if __name__ == "__main__":
    main()
