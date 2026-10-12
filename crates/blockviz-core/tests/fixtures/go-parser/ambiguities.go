package ambiguities

type T struct{ x int }

type U []int

func (T) M() {}

func (*T) P() {}

func f(x interface{}, c chan int, p *T, s []int, m map[T]int) {
	// `{` after a header expression opens the body, not a composite
	// literal, unless the literal is parenthesized or its type is a type
	// literal.
	if x == nil {
	}
	if (T{}) == *p {
	}
	for _, v := range []T{{1}, {2}} {
		_ = v
	}
	for i := range [3]int{} {
		_ = i
	}
	for k := range map[string]int{"a": 1} {
		_ = k
	}
	switch (T{}).x {
	}
	switch s := (U{1}); len(s) {
	}
	if v := (U{}); len(v) == 0 {
	}
	for x := (T{}); x.x < 3; x.x++ {
	}
	if f := func() bool { return T{}.x == 0 }; f() {
	}
	if struct{}{} == struct{}{} {
	}
	_ = m[T{1}]

	// Conversions and method expressions.
	_ = (*T)(p)
	_ = (*T).P
	_ = T.M
	_ = []int(s)
	_ = (func(int))(nil)
	_ = (chan int)(c)
	_ = <-chan int(nil)
	_ = (<-chan int)(c)
	_ = interface{ M() }(T{})
	_ = map[T]int(m)
	_ = [2]int{}

	// Index, then call; instantiation, then call.
	var fs []func(int) int
	_ = fs[0](1)
	_ = s[s[0]]

	// Type assertions and type switches.
	_ = x.(T)
	_, _ = x.(T)
	switch x.(type) {
	}
	switch y := x.(type) {
	case T, *T:
		_ = y
	}

	// Stars: dereference or pointer type.
	*p = T{}
	_ = *p
	_ = **&p
	var pp **T = &p
	_ = pp

	// Unary and receive.
	_ = <-c
	c <- <-c
	_ = -<-c
	_ = !(x == nil)
	_ = ^0

	// Selectors on literals and calls.
	_ = T{}.x
	_ = (&T{}).x
	_ = f
	_ = "abc"[1]
	_ = len("abc")
	_ = [...]string{"a"}[0]
}

func labels() {
L:
	{
	}
M:
	if true {
	}
N:
	;
	goto L
	goto M
	goto N
}

func receive(c chan chan int) {
	<-<-c
	_ = <-<-c
}
