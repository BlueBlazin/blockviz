package generics

import "golang.org/x/exp/constraints"

type Number interface {
	~int | ~int64 | ~float64
}

type Ordered interface {
	constraints.Integer | constraints.Float | ~string
}

type Stringish interface {
	~string
	String() string
}

type Union interface{ int | string }

type Comparable interface {
	comparable
}

type List[T any] struct {
	head *node[T]
	size int
}

type node[T any] struct {
	value T
	next  *node[T]
}

type Pair[K comparable, V any] struct {
	Key   K
	Value V
}

type Tree[T interface{ Less(T) bool }] struct {
	left, right *Tree[T]
}

type Matrix[T Number, U ~[]T] [][]T

// Ambiguities between array lengths and type parameter lists.
type (
	ArrayN    [N]int
	ArrayExpr [N * 2]int
	PtrConstr[P *Q,] struct{}
	PtrLit[P *[]int] struct{}
	PtrStruct[P *struct{}] []P
	ParenLit[P ([]int)] struct{}
	SliceConstr[P []int] struct{}
	MapConstr[P map[string]int] struct{}
	Several[P, Q any, R interface{}] struct{}
	Tilde[P ~int] struct{}
	TildeUnion[P ~int | ~string] struct{}
	NameUnion[P int | string] struct{}
	FuncConstr[P func()] struct{}
	ChanConstr[P chan int] struct{}
	Inst[P Pair[int, string]] struct{}
)

const N = 2

type Q int

// Generic type aliases (Go 1.24).
type Alias[T any] = List[T]

type AliasPair[K comparable, V any] = Pair[K, V]

type Embeds[T any] struct {
	List[T]
	*node[T]
	Pair[string, T]
	generics.Pair[int, T]
}

type EmbedsIface interface {
	Pair[int, string]
	generics.Pair[int, string]
	Number
}

func (l *List[T]) Push(v T) {
	l.head = &node[T]{value: v, next: l.head}
	l.size++
}

func (l *List[_]) Len() int { return l.size }

func (p Pair[K, V]) Swap() Pair[V, K] { return Pair[V, K]{p.Value, p.Key} }

func Map[S ~[]E, E, R any](s S, f func(E) R) []R {
	out := make([]R, 0, len(s))
	for _, v := range s {
		out = append(out, f(v))
	}
	return out
}

func Max[T constraints.Ordered](a, b T) T {
	if a > b {
		return a
	}
	return b
}

func Zero[T any]() (zero T) { return }

func Keys[M ~map[K]V, K comparable, V any](m M) []K { return nil }

func use() {
	_ = Max[int](1, 2)
	_ = Max(1.5, 2.5)
	_ = Map[[]int, int, string](nil, nil)
	_ = Pair[int, string]{1, "a"}
	_ = &List[int]{}
	var p Pair[string, []int]
	_ = p
	f := Zero[float64]
	_ = f
	g := Map[[]int, int, int]
	_ = g
	var a []int
	var b []int
	_ = a[b[0]](0)
	_ = generics.Pair[int, int]{}
	_ = Keys[map[string]int]
	type local[T any] []T
	var _ local[int]
}
