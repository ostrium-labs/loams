"""The Python half of the Loams Temporal history corpus (lane TSDK-python).

`python -m corpus_py generate|worker|start|replay`: see __main__.py.
"""

# The workflow ID prefix and task queue of every Python scenario:
# corpus-python-<scenario> on corpus-python (Go: corpus-go-, TS: corpus-ts-).
LANG = "python"
PREFIX = f"corpus-{LANG}-"
QUEUE = f"corpus-{LANG}"
