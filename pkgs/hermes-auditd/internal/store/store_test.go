package store

import (
	"context"
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"tentaflake/hermes-auditd/internal/hermes"
)

func newTempDB(t *testing.T) string {
	t.Helper()
	dir, err := os.MkdirTemp("", "hermes-store-test-*")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.RemoveAll(dir) })
	return filepath.Join(dir, "events.db")
}

func TestNewCreatesTables(t *testing.T) {
	dbPath := newTempDB(t)
	st, err := New(dbPath, 24)
	if err != nil {
		t.Fatal(err)
	}
	defer st.Close()

	// Verify tables exist by running a query
	ctx := context.Background()
	var count int
	err = st.db.QueryRowContext(ctx, "SELECT COUNT(*) FROM events").Scan(&count)
	if err != nil {
		t.Fatalf("events table not found: %v", err)
	}
}

func TestInsertAndQueryRoundTrip(t *testing.T) {
	dbPath := newTempDB(t)
	st, err := New(dbPath, 24)
	if err != nil {
		t.Fatal(err)
	}
	defer st.Close()

	ctx := context.Background()

	evt := hermes.Event{
		Agent:     "coding",
		File:      "/var/lib/hermes-coding/test.txt",
		Op:        "write",
		Timestamp: time.Now().UTC(),
		Size:      42,
	}

	if err := st.Insert(ctx, evt); err != nil {
		t.Fatal(err)
	}

	events, err := st.Query(ctx, "", "", "", 10)
	if err != nil {
		t.Fatal(err)
	}

	if len(events) != 1 {
		t.Fatalf("expected 1 event, got %d", len(events))
	}

	got := events[0]
	if got.Agent != "coding" {
		t.Errorf("agent = %q, want %q", got.Agent, "coding")
	}
	if got.File != "/var/lib/hermes-coding/test.txt" {
		t.Errorf("file = %q, want %q", got.File, "/var/lib/hermes-coding/test.txt")
	}
	if got.Op != "write" {
		t.Errorf("op = %q, want %q", got.Op, "write")
	}
	if got.Size != 42 {
		t.Errorf("size = %d, want %d", got.Size, 42)
	}
}

func TestQueryFilterByAgent(t *testing.T) {
	dbPath := newTempDB(t)
	st, err := New(dbPath, 24)
	if err != nil {
		t.Fatal(err)
	}
	defer st.Close()

	ctx := context.Background()
	now := time.Now().UTC()

	for idx, agent := range []string{"coding", "writing", "coding"} {
		st.Insert(ctx, hermes.Event{
			Agent:     agent,
			File:      "/tmp/f",
			Op:        "write",
			Timestamp: now.Add(time.Duration(idx) * time.Second),
		})
	}

	events, err := st.Query(ctx, "coding", "", "", 10)
	if err != nil {
		t.Fatal(err)
	}
	if len(events) != 2 {
		t.Errorf("expected 2 coding events, got %d", len(events))
	}
}

func TestPruneRemovesOldEvents(t *testing.T) {
	dbPath := newTempDB(t)
	st, err := New(dbPath, 24)
	if err != nil {
		t.Fatal(err)
	}
	defer st.Close()

	ctx := context.Background()
	now := time.Now().UTC()

	// Insert an old event (by manipulating timestamp directly)
	_, err = st.db.ExecContext(ctx,
		`INSERT INTO events (agent, file, op, timestamp) VALUES (?, ?, ?, ?)`,
		"coding", "/old.txt", "write", now.Add(-48*time.Hour).Format(time.RFC3339))
	if err != nil {
		t.Fatal(err)
	}

	// Insert a recent event
	st.Insert(ctx, hermes.Event{
		Agent:     "coding",
		File:      "/new.txt",
		Op:        "create",
		Timestamp: now,
	})

	// Prune (retention 24h)
	if err := st.Prune(ctx); err != nil {
		t.Fatal(err)
	}

	events, err := st.Query(ctx, "", "", "", 10)
	if err != nil {
		t.Fatal(err)
	}
	if len(events) != 1 {
		t.Errorf("expected 1 event after prune, got %d", len(events))
	}
	if events[0].File != "/new.txt" {
		t.Errorf("expected /new.txt to remain, got %q", events[0].File)
	}
}

func TestConcurrentInserts(t *testing.T) {
	dbPath := newTempDB(t)
	st, err := New(dbPath, 24)
	if err != nil {
		t.Fatal(err)
	}
	defer st.Close()

	ctx := context.Background()

	var wg sync.WaitGroup
	n := 10
	for i := range n {
		wg.Add(1)
		go func(idx int) {
			defer wg.Done()
			evt := hermes.Event{
				Agent:     "coding",
				File:      "/f.txt",
				Op:        "write",
				Timestamp: time.Now().UTC(),
				Size:      int64(idx),
			}
			if err := st.Insert(ctx, evt); err != nil {
				t.Errorf("concurrent insert %d: %v", idx, err)
			}
		}(i)
	}
	wg.Wait()

	events, err := st.Query(ctx, "", "", "", 100)
	if err != nil {
		t.Fatal(err)
	}
	if len(events) != n {
		t.Errorf("expected %d events, got %d", n, len(events))
	}
}

func TestInMemorySQLite(t *testing.T) {
	st, err := New("file::memory:?cache=shared", 24)
	if err != nil {
		t.Fatal(err)
	}
	defer st.Close()

	ctx := context.Background()
	evt := hermes.Event{
		Agent:     "test",
		File:      "/test.txt",
		Op:        "create",
		Timestamp: time.Now().UTC(),
	}
	if err := st.Insert(ctx, evt); err != nil {
		t.Fatal(err)
	}

	events, err := st.Query(ctx, "", "", "", 10)
	if err != nil {
		t.Fatal(err)
	}
	if len(events) != 1 {
		t.Errorf("expected 1 event, got %d", len(events))
	}
}

func TestStats(t *testing.T) {
	dbPath := newTempDB(t)
	st, err := New(dbPath, 24)
	if err != nil {
		t.Fatal(err)
	}
	defer st.Close()

	ctx := context.Background()
	now := time.Now().UTC()

	for _, agent := range []string{"coding", "coding", "writing"} {
		st.Insert(ctx, hermes.Event{
			Agent:     agent,
			File:      "/f.txt",
			Op:        "write",
			Timestamp: now,
		})
	}

	stats, err := st.Stats(ctx, "-24 hours")
	if err != nil {
		t.Fatal(err)
	}

	if stats["coding"] != 2 {
		t.Errorf("expected coding=2, got %d", stats["coding"])
	}
	if stats["writing"] != 1 {
		t.Errorf("expected writing=1, got %d", stats["writing"])
	}
}

func TestStartConsumesEvents(t *testing.T) {
	dbPath := newTempDB(t)
	st, err := New(dbPath, 24)
	if err != nil {
		t.Fatal(err)
	}
	defer st.Close()

	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	eventCh := make(chan hermes.Event, 10)
	notifyCh, err := st.Start(ctx, eventCh)
	if err != nil {
		t.Fatal(err)
	}

	evt := hermes.Event{
		Agent:     "coding",
		File:      "/test.txt",
		Op:        "create",
		Timestamp: time.Now().UTC(),
	}

	eventCh <- evt

	// Verify it appears on notify channel
	select {
	case got := <-notifyCh:
		if got.File != "/test.txt" {
			t.Errorf("notify file = %q, want %q", got.File, "/test.txt")
		}
	case <-time.After(2 * time.Second):
		t.Fatal("timeout waiting for notify")
	}

	// Verify it was persisted
	events, err := st.Query(ctx, "", "", "", 10)
	if err != nil {
		t.Fatal(err)
	}
	if len(events) != 1 {
		t.Errorf("expected 1 persisted event, got %d", len(events))
	}
}
