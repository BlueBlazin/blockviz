package shapes

import (
	"fmt"
	"math"
)

// Area of the circle.
func (c Circle) Area() float64 {
	return math.Pi * c.Radius * c.Radius
}

func (c Circle) String() string {
	return fmt.Sprintf("circle(%g)", c.Radius)
}

// Total adds up the area of shapes, skipping invalid ones.
func Total(shapes ...Shape) (sum float64) {
	for _, s := range shapes {
		if Grade(s.Area()) == "invalid" {
			continue
		}
		sum += s.Area()
	}
	return
}
