package shapes

import (
	"fmt"
	"math"
	"sort"
)

// Shape is a plane figure.
type Shape interface {
	Area() float64
	fmt.Stringer
}

// Kind says what a shape is.
type Kind int

const (
	KindCircle Kind = iota
	KindSquare
	KindTriangle
)

// Circle is a round shape.
type Circle struct {
	Radius float64 `json:"radius"`
}

// Square has four equal sides.
type Square struct {
	Side float64 `json:"side"`
}

// Area of the square.
func (s Square) Area() float64 { return s.Side * s.Side }

func (s Square) String() string { return fmt.Sprintf("square(%g)", s.Side) }

// Named gives a shape a label.
type Named struct {
	Shape
	Label string
}

// Defaults are the shapes the tour starts with.
func Defaults() []Shape {
	return []Shape{Circle{Radius: 1}, Square{Side: 2}, Named{Shape: Square{Side: 3}, Label: "big"}}
}

// Describe names the kind of a shape.
func Describe(s Shape) string {
	switch v := s.(type) {
	case Circle:
		return "circle of radius " + fmt.Sprint(v.Radius)
	case Square, *Square:
		return "square"
	case nil:
		return "nothing"
	default:
		return "shape"
	}
}

// Largest returns the shape with the largest area.
func Largest(all []Shape) (Shape, bool) {
	if len(all) == 0 {
		return nil, false
	}
	sorted := append([]Shape(nil), all...)
	sort.Slice(sorted, func(i, j int) bool { return sorted[i].Area() > sorted[j].Area() })
	return sorted[0], true
}

// Grade buckets an area.
func Grade(area float64) string {
	switch {
	case area > 100:
		return "huge"
	case area > 10:
		return "large"
	case math.IsNaN(area):
		fallthrough
	case area < 0:
		return "invalid"
	}
	return "small"
}

// Areas computes every area concurrently.
func Areas(all []Shape) []float64 {
	out := make([]float64, len(all))
	done := make(chan int)
	for i, s := range all {
		go func() {
			out[i] = s.Area()
			done <- i
		}()
	}
	for range all {
		<-done
	}
	return out
}
