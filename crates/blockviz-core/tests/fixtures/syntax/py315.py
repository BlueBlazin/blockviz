# 3.15: explicit lazy imports (PEP 810), unpacking in comprehensions (PEP 798),
# unary plus in match patterns. Forms taken from CPython 3.15's own tests.
lazy import json
lazy from os import path


def flatten(lists, dicts):
    items = [*xs for xs in lists]
    merged = {**d for d in dicts}
    unique = {*xs for xs in lists}
    return items, merged, unique


def sign(x):
    match x:
        case +0:
            return 0
        case _:
            return json.dumps(path.sep)
