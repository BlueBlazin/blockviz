# 3.12: PEP 695 type parameters and aliases, PEP 701 f-strings
# 3.13: type parameter defaults
type Pair[T] = tuple[T, T]
type Table[K = str, V = int] = dict[K, V]


class Box[T: (int, str) = int, *Ts, **P]:
    def __init__(self, item: T) -> None:
        self.item = item


def first[T](items: list[T]) -> T:
    return items[0]


def describe(songs):
    joined = f"{", ".join(songs)}"
    nested = f"{f"{f"{len(songs) + 1}"}"}"
    multiline = f"{
        songs[0]  # a comment inside the replacement field
    }"
    escaped = f"{'\n'.join(songs)}"
    return first([joined, nested, multiline, escaped])
