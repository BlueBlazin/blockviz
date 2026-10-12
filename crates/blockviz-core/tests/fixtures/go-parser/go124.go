// Syntax newer than the Go 1.18 oracle: checked by hand in go_parser.rs.
package go124

type C int

// A pointer constraint in a union is a type parameter list (Go 1.20+).
type PtrUnion[P *C | ~int] struct{}

type Set[T ~int | ~string, PT *T] []T

// A generic alias (Go 1.24).
type Vec[T any] = []T

func neg(x int) int {
	// `~` parses as a unary operator; the type checker rejects it here.
	return ~x
}

func iter(yield func(int) bool) {
	for i := range 3 {
		if !yield(i) {
			return
		}
	}
}

func use() {
	for v := range iter {
		_ = v
	}
}
