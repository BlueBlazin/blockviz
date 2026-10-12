package broken

// Fine parses.
func Fine() int {
	return 1
}

// Broken has a syntax error in the middle.
func Broken() int {
	x := 1
	y := := 2
	return x
}

// StillHere comes after the broken function.
type StillHere struct{ A int }

// AlsoFine comes last.
func AlsoFine() string { return "ok" }
