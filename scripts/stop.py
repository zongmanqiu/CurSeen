"""Stops CurSeen process"""

import os
import signal
import subprocess
from pathlib import Path


def main():
    pid_file = Path(os.environ.get("APPDATA", ".")) / "CurSeen" / "curseen.pid"

    if pid_file.exists():
        pid = int(pid_file.read_text().strip())
        try:
            os.kill(pid, signal.SIGTERM)
            print(f"CurSeen (PID {pid}) stopped")
            pid_file.unlink(missing_ok=True)
            return
        except (OSError, ProcessLookupError):
            pid_file.unlink(missing_ok=True)

    # Fallback: find python(w).exe running curseen
    for proc_name in ("pythonw.exe", "python.exe"):
        try:
            result = subprocess.run(
                ["tasklist", "/fi", f"IMAGENAME eq {proc_name}", "/fo", "csv", "/nh"],
                capture_output=True, text=True, timeout=5
            )
            for line in result.stdout.strip().splitlines():
                if "curseen" in line.lower():
                    parts = line.split(",")
                    if len(parts) >= 2:
                        pid_str = parts[1].strip('"')
                        subprocess.run(["taskkill", "/f", "/pid", pid_str],
                                       capture_output=True, timeout=5)
                        print(f"Killed {proc_name} (PID {pid_str})")
        except Exception:
            pass

    print("CurSeen stopped")


if __name__ == "__main__":
    main()
