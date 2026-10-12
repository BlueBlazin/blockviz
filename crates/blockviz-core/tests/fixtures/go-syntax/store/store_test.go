package store

import (
	"testing"

	"example.com/tour/shapes"
)

func TestNotFound(t *testing.T) {
	err := &NotFoundError{Key: "x"}
	if err.Error() != "not found: x" {
		t.Fatalf("got %q", err.Error())
	}
}

func TestAreas(t *testing.T) {
	cases := []struct {
		name string
		s    shapes.Shape
		want float64
	}{
		{"square", shapes.Square{Side: 2}, 4},
		{"unit", shapes.Square{Side: 1}, 1},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			if got := tc.s.Area(); got != tc.want {
				t.Errorf("area = %v, want %v", got, tc.want)
			}
		})
	}
}

func BenchmarkSafely(b *testing.B) {
	for b.Loop() {
		_ = safely(func() {})
	}
}
