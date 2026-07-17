package web

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
	"time"

	"tentaflake/hermes-auditd/internal/config"
	"tentaflake/hermes-auditd/internal/hermes"
	"tentaflake/hermes-auditd/internal/store"
)

// fakeQuerier implements the querier interface with canned data.
type fakeQuerier struct {
	events    []hermes.Event
	lastLimit int
}

func (f *fakeQuerier) Query(_ context.Context, agent, _, _ string, limit int) ([]hermes.Event, error) {
	f.lastLimit = limit
	var out []hermes.Event
	for i := len(f.events) - 1; i >= 0 && len(out) < limit; i-- { // newest first
		if agent == "" || f.events[i].Agent == agent {
			out = append(out, f.events[i])
		}
	}
	return out, nil
}

func (f *fakeQuerier) Since(_ context.Context, afterID int64, limit int) ([]hermes.Event, error) {
	var out []hermes.Event
	for _, e := range f.events {
		if e.ID > afterID && len(out) < limit {
			out = append(out, e)
		}
	}
	return out, nil
}

func (f *fakeQuerier) AgentRows(_ context.Context, _ string) ([]store.AgentRow, error) {
	return []store.AgentRow{{Agent: "x", Recent: 2, Total: 2}}, nil
}

func (f *fakeQuerier) Stats(_ context.Context, _ string) (map[string]int, error) {
	return map[string]int{"x": 2}, nil
}

func testServer(t *testing.T) *httptest.Server {
	t.Helper()
	q := &fakeQuerier{events: []hermes.Event{
		{ID: 1, Agent: "x", File: "/var/lib/hermes-x/a", Op: "create", Timestamp: time.Unix(1, 0).UTC()},
		{ID: 2, Agent: "x", File: "/var/lib/hermes-x/b", Op: "write", Timestamp: time.Unix(2, 0).UTC()},
	}}
	exp, err := NewExplorer([]config.Root{{Name: "x", Path: t.TempDir()}}, nil)
	if err != nil {
		t.Fatal(err)
	}
	return httptest.NewServer(NewServer(q, exp).Handler())
}

func TestEventsEndpoint(t *testing.T) {
	srv := testServer(t)
	defer srv.Close()
	resp, err := http.Get(srv.URL + "/api/events?limit=10")
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != 200 {
		t.Fatalf("status %d", resp.StatusCode)
	}
	var evs []hermes.Event
	if err := json.NewDecoder(resp.Body).Decode(&evs); err != nil {
		t.Fatal(err)
	}
	if len(evs) != 2 || evs[0].ID != 2 {
		t.Errorf("want 2 events newest-first, got %+v", evs)
	}
}

func TestRootsEndpoint(t *testing.T) {
	srv := testServer(t)
	defer srv.Close()
	resp, err := http.Get(srv.URL + "/api/roots")
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	var roots []RootName
	json.NewDecoder(resp.Body).Decode(&roots)
	if len(roots) != 1 || roots[0].Name != "x" {
		t.Errorf("want one root 'x', got %+v", roots)
	}
}

func TestWriteMethodRejected(t *testing.T) {
	srv := testServer(t)
	defer srv.Close()
	resp, err := http.Post(srv.URL+"/api/events", "text/plain", nil)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusMethodNotAllowed {
		t.Errorf("POST should be 405, got %d", resp.StatusCode)
	}
}

func TestEventsLimitClamped(t *testing.T) {
	q := &fakeQuerier{}
	exp, err := NewExplorer([]config.Root{{Name: "x", Path: t.TempDir()}}, nil)
	if err != nil {
		t.Fatal(err)
	}
	srv := httptest.NewServer(NewServer(q, exp).Handler())
	defer srv.Close()
	resp, err := http.Get(srv.URL + "/api/events?limit=999999")
	if err != nil {
		t.Fatal(err)
	}
	resp.Body.Close()
	if q.lastLimit != maxEventsLimit {
		t.Errorf("limit passed to store = %d, want clamp to %d", q.lastLimit, maxEventsLimit)
	}
}

func TestInvalidWindow(t *testing.T) {
	srv := testServer(t)
	defer srv.Close()
	resp, err := http.Get(srv.URL + "/api/agents?window=5+minutes")
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	if resp.StatusCode != http.StatusBadRequest {
		t.Errorf("bad window should be 400, got %d", resp.StatusCode)
	}
}
