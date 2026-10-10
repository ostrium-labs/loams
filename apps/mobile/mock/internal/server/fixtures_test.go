package server_test

import (
	"os"
	"path/filepath"
)

// fixtures reads conformance/fixtures/<name> from the repository root.
func fixtures(name string) ([]byte, error) {
	return os.ReadFile(filepath.Join("..", "..", "..", "conformance", "fixtures", name))
}
