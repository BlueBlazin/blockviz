package semicolons; import "fmt"; func f() int { return 1 }; var x = 1; type T int

func lines(a int, s []int, c chan int) (int, bool) {
	a++
	a--
	s[0]++
	_ = s[0]
	_ = (a)
	_ = s[:]
	_ = func() {}
	fmt.Println(a); fmt.Println(a)
	for { break }
	for {
		continue
	}
L:
	for range s {
		break L
	}
	switch a { case 1: fallthrough; default: }
	if a > 0 { return a, true } else { return 0, false }
}

func empty() {
	;;
	{ ; }
	if true {}
	L1: ;
	L2:
}

func labels() {
A:
B:
	goto A
	goto B
}

func breaks() {
	for {
		break
	}
	for {
		switch {
		case true:
			fallthrough
		default:
		}
		break
	}
}

func returns() (a, b int) { return }

func trailing(
	a int,
	b int,
) (
	int,
	error,
) {
	return f(
		), fmt.Errorf(
		"x",
	)
}

var composite = []int{
	1,
	2,
}

var call = fmt.Sprint(
	1,
	2,
)

func eof() { _ = 1 }