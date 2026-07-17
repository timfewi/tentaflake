package watcher

import (
	"context"
	"os"
	"path/filepath"

	"testing"
	"time"

	"tentaflake/hermes-auditd/internal/hermes"
)

func newTempDir(t *testing.T) string {
	t.Helper()
	dir, err := os.MkdirTemp("", "hermes-test-*")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.RemoveAll(dir) })
	return dir
}

// writeFile writes data to a new or existing file (triggers inotify events).
func writeFile(t *testing.T, path string) {
	t.Helper()
	if err := os.WriteFile(path, []byte("x"), 0644); err != nil {
		t.Fatal(err)
	}
}

// createEmptyFile creates a file without writing content (triggers create-only).
func createEmptyFile(t *testing.T, path string) {
	t.Helper()
	f, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	f.Close()
}

// assertEventIn checks that an event arrives with any of the given ops.
func assertEventIn(t *testing.T, ch <-chan hermes.Event, ops []string, file string) hermes.Event {
	t.Helper()
	select {
	case evt := <-ch:
		matched := false
		for _, op := range ops {
			if evt.Op == op {
				matched = true
				break
			}
		}
		if !matched {
			t.Errorf("event op %q not in acceptable ops %v for file %q", evt.Op, ops, file)
		}
		if evt.File != file {
			t.Errorf("expected file %q, got %q", file, evt.File)
		}
		return evt
	case <-time.After(2 * time.Second):
		t.Fatalf("timeout waiting for event: ops=%v file=%q", ops, file)
		return hermes.Event{}
	}
}

func assertNoEvent(t *testing.T, ch <-chan hermes.Event, label string) {
	t.Helper()
	select {
	case evt := <-ch:
		t.Errorf("unexpected event for %s: %+v", label, evt)
	case <-time.After(500 * time.Millisecond):
	}
}

func TestWatcherCreateFile(t *testing.T) {
	dir := newTempDir(t)

	w, err := NewWatcher([]string{dir})
	if err != nil {
		t.Fatal(err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	ch := w.Start(ctx)

	time.Sleep(100 * time.Millisecond)

	testFile := filepath.Join(dir, "test.txt")
	writeFile(t, testFile)

	// Accept create or write — inotify may coalesce both within debounce window
	assertEventIn(t, ch, []string{"create", "write"}, testFile)
}

func TestWatcherRemoveFile(t *testing.T) {
	dir := newTempDir(t)

	w, err := NewWatcher([]string{dir})
	if err != nil {
		t.Fatal(err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	ch := w.Start(ctx)

	time.Sleep(100 * time.Millisecond)

	rmFile := filepath.Join(dir, "rmtest.txt")
	writeFile(t, rmFile)
	// consume the create/write event
	assertEventIn(t, ch, []string{"create", "write"}, rmFile)

	if err := os.Remove(rmFile); err != nil {
		t.Fatal(err)
	}

	assertEventIn(t, ch, []string{"remove"}, rmFile)
}

func TestWatcherIgnoreDB(t *testing.T) {
	dir := newTempDir(t)

	w, err := NewWatcher([]string{dir})
	if err != nil {
		t.Fatal(err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	ch := w.Start(ctx)

	time.Sleep(100 * time.Millisecond)

	// Create a .db file — should be ignored
	writeFile(t, filepath.Join(dir, "events.db"))

	assertNoEvent(t, ch, "events.db")
}

func TestWatcherDebounce(t *testing.T) {
	dir := newTempDir(t)

	w, err := NewWatcher([]string{dir})
	if err != nil {
		t.Fatal(err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	ch := w.Start(ctx)

	time.Sleep(100 * time.Millisecond)

	testFile := filepath.Join(dir, "debounce.txt")

	// Rapid writes to same file — should coalesce into 1 event
	writeFile(t, testFile)
	time.Sleep(10 * time.Millisecond)
	writeFile(t, testFile)
	time.Sleep(10 * time.Millisecond)
	writeFile(t, testFile)

	// Should produce exactly 1 event after debounce
	assertEventIn(t, ch, []string{"create", "write"}, testFile)

	// No second event
	assertNoEvent(t, ch, "second debounce event")
}

func TestWatcherMultipleDirs(t *testing.T) {
	dir1 := newTempDir(t)
	dir2 := newTempDir(t)

	w, err := NewWatcher([]string{dir1, dir2})
	if err != nil {
		t.Fatal(err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	ch := w.Start(ctx)

	time.Sleep(100 * time.Millisecond)

	file1 := filepath.Join(dir1, "f1.txt")
	file2 := filepath.Join(dir2, "f2.txt")
	writeFile(t, file1)
	writeFile(t, file2)

	seen := make(map[string]bool)
	for i := 0; i < 2; i++ {
		select {
		case evt := <-ch:
			seen[evt.File] = true
		case <-time.After(2 * time.Second):
			t.Fatalf("timeout waiting for event %d", i)
		}
	}

	if !seen[file1] {
		t.Errorf("missing event for %s", file1)
	}
	if !seen[file2] {
		t.Errorf("missing event for %s", file2)
	}
}

func TestWatcherIgnoreGitDir(t *testing.T) {
	dir := newTempDir(t)

	w, err := NewWatcher([]string{dir})
	if err != nil {
		t.Fatal(err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	ch := w.Start(ctx)

	time.Sleep(100 * time.Millisecond)

	// Create .git directory — should be ignored by addRecursive
	gitDir := filepath.Join(dir, ".git")
	if err := os.Mkdir(gitDir, 0755); err != nil {
		t.Fatal(err)
	}

	// Writing within .git should not produce events
	writeFile(t, filepath.Join(gitDir, "HEAD"))
	assertNoEvent(t, ch, ".git/HEAD")
}

func TestAgentNameFromPath(t *testing.T) {
	tests := []struct {
		path string
		want string
	}{
		{"/var/lib/hermes-coding/test.txt", "coding"},
		{"/var/lib/hermes-coding/skills/foo.lua", "coding"},
		{"/var/lib/hermes-coding", "coding"},
		{"/var/lib/hermes-coding/", "coding"},
		{"/var/lib/hermes-coding/workspace/proj/main.go", "coding"},
		{"/var/lib/zeroclaw-assistant/test.txt", "zeroclaw-assistant"},
		{"/var/lib/zeroclaw-assistant/data/skills/foo.md", "zeroclaw-assistant"},
		{"/var/lib/zeroclaw-assistant", "zeroclaw-assistant"},
		{"/tmp/something.txt", "unknown"},
		{"/var/lib/other/file.txt", "unknown"},
		{"/var/lib/hermes-/file.txt", "unknown"},
		{"/var/lib/zeroclaw-/file.txt", "unknown"},
	}

	for _, tc := range tests {
		t.Run(tc.path, func(t *testing.T) {
			got := agentNameFromPath(tc.path)
			if got != tc.want {
				t.Errorf("agentNameFromPath(%q) = %q, want %q", tc.path, got, tc.want)
			}
		})
	}
}

func TestIsIgnored(t *testing.T) {
	tests := []struct {
		path    string
		ignored bool
	}{
		{"/var/lib/hermes-coding/events.db", true},
		{"/var/lib/hermes-coding/events.db-wal", true},
		{"/var/lib/hermes-coding/events.db-shm", true},
		{"/var/lib/hermes-coding/.git/HEAD", true},
		{"/var/lib/hermes-coding/node_modules/pkg/index.js", true},
		{"/var/lib/hermes-coding/workspace/main.go", false},
		{"/tmp/test.txt", false},
	}

	for _, tc := range tests {
		t.Run(tc.path, func(t *testing.T) {
			got := isIgnored(tc.path)
			if got != tc.ignored {
				t.Errorf("isIgnored(%q) = %v, want %v", tc.path, got, tc.ignored)
			}
		})
	}
}

// TestWatcherDebounceWriteAfterCreate verifies that creating then
// immediately writing a file produces only one debounced event.
func TestWatcherDebounceWriteAfterCreate(t *testing.T) {
	dir := newTempDir(t)

	w, err := NewWatcher([]string{dir})
	if err != nil {
		t.Fatal(err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	ch := w.Start(ctx)

	time.Sleep(100 * time.Millisecond)

	testFile := filepath.Join(dir, "quickwrite.txt")

	// Create file empty then write — rapid succession
	createEmptyFile(t, testFile)
	time.Sleep(10 * time.Millisecond)
	writeFile(t, testFile)

	// Should coalesce into 1 event
	assertEventIn(t, ch, []string{"create", "write"}, testFile)
	assertNoEvent(t, ch, "second event after coalesce")
}

// TestWatcherDirCap verifies that addRecursive stops adding inotify watches
// once the directory cap is reached.
func TestWatcherDirCap(t *testing.T) {
	old := maxWatchedDirs
	maxWatchedDirs = 2
	t.Cleanup(func() { maxWatchedDirs = old })

	dir := newTempDir(t)
	for _, name := range []string{"a", "b", "c", "d"} {
		if err := os.Mkdir(filepath.Join(dir, name), 0755); err != nil {
			t.Fatal(err)
		}
	}

	w, err := NewWatcher([]string{dir})
	if err != nil {
		t.Fatal(err)
	}
	defer w.Close()

	if w.watched != 2 {
		t.Errorf("watched = %d, want cap 2", w.watched)
	}
	if !w.capWarned {
		t.Error("capWarned not set after hitting the cap")
	}
}

// TestWatcherNewSubDirectory verifies that creating a new directory
// within a watched directory is recursively watched.
func TestWatcherNewSubDirectory(t *testing.T) {
	dir := newTempDir(t)

	w, err := NewWatcher([]string{dir})
	if err != nil {
		t.Fatal(err)
	}

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	ch := w.Start(ctx)

	time.Sleep(100 * time.Millisecond)

	subDir := filepath.Join(dir, "subdir")
	if err := os.Mkdir(subDir, 0755); err != nil {
		t.Fatal(err)
	}

	time.Sleep(200 * time.Millisecond)

	// Consume the directory-creation event
	assertEventIn(t, ch, []string{"create"}, subDir)

	// File in the new subdirectory should be detected
	subFile := filepath.Join(subDir, "nested.txt")
	writeFile(t, subFile)

	assertEventIn(t, ch, []string{"create", "write"}, subFile)
}
