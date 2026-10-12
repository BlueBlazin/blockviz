// Package modern uses syntax from Go 1.18 through 1.24.
package modern

import (
	"cmp"
	"iter"
	"maps"
	"slices"
)

// Number is any numeric type.
type Number interface {
	~int | ~int64 | ~float64
}

// Set is a generic type alias (Go 1.24).
type Set[T comparable] = map[T]struct{}

// List is a generic list.
type List[T any] struct {
	items []T
}

// Push appends to the list.
func (l *List[T]) Push(v T) { l.items = append(l.items, v) }

// All iterates over the list (range over func, Go 1.23).
func (l *List[T]) All() iter.Seq2[int, T] {
	return func(yield func(int, T) bool) {
		for i, v := range l.items {
			if !yield(i, v) {
				return
			}
		}
	}
}

// Sum adds numbers.
func Sum[N Number](xs ...N) (total N) {
	for _, x := range xs {
		total += x
	}
	return total
}

// Map applies f to every element.
func Map[S ~[]E, E, R any](s S, f func(E) R) []R {
	out := make([]R, 0, len(s))
	for _, v := range s {
		out = append(out, f(v))
	}
	return out
}

// Pair holds a key and a value.
type Pair[K cmp.Ordered, V any] struct {
	Key K
	Val V
}

// Sorted returns a map's pairs in key order.
func Sorted[K cmp.Ordered, V any](m map[K]V) []Pair[K, V] {
	keys := slices.Sorted(maps.Keys(m))
	out := make([]Pair[K, V], 0, len(keys))
	for _, k := range keys {
		out = append(out, Pair[K, V]{Key: k, Val: m[k]})
	}
	return out
}

// Countdown ranges over an integer (Go 1.22) and uses min, max and clear (Go 1.21).
func Countdown(n int, seen map[int]bool) []int {
	var out []int
	for i := range max(n, 0) {
		out = append(out, n-i)
	}
	clear(seen)
	return out[:min(len(out), 10)]
}
