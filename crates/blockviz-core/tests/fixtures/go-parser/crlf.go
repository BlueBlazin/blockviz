// Package crlf has Windows line endings throughout.
package crlf

/* A block comment
   over two lines. */
// A line comment.
type T struct {
	A int // line comment
	B string `tag:"x"`
}

const raw = `line one
line two`

func f() int {
	x := 1 // trailing
	if x > 0 {
		return x
	}
	return 0
}
