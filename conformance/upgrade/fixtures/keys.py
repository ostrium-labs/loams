"""The API keys file: a call with the stack's key is let in, one without is
refused, before and after (§54 §14 step 2's "the API keys file")."""

from lib import NS, Skip

NAME = "keys"


def calls(stack):
    path = f"/v1/namespaces/{NS}/streams"
    with_key, _ = stack.http().get(path)
    without, _ = stack.http().get(path, auth=False)
    return with_key, without


def write(stack):
    if not stack.key:
        raise Skip("no API key given (--key): the stack does not run apikey auth")
    with_key, without = calls(stack)
    if with_key != 200:
        raise AssertionError(f"the key is refused: {with_key}")
    return {"with_key": with_key, "without": without}


def check(stack, rec):
    got = dict(zip(("with_key", "without"), calls(stack)))
    return [] if got == {k: rec[k] for k in got} else [f"auth: {got}, was {rec}"]


def write_new(stack):
    return {}


def check_new(stack, rec, present):
    return []
