package types

import (
	"io"
	"sync"
)

type (
	Array      [4]int
	Array2     [2][3]float64
	ArrayExpr  [N * 2]byte
	ArrayParen [(N)]byte
	Slice      []string
	SliceSlice [][]int
	Map        map[string]int
	MapNested  map[[2]int]map[string][]*int
	Pointer    *int
	PtrPtr     **Pointer
	Qualified  io.Reader
	QualPtr    *sync.Mutex
	Paren      (int)
	ParenPtr   (*int)

	Chan       chan int
	SendOnly   chan<- int
	RecvOnly   <-chan int
	ChanOfRecv chan (<-chan int)
	SendOfRecv chan<- <-chan int
	RecvOfRecv <-chan <-chan int
	ChanChan   chan chan int
	SendChan   chan<- chan int

	Func        func()
	FuncResult  func(int) string
	FuncResults func(a, b int) (c int, err error)
	FuncVar     func(...int)
	FuncFunc    func(func(int) bool) func() int
	FuncChan    func() <-chan int

	Empty  struct{}
	Struct struct {
		a       int
		b, c    string
		D       []byte `json:"d,omitempty"`
		E       map[string]int "plain tag"
		f       func()
		g, h, i struct{ x int }
		_       int
	}
	EmbedStruct struct {
		io.Reader
		*sync.Mutex
		Struct `embedded:"tag"`
		*Empty
		error
	}

	EmptyInterface interface{}
	Interface      interface {
		Read(p []byte) (n int, err error)
		Close() error
		io.Writer
		fmt
		String() string // line comment on a method
		// Doc comment on a method.
		Len() int
	}
	Any = interface{}
)

const N = 4

var anon struct {
	sync.Mutex
	m map[string]int
}

var ch = make(chan<- struct{}, 1)

var conv = (<-chan int)(nil)

var fnType = func(x int) (y int) { return x }

var arrays = [...]string{"a", "b"}

var nested = [][]int{{1}, {2, 3}}

var mapLit = map[string]struct{ x int }{"a": {1}, "b": {x: 2}}
