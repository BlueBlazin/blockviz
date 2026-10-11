# 3.8: positional-only parameters, walrus, f-string `=`; 3.9: relaxed decorators
import functools

buttons = [functools]

@buttons[0].cache
def scale(values, /, factor, *, clamp=None):
    if (n := len(values)) > 3:
        print(f"{n=}")
    return [v * factor for v in values]
