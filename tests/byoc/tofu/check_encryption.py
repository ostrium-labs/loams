#!/usr/bin/env python3
"""example_with_keys_enforces_encryption (plan CC1 Task 7, D1827).

Every root module under deploy/tofu/examples that can put a Confluent key in state
(it passes create_api_keys to the confluent module, or sets kafka_admin to
module_key) must have an `encryption` block whose `state` and `plan` blocks both
say `enforced = true`. A crude HCL reader (comments stripped, braces counted) is
enough for the examples' own style. Exit 1 and name the file on a failure.
"""
import pathlib
import re
import sys


def strip_comments(text: str) -> str:
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.S)
    return "\n".join(re.split(r"\s(#|//)", " " + line, maxsplit=1)[0] for line in text.splitlines())


def block_bodies(text: str, header: str):
    """Yield the bodies of every `<header> {` block (header is a regex)."""
    for m in re.finditer(header + r"\s*\{", text):
        depth, i = 1, m.end()
        while depth and i < len(text):
            depth += {"{": 1, "}": -1}.get(text[i], 0)
            i += 1
        yield text[m.end(): i - 1]


def main(root: str) -> int:
    fail = 0
    examples = sorted(p for p in pathlib.Path(root).iterdir() if p.is_dir())
    for ex in examples:
        text = "\n".join(strip_comments(f.read_text()) for f in sorted(ex.glob("*.tf")))
        needs = re.search(r"create_api_keys\s*=", text) or re.search(r'kafka_admin\s*=\s*"module_key"', text)
        if not needs:
            continue
        enc = list(block_bodies(text, r"\bencryption"))
        ok = False
        for body in enc:
            states = list(block_bodies(body, r"\bstate"))
            plans = list(block_bodies(body, r"\bplan"))
            if states and plans and all(re.search(r"enforced\s*=\s*true", b) for b in states + plans):
                ok = True
        if ok:
            print(f"ok   example_with_keys_enforces_encryption: {ex.name}")
        else:
            print(f"FAIL example_with_keys_enforces_encryption: {ex.name} can create a Confluent key but its state and plan encryption is not enforced = true")
            fail = 1
    return fail


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
