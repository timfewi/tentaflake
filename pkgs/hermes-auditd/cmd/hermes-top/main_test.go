package main

import (
	"context"
	"strings"
	"testing"
	"time"

	tea "github.com/charmbracelet/bubbletea"

	"tentaflake/hermes-auditd/internal/hermes"
	"tentaflake/hermes-auditd/internal/store"
)

// seededModel returns a model backed by an in-memory store with a few events,
// already refreshed and sized, ready for View() assertions.
func seededModel(t *testing.T) model {
	t.Helper()
	st, err := store.New("file::memory:?cache=shared", 1)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { st.Close() })

	ctx := context.Background()
	now := time.Now().UTC()
	events := []hermes.Event{
		{Agent: "coding", File: "/var/lib/hermes-coding/SOUL.md", Op: "write", Timestamp: now.Add(-3 * time.Second)},
		{Agent: "coding", File: "/var/lib/hermes-coding/skills/web.md", Op: "create", Timestamp: now.Add(-1 * time.Second)},
		{Agent: "research", File: "/var/lib/hermes-research/notes.md", Op: "remove", Timestamp: now},
	}
	for _, e := range events {
		if err := st.Insert(ctx, e); err != nil {
			t.Fatal(err)
		}
	}

	m := model{
		st:        st,
		window:    "-300 seconds",
		windowLbl: "5m",
		interval:  time.Second,
		hostname:  "test-host",
	}
	mi, _ := m.Update(tea.WindowSizeMsg{Width: 100, Height: 30})
	m = mi.(model)
	// Drive one refresh synchronously.
	msg := m.refresh()()
	mi, _ = m.Update(msg)
	return mi.(model)
}

func TestViewRendersAgentsAndEvents(t *testing.T) {
	m := seededModel(t)
	if m.err != nil {
		t.Fatalf("unexpected refresh error: %v", m.err)
	}
	out := m.View()

	for _, want := range []string{"hermes-top", "test-host", "coding", "research", "SOUL.md", "skills/web.md", "notes.md", "EVENTS", "create", "remove", "write"} {
		if !strings.Contains(out, want) {
			t.Errorf("View() missing %q\n---\n%s", want, out)
		}
	}
	if m.total != 3 {
		t.Errorf("total = %d, want 3", m.total)
	}
	if got := len(m.logbuf); got != 3 {
		t.Errorf("logbuf = %d events, want 3", got)
	}
	if m.lastID == 0 {
		t.Error("lastID not advanced after refresh")
	}
}

func TestFilterCyclesThroughAgents(t *testing.T) {
	m := seededModel(t)

	// View shows both agents' events before filtering.
	if n := len(m.filtered()); n != 3 {
		t.Fatalf("unfiltered = %d events, want 3", n)
	}

	// f → first agent alphabetically ("coding").
	mi, _ := m.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune{'f'}})
	m = mi.(model)
	if m.filter != "coding" {
		t.Fatalf("after one 'f', filter = %q, want coding", m.filter)
	}
	if n := len(m.filtered()); n != 2 {
		t.Errorf("coding filter = %d events, want 2", n)
	}

	// f → next agent ("research").
	mi, _ = m.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune{'f'}})
	m = mi.(model)
	if m.filter != "research" {
		t.Fatalf("after two 'f', filter = %q, want research", m.filter)
	}

	// f → wraps back to all.
	mi, _ = m.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune{'f'}})
	m = mi.(model)
	if m.filter != "" {
		t.Errorf("after three 'f', filter = %q, want all", m.filter)
	}
}

func TestPauseToggles(t *testing.T) {
	m := seededModel(t)
	mi, _ := m.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune{'p'}})
	m = mi.(model)
	if !m.paused {
		t.Error("expected paused after 'p'")
	}
	if !strings.Contains(m.View(), "PAUSED") {
		t.Error("expected PAUSED indicator in view")
	}
}

func TestQuitKeys(t *testing.T) {
	m := seededModel(t)
	for _, key := range []tea.KeyMsg{
		{Type: tea.KeyRunes, Runes: []rune{'q'}},
		{Type: tea.KeyCtrlC},
		{Type: tea.KeyEsc},
	} {
		_, cmd := m.Update(key)
		if cmd == nil {
			t.Fatalf("key %v returned no command", key)
		}
		if _, ok := cmd().(tea.QuitMsg); !ok {
			t.Errorf("key %v did not produce tea.QuitMsg", key)
		}
	}
}

func TestShortPathStripsStateDirPrefix(t *testing.T) {
	got := shortPath("/var/lib/hermes-coding/skills/web.md", "coding")
	if got != "skills/web.md" {
		t.Errorf("shortPath = %q, want skills/web.md", got)
	}
	// Non-matching paths are returned unchanged.
	if got := shortPath("/etc/passwd", "coding"); got != "/etc/passwd" {
		t.Errorf("shortPath unchanged = %q, want /etc/passwd", got)
	}
}
