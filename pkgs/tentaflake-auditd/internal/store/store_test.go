package store

import (
	"context"
	"os"
	"path/filepath"
	"sync"
	"testing"
	"time"

	"tentaflake/tentaflake-auditd/internal/event"
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

	evt := event.Event{
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
		st.Insert(ctx, event.Event{
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
	st.Insert(ctx, event.Event{
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

// TestPruneSameDayBoundary guards against the format-mismatch bug where stored
// RFC3339 timestamps ("…T…Z") were string-compared against datetime('now', …)
// (space-separated): the compare only agreed when the date differed, so an event
// older than retention but on the same calendar day was never pruned.
func TestPruneSameDayBoundary(t *testing.T) {
	dbPath := newTempDB(t)
	st, err := New(dbPath, 1) // 1-hour retention
	if err != nil {
		t.Fatal(err)
	}
	defer st.Close()

	ctx := context.Background()
	now := time.Now().UTC()

	// 90 minutes old — same day, but outside the 1-hour window.
	st.Insert(ctx, event.Event{Agent: "coding", File: "/old.txt", Op: "write", Timestamp: now.Add(-90 * time.Minute)})
	st.Insert(ctx, event.Event{Agent: "coding", File: "/new.txt", Op: "write", Timestamp: now})

	if err := st.Prune(ctx); err != nil {
		t.Fatal(err)
	}
	events, err := st.Query(ctx, "", "", "", 10)
	if err != nil {
		t.Fatal(err)
	}
	if len(events) != 1 {
		t.Fatalf("expected 1 event after same-day prune, got %d", len(events))
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
			evt := event.Event{
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
	evt := event.Event{
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
		st.Insert(ctx, event.Event{
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

func TestSinceReturnsOnlyNewerEvents(t *testing.T) {
	dbPath := newTempDB(t)
	st, err := New(dbPath, 24)
	if err != nil {
		t.Fatal(err)
	}
	defer st.Close()

	ctx := context.Background()
	now := time.Now().UTC()
	for i := range 5 {
		st.Insert(ctx, event.Event{
			Agent:     "coding",
			File:      "/f.txt",
			Op:        "write",
			Timestamp: now.Add(time.Duration(i) * time.Second),
		})
	}

	// All events, ascending.
	all, err := st.Since(ctx, 0, 100)
	if err != nil {
		t.Fatal(err)
	}
	if len(all) != 5 {
		t.Fatalf("expected 5 events, got %d", len(all))
	}
	for i := 1; i < len(all); i++ {
		if all[i].ID <= all[i-1].ID {
			t.Fatalf("expected ascending IDs, got %d after %d", all[i].ID, all[i-1].ID)
		}
	}

	// Only events after the 3rd row.
	rest, err := st.Since(ctx, all[2].ID, 100)
	if err != nil {
		t.Fatal(err)
	}
	if len(rest) != 2 {
		t.Errorf("expected 2 newer events, got %d", len(rest))
	}

	// Limit is respected.
	limited, err := st.Since(ctx, 0, 2)
	if err != nil {
		t.Fatal(err)
	}
	if len(limited) != 2 {
		t.Errorf("expected limit of 2, got %d", len(limited))
	}
}

func TestAgentRows(t *testing.T) {
	dbPath := newTempDB(t)
	st, err := New(dbPath, 24)
	if err != nil {
		t.Fatal(err)
	}
	defer st.Close()

	ctx := context.Background()
	now := time.Now().UTC()

	// coding: one old (outside 5m window) + two recent; last op is "remove".
	if _, err := st.db.ExecContext(ctx,
		`INSERT INTO events (agent, file, op, timestamp) VALUES (?, ?, ?, ?)`,
		"coding", "/old.txt", "write", now.Add(-1*time.Hour).Format(time.RFC3339)); err != nil {
		t.Fatal(err)
	}
	st.Insert(ctx, event.Event{Agent: "coding", File: "/a.txt", Op: "create", Timestamp: now.Add(-2 * time.Second)})
	st.Insert(ctx, event.Event{Agent: "coding", File: "/b.txt", Op: "remove", Timestamp: now})
	// research: one recent event.
	st.Insert(ctx, event.Event{Agent: "research", File: "/r.txt", Op: "write", Timestamp: now})

	rows, err := st.AgentRows(ctx, "-5 minutes")
	if err != nil {
		t.Fatal(err)
	}

	byAgent := make(map[string]AgentRow, len(rows))
	for _, r := range rows {
		byAgent[r.Agent] = r
	}

	coding, ok := byAgent["coding"]
	if !ok {
		t.Fatal("missing coding row")
	}
	if coding.Total != 3 {
		t.Errorf("coding total = %d, want 3", coding.Total)
	}
	if coding.Recent != 2 {
		t.Errorf("coding recent (5m) = %d, want 2", coding.Recent)
	}
	if coding.LastOp != "remove" || coding.LastFile != "/b.txt" {
		t.Errorf("coding last = %s %s, want remove /b.txt", coding.LastOp, coding.LastFile)
	}

	research, ok := byAgent["research"]
	if !ok {
		t.Fatal("missing research row")
	}
	if research.Total != 1 || research.Recent != 1 {
		t.Errorf("research total/recent = %d/%d, want 1/1", research.Total, research.Recent)
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

	eventCh := make(chan event.Event, 10)
	notifyCh, err := st.Start(ctx, eventCh)
	if err != nil {
		t.Fatal(err)
	}

	evt := event.Event{
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
