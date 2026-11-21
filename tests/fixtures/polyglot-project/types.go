// Package polyglot is the Go side of the polyglot fixture.
package polyglot

import "fmt"

// Widget is the acceptance anchor struct.
type Widget struct {
	Name  string
	Depth int
}

// NewWidget constructs the anchor used from main.go.
func NewWidget(name string, depth int) *Widget {
	return &Widget{Name: name, Depth: depth}
}

// Describe is referenced from main.go.
func (w *Widget) Describe() string {
	return fmt.Sprintf("%s:%d", w.Name, w.Depth)
}
