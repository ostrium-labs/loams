#!/usr/bin/env python3
"""Prints the content of one write_files entry of a cloud-init file.

    write_file.py cloud-init.yaml /etc/loams-byoc/config.env
"""
import sys

import yaml

for f in yaml.safe_load(open(sys.argv[1]))["write_files"]:
    if f["path"] == sys.argv[2]:
        sys.stdout.write(f["content"])
        break
else:
    sys.exit(f"{sys.argv[2]} is not in {sys.argv[1]}")
