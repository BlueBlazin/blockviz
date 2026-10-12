// +build linux darwin

// Package legacy uses syntax from Go 1.0 through 1.17.
package legacy

import (
	"io/ioutil"
	"os"
	"strings"
)

const perm = 0755

// Counter counts with a method value and a method expression.
type Counter struct{ n int }

func (c *Counter) Inc() { c.n++ }

// Classify reads a byte the old way.
func Classify(b byte) string {
	switch {
	case 'a' <= b && b <= 'z':
		return "lower"
	case 'A' <= b && b <= 'Z':
		return "upper"
	}
	return "other"
}

// ReadAll reads a file with ioutil.
func ReadAll(path string) (string, error) {
	data, err := ioutil.ReadFile(path)
	if err != nil {
		return "", err
	}
	return string(data), nil
}

// Write writes a file with an octal mode.
func Write(path, text string) error {
	return ioutil.WriteFile(path, []byte(text), perm)
}

// Find searches a grid with labelled loops and goto.
func Find(grid [][]int, want int) (row, col int) {
	row, col = -1, -1
outer:
	for i := 0; i < len(grid); i++ {
		for j := range grid[i] {
			if grid[i][j] == want {
				row, col = i, j
				break outer
			}
		}
	}
	if row < 0 {
		goto done
	}
	_ = strings.Repeat("-", row)
done:
	return
}

// Tally uses a method expression and an anonymous struct.
func Tally(n int) int {
	c := &Counter{}
	inc := (*Counter).Inc
	for i := 0; i < n; i++ {
		inc(c)
	}
	point := struct{ X, Y int }{1, 2}
	return c.n + point.X
}

// Env reads an environment variable with a default.
func Env(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}
