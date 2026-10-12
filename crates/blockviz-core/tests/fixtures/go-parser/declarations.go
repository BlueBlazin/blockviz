// Copyright header: a comment group that is not the package doc.

// Package declarations covers top-level declarations and the comments
// go/parser attaches to them.
package declarations

import "fmt"

import (
	"errors" // line comment on an import

	// Doc comment on an import.
	str "strings"
	. "math"
	_ "embed"
	raw `unicode/utf8`
)

// Doc for a single constant.
const Pi = 3.14159 // line comment on a const

const (
	// Doc on the first spec of a group.
	A = iota // a
	B        // b
	C
	_
	D, E = iota * 2, iota * 3
	F int = 7
)

const typed, untyped float64 = 1, 2

var x int

var (
	y, z    = 1, "two"
	w       map[string][]int
	counter = func() int { return 0 }()
)

var v1, v2 struct{ a, b int } = struct{ a, b int }{1, 2}, struct{ a, b int }{}

// T is a defined type.
type T int

type (
	// Alias is an alias.
	Alias = T
	Point struct {
		X, Y float64 // coordinates
	}
	Fn       func(int) error
	Embedded struct{ Point }
)

// Method with a pointer receiver.
func (p *Point) Move(dx, dy float64) {
	p.X += dx
	p.Y += dy
}

// Value receiver, unnamed.
func (Point) Zero() Point { return Point{} }

func (_ T) Blank() {}

func noBody(x int) int

func variadic(format string, args ...interface{}) (n int, err error) {
	return fmt.Println(format, args)
}

func results() (int, string, error) { return 0, "", errors.New("e") }

func named() (a, b int, c string) { return }

func unnamedParams(int, string, ...float64) {}

func blankParams(_ int, _ string) {}

func paramGroups(a, b int, c string, d, e, f []byte) {}

func funcParams(f func(int) int, g func() (int, error), h func(...string)) func() {
	return nil
}

func init() {}

func init() {
	_ = str.ToUpper
	_ = Sqrt
}

func main() {}
