#!/usr/bin/env python3
"""Unsigned native iOS targets; intentionally restricted to macOS."""
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1] / "ios"


def run(args, timeout=1800):
    subprocess.run(args, cwd=ROOT, check=True, timeout=timeout)


def main():
    if sys.platform != "darwin":
        raise SystemExit("Native iOS generation/build/test requires macOS, Xcode and XcodeGen")
    action = sys.argv[1]
    run(["xcodegen", "generate"], 120)
    if action == "generate":
        return
    if action == "test":
        for package in ("LoamsCore", "LoamsData"):
            run(["swift", "test", "--package-path", f"Packages/{package}"])
    destination = os.environ.get("LOAMS_IOS_DESTINATION")
    if not destination:
        devices = json.loads(subprocess.check_output(
            ["xcrun", "simctl", "list", "devices", "available", "-j"], cwd=ROOT))["devices"]
        phones = [d for runtime in sorted(devices, reverse=True) if "iOS" in runtime
                  for d in devices[runtime] if d["name"].startswith("iPhone")]
        if not phones:
            raise SystemExit("No available iPhone simulator; install an iOS simulator runtime")
        destination = "id=" + phones[0]["udid"]
    run(["xcodebuild", "-project", "Loams.xcodeproj", "-scheme", "Loams",
         "-destination", destination, "-configuration", "Debug", "-derivedDataPath", "build",
         "CODE_SIGNING_ALLOWED=NO", "-skipPackagePluginValidation", action])


if __name__ == "__main__":
    main()
