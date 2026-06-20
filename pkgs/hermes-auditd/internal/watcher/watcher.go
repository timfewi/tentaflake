// Package watcher monitors filesystem directories for changes using fsnotify.
//
// It recursively watches directories, extracts agent names from paths,
// debounces rapid events, and ignores well-known noise patterns.
// The watcher has no knowledge of SQLite, HTTP, or WebSocket.
package watcher

import (
	"context"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/fsnotify/fsnotify"
	"github.com/timfewi/nixos-agent-orchestration/hermes-auditd/internal/hermes"
)

// Watcher monitors directories for filesystem events and emits
// hermes.Event values on a channel.
type Watcher struct {
	w    *fsnotify.Watcher
	dirs []string
}

// NewWatcher creates a new Watcher and recursively adds all given
// directories to the underlying fsnotify watcher.
func NewWatcher(dirs []string) (*Watcher, error) {
	w, err := fsnotify.NewWatcher()
	if err != nil {
		return nil, fmt.Errorf("fsnotify new: %w", err)
	}

	watcher := &Watcher{w: w, dirs: dirs}

	for _, dir := range dirs {
		if err := watcher.addRecursive(dir); err != nil {
			w.Close()
			return nil, fmt.Errorf("addRecursive %q: %w", dir, err)
		}
	}

	return watcher, nil
}

// addRecursive walks dir and adds every directory to the watcher.
func (watcher *Watcher) addRecursive(dir string) error {
	return filepath.WalkDir(dir, func(path string, d os.DirEntry, err error) error {
		if err != nil {
			slog.Warn("walk error, skipping", "path", path, "error", err)
			return nil
		}
		if d.IsDir() {
			if isIgnored(path) {
				return filepath.SkipDir
			}
			if err := watcher.w.Add(path); err != nil {
				return fmt.Errorf("watch add %q: %w", path, err)
			}
			slog.Debug("watching directory", "path", path)
		}
		return nil
	})
}

// Start begins watching directories and returns a receive-only channel
// of hermes.Event values. Events are debounced per file within a 100ms
// window. The goroutine exits when ctx is cancelled.
func (watcher *Watcher) Start(ctx context.Context) <-chan hermes.Event {
	out := make(chan hermes.Event, 100)

	go watcher.loop(ctx, out)

	return out
}

// debounceEntry tracks the latest event for a file and its coalescing timer.
type debounceEntry struct {
	event hermes.Event
	timer *time.Timer
}

const debounceWindow = 100 * time.Millisecond

// debounceMap manages pending debounced events with a mutex.
type debounceMap struct {
	mu      sync.Mutex
	pending map[string]*debounceEntry
}

func newDebounceMap() *debounceMap {
	return &debounceMap{
		pending: make(map[string]*debounceEntry),
	}
}

// Add stores or updates a debounce entry for path.
// When the timer fires, after is called.
func (dm *debounceMap) Add(path string, evt hermes.Event, after func()) {
	dm.mu.Lock()
	defer dm.mu.Unlock()

	entry, exists := dm.pending[path]
	if exists {
		entry.timer.Stop()
		entry.event = evt
		entry.timer.Reset(debounceWindow)
	} else {
		timer := time.AfterFunc(debounceWindow, after)
		dm.pending[path] = &debounceEntry{
			event: evt,
			timer: timer,
		}
	}
}

// Flush sends the pending event for path (if any) and removes it.
func (dm *debounceMap) Flush(path string, out chan<- hermes.Event, ctx context.Context) bool {
	dm.mu.Lock()
	entry, ok := dm.pending[path]
	if ok {
		entry.timer.Stop()
		delete(dm.pending, path)
	}
	dm.mu.Unlock()
	if ok {
		select {
		case out <- entry.event:
		case <-ctx.Done():
		}
	}
	return ok
}

// FlushAll sends all pending events (non-blocking) and clears the map.
func (dm *debounceMap) FlushAll(out chan<- hermes.Event) {
	dm.mu.Lock()
	for path, entry := range dm.pending {
		entry.timer.Stop()
		delete(dm.pending, path)
		select {
		case out <- entry.event:
		default:
		}
	}
	dm.mu.Unlock()
}

func (watcher *Watcher) loop(ctx context.Context, out chan<- hermes.Event) {
	defer close(out)

	dm := newDebounceMap()

	for {
		select {
		case <-ctx.Done():
			dm.FlushAll(out)
			return

		case fsEvent, ok := <-watcher.w.Events:
			if !ok {
				return
			}

			if isIgnored(fsEvent.Name) {
				continue
			}

			evt := watcher.toEvent(fsEvent)
			if evt == nil {
				continue
			}

			path := fsEvent.Name
			dm.Add(path, *evt, func() { dm.Flush(path, out, ctx) })

			if fsEvent.Has(fsnotify.Create) {
				watcher.watchNewDirectory(path)
			}

		case err, ok := <-watcher.w.Errors:
			if !ok {
				return
			}
			slog.Error("fsnotify error", "error", err)
		}
	}
}

// watchNewDirectory starts watching a newly created directory recursively.
func (watcher *Watcher) watchNewDirectory(path string) {
	info, err := os.Stat(path)
	if err != nil || !info.IsDir() {
		return
	}
	if err := watcher.addRecursive(path); err != nil {
		slog.Error("add new directory", "path", path, "error", err)
	}
}

// toEvent converts an fsnotify event to a hermes.Event.
// Returns nil if the event type is not interesting.
func (watcher *Watcher) toEvent(fsEvent fsnotify.Event) *hermes.Event {
	var op string
	switch {
	case fsEvent.Has(fsnotify.Create):
		op = "create"
	case fsEvent.Has(fsnotify.Write):
		op = "write"
	case fsEvent.Has(fsnotify.Remove):
		op = "remove"
	case fsEvent.Has(fsnotify.Rename):
		op = "rename"
	case fsEvent.Has(fsnotify.Chmod):
		op = "chmod"
	default:
		return nil
	}

	// Stat the file for size; ignore errors (file may be gone).
	var size int64
	if info, err := os.Stat(fsEvent.Name); err == nil && !info.IsDir() {
		size = info.Size()
	}

	return &hermes.Event{
		Agent:     agentNameFromPath(fsEvent.Name),
		File:      fsEvent.Name,
		Op:        op,
		Timestamp: time.Now().UTC(),
		Size:      size,
	}
}

// agentNameFromPath extracts the agent name from a file path.
// Convention: /var/lib/hermes-<name>/... → "name"
// Unknown paths return "unknown".
func agentNameFromPath(path string) string {
	const prefix = "/var/lib/hermes-"
	if !strings.HasPrefix(path, prefix) {
		return "unknown"
	}
	rest := strings.TrimPrefix(path, prefix)
	parts := strings.SplitN(rest, "/", 2)
	if len(parts) == 0 || parts[0] == "" {
		return "unknown"
	}
	return parts[0]
}

// isIgnored returns true if the path should not produce events.
func isIgnored(path string) bool {
	// Ignore SQLite auxiliary files
	if strings.HasSuffix(path, ".db") ||
		strings.HasSuffix(path, ".db-wal") ||
		strings.HasSuffix(path, ".db-shm") {
		return true
	}

	// Check each path component for ignored directories
	parts := strings.Split(path, string(filepath.Separator))
	for _, part := range parts {
		if part == ".git" || part == "node_modules" {
			return true
		}
	}

	return false
}
