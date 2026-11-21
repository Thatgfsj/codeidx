package polyglot

import "fmt"

// BuildWidget is a cross-file call site: its body calls NewWidget.
func BuildWidget() *Widget {
	return NewWidget("demo", 2)
}

func main() {
	w := BuildWidget()
	fmt.Println(w.Describe())
}
