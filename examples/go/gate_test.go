package gate

import "testing"

func TestZeroStaysShut(t *testing.T) {
	if Gate(0) {
		t.Fatal("open")
	}
}
