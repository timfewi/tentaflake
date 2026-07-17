package web

import (
	"errors"
	"os"
	"path/filepath"
	"runtime"
	"testing"

	"tentaflake/tentaflake-auditd/internal/config"
)

// newTestExplorer builds a root tree with a mix of content and secrets.
func newTestExplorer(t *testing.T) (*Explorer, string) {
	t.Helper()
	dir := t.TempDir()
	mk := func(rel, body string) {
		full := filepath.Join(dir, rel)
		if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(full, []byte(body), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	mk("workspace/notes.md", "hello")
	mk("workspace/sub/deep.txt", "deep")
	mk(".env", "SECRET=1")
	mk("workspace/auth.json", "{}")
	mk("workspace/private.key", "k")
	mk("kanban/board.json", "[]")

	exp, err := NewExplorer([]config.Root{{Name: "x", Path: dir}}, nil)
	if err != nil {
		t.Fatal(err)
	}
	return exp, dir
}

func names(es []Entry) map[string]bool {
	m := map[string]bool{}
	for _, e := range es {
		m[e.Name] = true
	}
	return m
}

func TestListExcludesSecretsAtRoot(t *testing.T) {
	exp, _ := newTestExplorer(t)
	entries, err := exp.List("x", "")
	if err != nil {
		t.Fatal(err)
	}
	n := names(entries)
	if !n["workspace"] || !n["kanban"] {
		t.Errorf("expected workspace+kanban, got %v", n)
	}
	if n[".env"] {
		t.Error(".env must be hidden at root")
	}
}

func TestListExcludesSecretsNested(t *testing.T) {
	exp, _ := newTestExplorer(t)
	entries, err := exp.List("x", "workspace")
	if err != nil {
		t.Fatal(err)
	}
	n := names(entries)
	if !n["notes.md"] || !n["sub"] {
		t.Errorf("expected notes.md+sub, got %v", n)
	}
	if n["auth.json"] {
		t.Error("auth.json must be hidden even when nested")
	}
	if n["private.key"] {
		t.Error("*.key must be hidden")
	}
}

func TestReadDeniedComponent(t *testing.T) {
	exp, _ := newTestExplorer(t)
	if _, _, err := exp.Stat("x", ".env"); !errors.Is(err, ErrDenied) {
		t.Errorf("reading .env: want ErrDenied, got %v", err)
	}
	if _, _, err := exp.Stat("x", "workspace/auth.json"); !errors.Is(err, ErrDenied) {
		t.Errorf("reading nested secret: want ErrDenied, got %v", err)
	}
}

func TestTraversalContained(t *testing.T) {
	exp, _ := newTestExplorer(t)
	// "../" style paths must collapse inside the root, never escape it.
	if _, _, err := exp.Stat("x", "../../../../etc/passwd"); err == nil {
		t.Error("traversal to /etc/passwd should not succeed")
	}
	// A clean ".." just resolves back to the root dir (a directory, not a file).
	if _, _, err := exp.Stat("x", ".."); !errors.Is(err, ErrDenied) && !errors.Is(err, ErrNotFound) {
		t.Errorf("'..' should not yield a file, got %v", err)
	}
}

func TestSymlinkEscapeBlocked(t *testing.T) {
	if runtime.GOOS == "windows" {
		t.Skip("symlinks")
	}
	exp, dir := newTestExplorer(t)
	link := filepath.Join(dir, "workspace", "escape")
	if err := os.Symlink("/etc", link); err != nil {
		t.Skipf("symlink unsupported: %v", err)
	}
	if _, _, err := exp.Stat("x", "workspace/escape/hosts"); err == nil {
		t.Error("symlink escaping the root must be blocked")
	}
}

func TestUnknownAgent(t *testing.T) {
	exp, _ := newTestExplorer(t)
	if _, err := exp.List("nope", ""); !errors.Is(err, ErrNotFound) {
		t.Errorf("unknown agent: want ErrNotFound, got %v", err)
	}
}

func TestExtraDeny(t *testing.T) {
	dir := t.TempDir()
	if err := os.WriteFile(filepath.Join(dir, "keepout.bin"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "ok.txt"), []byte("x"), 0o644); err != nil {
		t.Fatal(err)
	}
	exp, err := NewExplorer([]config.Root{{Name: "x", Path: dir}}, []string{"*.bin"})
	if err != nil {
		t.Fatal(err)
	}
	entries, err := exp.List("x", "")
	if err != nil {
		t.Fatal(err)
	}
	if names(entries)["keepout.bin"] {
		t.Error("extra deny *.bin not applied")
	}
}
