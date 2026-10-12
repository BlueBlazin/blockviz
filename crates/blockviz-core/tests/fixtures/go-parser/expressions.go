package expressions

import (
	"fmt"
	"strings"
)

type T struct{ a, b int }

func (T) M(int) int    { return 0 }
func (*T) PM(int) int  { return 0 }
func f(args ...int) int { return len(args) }

func operators(a, b int, x, y bool, c chan int, p *int) {
	_ = a + b*2 - a/b%3
	_ = a<<2 | b>>1 ^ a&b &^ 7
	_ = -a + +b - ^a
	_ = !x && y || !y
	_ = a == b || a != b && a < b || a <= b || a > b || a >= b
	_ = (a + b) * (a - b)
	_ = ((a))
	_ = *p + *&a
	_ = <-c
	_ = <-c + <-c
	v, ok := <-c
	_, _ = v, ok
	_ = &T{1, 2}
	_ = &[]int{1}
	_ = -(-a)
	_ = a + b + a + b + a
	_ = a*b + a*b
	_ = a || b && x
}

func primaries(s []int, m map[string]int, t T, i interface{}, ps *[]int, str string) {
	_ = s[0]
	_ = s[1:]
	_ = s[:2]
	_ = s[1:2]
	_ = s[:]
	_ = s[1:2:3]
	_ = s[:2:3]
	_ = (*ps)[0]
	_ = str[len(str)-1]
	_ = m["k"]
	_ = t.a
	_ = T.M
	_ = (*T).PM
	_ = (&t).M(1)
	_ = i.(T)
	_ = i.(*T)
	_ = i.(interface{ M(int) int })
	_ = i.(fmt.Stringer).String()
	_ = f()
	_ = f(1, 2, 3)
	_ = f(s...)
	_ = f(s[0], s[1:]...)
	_ = fmt.Sprintf("%d %s", 1, "x")
	_ = strings.NewReplacer("a", "b").Replace("abc")
	_ = []byte("bytes")
	_ = string(rune(65))
	_ = (*int)(nil)
	_ = (func())(nil)
	_ = (<-chan int)(nil)
	_ = map[string]int(nil)
	_ = interface{}(nil)
	_ = struct{}{}
	_ = func(x int) int { return x * 2 }(3)
	_ = make([]int, 0, 10)
	_ = new(T)
	_ = make(map[string][]chan<- int)
	_ = len("x")
	_ = s[f(1):f(2)]
}

func composites() {
	_ = T{}
	_ = T{1, 2}
	_ = T{a: 1, b: 2}
	_ = T{
		a: 1,
		b: 2,
	}
	_ = []T{{1, 2}, {a: 3}, {}}
	_ = []*T{{1, 2}, &T{3, 4}}
	_ = [...]int{1, 2, 3}
	_ = [2][2]int{{1, 2}, {3, 4}}
	_ = map[string]T{"x": {1, 2}}
	_ = map[T]string{{1, 2}: "x"}
	_ = map[[2]int][]string{{1, 2}: {"a", "b"}}
	_ = []int{0: 1, 5: 2}
	_ = struct {
		x int
		y string
	}{1, "a"}
	_ = []func() int{func() int { return 1 }}
	_ = []interface{}{1, "a", nil, T{}}
	_ = [][]int{
		{1, 2},
		{3},
	}
}

func literals() {
	_ = 0
	_ = 42
	_ = 4_2
	_ = 0600
	_ = 0_600
	_ = 0o600
	_ = 0O600
	_ = 0x_67_7a_2f_cc_40_c6
	_ = 0XBadFace
	_ = 0b1011
	_ = 0B_1_0
	_ = 170141183460469231731687303715884105727
	_ = 0.
	_ = 72.40
	_ = 072.40
	_ = 2.71828
	_ = 1.e+0
	_ = 6.67428e-11
	_ = 1E6
	_ = .25
	_ = .12345E+5
	_ = 1_5.
	_ = 0.15e+0_2
	_ = 0x1p-2
	_ = 0x2.p10
	_ = 0x1.Fp+0
	_ = 0X.8p-0
	_ = 0X_1FFFP-16
	_ = 0i
	_ = 0123i
	_ = 0o123i
	_ = 0xabci
	_ = 0.i
	_ = 2.71828i
	_ = 1.e+0i
	_ = 6.67428e-11i
	_ = 1E6i
	_ = .25i
	_ = .12345E+5i
	_ = 0x1p-2i
	_ = 'a'
	_ = 'ä'
	_ = '本'
	_ = '\t'
	_ = '\000'
	_ = '\007'
	_ = '\377'
	_ = '\x07'
	_ = '\xff'
	_ = 'ዤ'
	_ = '\U00101234'
	_ = '\''
	_ = '"'
	_ = '\\'
	_ = "abc"
	_ = "\\"
	_ = "\n"
	_ = "\""
	_ = "Hello, world!\n"
	_ = "日本語"
	_ = "日本\U00008a9e"
	_ = "\xffÿ"
	_ = "\a\b\f\n\r\t\v\\\""
	_ = ""
	_ = `abc`
	_ = `\n
\n`
	_ = ``
	_ = `"quoted" and 'single'`
}

func funcLits() {
	add := func(a, b int) int { return a + b }
	_ = add
	var handler func(string) (int, error) = func(s string) (int, error) {
		return len(s), nil
	}
	_ = handler
	defer func() {
		recover()
	}()
	go func(n int) {}(1)
	func() {}()
}
