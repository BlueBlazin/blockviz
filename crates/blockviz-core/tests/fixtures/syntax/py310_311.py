# 3.10: structural pattern matching, parenthesized context managers
# 3.11: except*, star expressions in subscripts and annotations
from typing import TypeVarTuple

Ts = TypeVarTuple("Ts")


def route(command, reader, writer):
    with (open(reader) as src, open(writer, "w") as dst):
        match command:
            case {"action": "copy", **rest}:
                dst.write(src.read())
            case [first, *others] if first:
                return others
            case str() | bytes():
                return command
            case _:
                return None


def grouped(*args: *Ts):
    try:
        return args[*range(2)]
    except* ValueError as group:
        raise group
