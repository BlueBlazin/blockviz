# 3.14: template strings (PEP 750), unparenthesized except types (PEP 758)
def render(name, value):
    template = t"Hello {name}, you have {value:,} points"
    return template


def parse(text):
    try:
        return int(text)
    except ValueError, TypeError:
        return render(text, 0)
