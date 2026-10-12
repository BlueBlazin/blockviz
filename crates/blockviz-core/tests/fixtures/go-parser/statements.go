package statements

import "fmt"

type T struct{ x int }

func simple(a, b int, c chan int, m map[string]int, s []int) {
	a = b
	a, b = b, a
	a += 1
	a -= 1
	a *= 2
	a /= 2
	a %= 3
	a &= 1
	a |= 2
	a ^= 3
	a <<= 1
	a >>= 1
	a &^= 4
	a++
	b--
	s[0]++
	m["k"]--
	c <- a
	x := 1
	y, z := 2, "z"
	_, _, _ = x, y, z
	_ = a
	fmt.Println(a)
	<-c
	(fmt.Println)()
	var v int
	const k = 1
	type local struct{ f int }
	var (
		p = 1
		q string
	)
	_, _, _, _ = v, p, q, local{}
	;
	{
		inner := 1
		_ = inner
	}
	{
	}
}

func ifs(x int, err error) int {
	if x > 0 {
		return 1
	}
	if x := x * 2; x > 10 {
		return x
	} else if x > 5 {
		return 5
	} else if y := x + 1; y > 3 {
		return y
	} else {
		return 0
	}
	if err != nil {
	}
	if v, ok := interface{}(x).(int); ok {
		_ = v
	}
	if (T{}) == (T{x: 1}) {
	}
	if x == (T{}).x {
	}
	if func() bool { return true }() {
	}
	return -1
}

func switches(x int, i interface{}) {
	switch {
	}
	switch x {
	case 1:
	case 2, 3:
		fmt.Println("two or three")
		fallthrough
	case 4:
		break
	default:
		return
	}
	switch y := x * 2; y {
	case 0:
	}
	switch x := x; {
	case x > 0:
	}
	switch f := func() int { return x }; f() {
	}
	switch i.(type) {
	case nil:
	case int, int64:
	case []string, map[string]int, func(), chan int, *T, struct{}, interface{ M() }:
	default:
	}
	switch v := i.(type) {
	case fmt.Stringer:
		_ = v.String()
	}
	switch y := 1; v := i.(type) {
	case int:
		_, _ = y, v
	}
	switch (T{}) {
	}
}

func loops(s []int, m map[string]int, c chan int, n int, seq func(func(int) bool)) {
	for {
		break
	}
	for n > 0 {
		n--
	}
	for i := 0; i < 10; i++ {
		continue
	}
	for i := 0; i < 10; {
	}
	for ; n > 0; n-- {
	}
	for ; ; n++ {
	}
	for ;; {
		break
	}
	for i := range s {
		_ = i
	}
	for i, v := range s {
		_, _ = i, v
	}
	for _, v := range []int{1, 2} {
		_ = v
	}
	for k, v := range m {
		_, _ = k, v
	}
	for v := range c {
		_ = v
	}
	var k string
	var v int
	for k, v = range m {
	}
	for k = range m {
	}
	_, _ = k, v
	for range s {
	}
	for range 10 {
	}
	for i := range 10 {
		_ = i
	}
	for x := range seq {
		_ = x
	}
	for range c {
	}
	for i := 0; i < len([]int{1}); i++ {
	}
}

func jumps(n int) {
outer:
	for i := 0; i < n; i++ {
	inner:
		for j := 0; j < n; j++ {
			if j == 1 {
				continue outer
			}
			if j == 2 {
				break inner
			}
			if j == 3 {
				goto done
			}
		}
	}
done:
	return
empty:
}

func selects(c chan int, d chan<- string, quit <-chan struct{}) {
	select {}
	select {
	case v := <-c:
		_ = v
	case v, ok := <-c:
		_, _ = v, ok
	case <-c:
	case d <- "x":
	case <-quit:
		return
	default:
	}
	var x int
	select {
	case x = <-c:
	}
	_ = x
}

func gos(f func(), g func(int) int) {
	go f()
	go g(1)
	go func() {}()
	defer f()
	defer fmt.Println("done")
	defer func(x int) {}(1)
	go (f)()
}

func returns() (int, error) {
	return 0, nil
}

func bareReturn() {
	return
}

func nested() {
	{
		{
			{
				if true {
					for {
						switch {
						default:
							func() {
								select {
								default:
								}
							}()
						}
						break
					}
				}
			}
		}
	}
}
