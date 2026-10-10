#!/usr/bin/env python3
"""Own an isolated local mock for the network suite; fail if tests were skipped."""
import os
from pathlib import Path
import subprocess
import tempfile
import time
from urllib.error import URLError
from urllib.request import urlopen
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]


def main():
    cache = ROOT / ".cache"
    cache.mkdir(exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="conformance-", dir=cache) as directory:
        binary = Path(directory) / "loams-mock"
        subprocess.run(["go", "build", "-trimpath", "-buildvcs=false", "-o", str(binary), "./cmd/loams-mock"],
                       cwd=ROOT / "mock", env={**os.environ, "CGO_ENABLED": "0"}, check=True, timeout=180)
        with (cache / "conformance.log").open("w") as log:
            mock = subprocess.Popen([str(binary), "-listen", "127.0.0.1:8084",
                                     "-public-url", "http://127.0.0.1:8084", "-qr=false", "-heartbeat", "2s"],
                                    cwd=ROOT / "mock", stdout=log, stderr=subprocess.STDOUT)
            try:
                for _ in range(30):
                    if mock.poll() is not None:
                        raise RuntimeError("Mock exited; check apps/mobile/.cache/conformance.log (port 8084 must be free)")
                    try:
                        with urlopen("http://127.0.0.1:8084/healthz", timeout=1) as response:
                            if response.status == 200:
                                time.sleep(0.2)
                                if mock.poll() is not None:
                                    raise RuntimeError("Mock exited during startup; port 8084 must be free")
                                break
                    except URLError:
                        pass
                    time.sleep(1)
                else:
                    raise RuntimeError("Mock health check timed out")
                subprocess.run(["./gradlew", "--no-daemon", "--stacktrace", ":conformance:test", "--rerun-tasks"],
                               cwd=ROOT / "android", env={**os.environ, "LOAMS_MOCK_URL": "http://127.0.0.1:8084"},
                               check=True, timeout=1200)
                reports = list((ROOT / "android/conformance/build/test-results/test").glob("TEST-*.xml"))
                suites = [ET.parse(p).getroot() for p in reports]
                if not suites or sum(int(s.get("tests", 0)) for s in suites) == 0:
                    raise RuntimeError("Conformance produced no test results")
                if any(int(s.get(k, 0)) for s in suites for k in ("skipped", "failures", "errors")):
                    raise RuntimeError("Conformance had skipped or failing tests")
            finally:
                mock.terminate()
                try:
                    mock.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    mock.kill()
                    mock.wait()


if __name__ == "__main__":
    main()
