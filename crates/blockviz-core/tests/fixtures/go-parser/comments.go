/* A block comment first,
   spanning lines. */
// And a line comment right after: one group.

// Package comments is the package doc: the group right above the clause.
package comments // line comment on the package clause

//go:generate stringer -type=Kind

/*
Doc in a block comment.
*/
type Kind int

// Doc separated by a blank line is not a doc.

func detached() {}

// Doc
//
// with a blank comment line inside.
func withBlankDocLine() {}

/* one */ /* two */
func twoBlocksOnALine() {}

func lineComments() {
	x := 1 // after a statement
	x++    /* block after */
	x = x + /* inside */ 1
	return // after return
}

func blockSpanningLines() int {
	return 1 /* this comment
	spans lines, so a semicolon comes before it */
}

func commentBeforeBrace() {
	_ = 1
	// trailing comment in a block
}

type S struct {
	// Doc on a field.
	A int // line on a field
	B int /* block line on a field */

	// Doc on an embedded field.
	Embedded // line on an embedded field
	C, D     string `tag:"x"` // after a tag
	// Lone comment before the closing brace.
}

type I interface {
	// Doc on a method.
	M() // line on a method
	// Doc on an embedded interface.
	Embedded
	~int | ~string // line on a type set
}

type Embedded interface{}

var (
	// Doc on a var spec.
	a = 1 // line on a var spec
	b = 2 // b's line

	// A group closed by a blank line.

	d = 3
)

const ( // line after the paren
	c = iota /* block */ // and line
)

func params(
	// comments in parameter lists are not docs
	a int, // nor line comments
	b string,
) {
}

//line other.go:100
func renumbered() {}

// After a //line directive the next line is 100: this comment ends on
// line 101 in go/token's numbering and is still a doc for the func below.
func afterDirective() {}

/*line other.go:10:5*/ func blockDirective() {}

//go:noinline
//go:nosplit
func directives() {}

func semicolonAtComment() {
	x := 1 /* not the end */ + 2 // the end
	_ = x
	y := []int{
		1, // first
		2, /* second */
	}
	_ = y
}

// The last comment, with no newline after it.
// It is a line on its own.