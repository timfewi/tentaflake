package web

import (
	"context"
	"embed"
	"encoding/json"
	"errors"
	"io"
	"io/fs"
	"log/slog"
	"net/http"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"tentaflake/tentaflake-auditd/internal/event"
	"tentaflake/tentaflake-auditd/internal/store"
)

//go:embed ui
var uiFS embed.FS

// maxPreviewBytes caps the size of a text preview returned by /api/fs/read.
const maxPreviewBytes = 512 * 1024

// maxEventsLimit caps ?limit= on /api/events so a client cannot request an
// unbounded result set.
const maxEventsLimit = 1000

// querier is the read-only subset of *store.Store the console depends on.
// Defining it as an interface keeps the web package testable with a fake store.
type querier interface {
	Query(ctx context.Context, agent, since, until string, limit int) ([]event.Event, error)
	Since(ctx context.Context, afterID int64, limit int) ([]event.Event, error)
	AgentRows(ctx context.Context, window string) ([]store.AgentRow, error)
	Stats(ctx context.Context, window string) (map[string]int, error)
}

// Server wires the audit store and the file Explorer behind one HTTP mux.
type Server struct {
	q   querier
	exp *Explorer
}

// NewServer constructs the console HTTP server.
func NewServer(q querier, exp *Explorer) *Server {
	return &Server{q: q, exp: exp}
}

// Handler returns the fully-routed http.Handler. All routes are GET-only.
func (s *Server) Handler() http.Handler {
	mux := http.NewServeMux()

	mux.HandleFunc("/api/roots", s.get(s.handleRoots))
	mux.HandleFunc("/api/fs/list", s.get(s.handleList))
	mux.HandleFunc("/api/fs/read", s.get(s.handleRead))
	mux.HandleFunc("/api/fs/download", s.get(s.handleDownload))
	mux.HandleFunc("/api/agents", s.get(s.handleAgents))
	mux.HandleFunc("/api/stats", s.get(s.handleStats))
	mux.HandleFunc("/api/events", s.get(s.handleEvents))
	mux.HandleFunc("/api/stream", s.get(s.handleStream))

	// Static UI from the embedded ui/ directory, served at the root.
	sub, _ := fs.Sub(uiFS, "ui")
	mux.Handle("/", http.FileServer(http.FS(sub)))

	return logRequests(mux)
}

// get wraps a handler to enforce the GET method (read-only surface).
func (s *Server) get(h http.HandlerFunc) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet {
			http.Error(w, "method not allowed", http.StatusMethodNotAllowed)
			return
		}
		h(w, r)
	}
}

func (s *Server) handleRoots(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, s.exp.Roots())
}

func (s *Server) handleList(w http.ResponseWriter, r *http.Request) {
	entries, err := s.exp.List(r.URL.Query().Get("agent"), r.URL.Query().Get("path"))
	if err != nil {
		writeFSErr(w, err)
		return
	}
	writeJSON(w, entries)
}

func (s *Server) handleRead(w http.ResponseWriter, r *http.Request) {
	full, info, err := s.exp.Stat(r.URL.Query().Get("agent"), r.URL.Query().Get("path"))
	if err != nil {
		writeFSErr(w, err)
		return
	}
	f, err := openRegular(full)
	if err != nil {
		writeFSErr(w, err)
		return
	}
	defer f.Close()
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	if info.Size() > maxPreviewBytes {
		w.Header().Set("X-Truncated", "true")
	}
	// Copy errors after headers are sent can only mean a dropped client.
	_, _ = io.Copy(w, io.LimitReader(f, maxPreviewBytes))
}

func (s *Server) handleDownload(w http.ResponseWriter, r *http.Request) {
	full, info, err := s.exp.Stat(r.URL.Query().Get("agent"), r.URL.Query().Get("path"))
	if err != nil {
		writeFSErr(w, err)
		return
	}
	f, err := openRegular(full)
	if err != nil {
		writeFSErr(w, err)
		return
	}
	defer f.Close()
	w.Header().Set("Content-Disposition", "attachment; filename=\""+filepath.Base(full)+"\"")
	http.ServeContent(w, r, filepath.Base(full), info.ModTime(), f)
}

func (s *Server) handleAgents(w http.ResponseWriter, r *http.Request) {
	window, err := windowParam(r, "-5 minutes")
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	rows, err := s.q.AgentRows(r.Context(), window)
	if err != nil {
		http.Error(w, "query failed", http.StatusInternalServerError)
		slog.Error("agents query", "error", err)
		return
	}
	writeJSON(w, rows)
}

func (s *Server) handleStats(w http.ResponseWriter, r *http.Request) {
	window, err := windowParam(r, "-24 hours")
	if err != nil {
		http.Error(w, err.Error(), http.StatusBadRequest)
		return
	}
	stats, err := s.q.Stats(r.Context(), window)
	if err != nil {
		http.Error(w, "query failed", http.StatusInternalServerError)
		slog.Error("stats query", "error", err)
		return
	}
	writeJSON(w, stats)
}

// handleEvents returns the most recent events (newest first), optionally
// filtered by ?agent=. ?limit= caps results (default 200, max 1000).
func (s *Server) handleEvents(w http.ResponseWriter, r *http.Request) {
	limit := intParam(r, "limit", 200, maxEventsLimit)
	events, err := s.q.Query(r.Context(), r.URL.Query().Get("agent"), "", "", limit)
	if err != nil {
		http.Error(w, "query failed", http.StatusInternalServerError)
		slog.Error("events query", "error", err)
		return
	}
	writeJSON(w, events)
}

// handleStream is a Server-Sent Events feed of new events. It seeds the cursor
// at the newest existing event, then polls store.Since every second so clients
// receive only events created after they connect.
func (s *Server) handleStream(w http.ResponseWriter, r *http.Request) {
	flusher, ok := w.(http.Flusher)
	if !ok {
		http.Error(w, "streaming unsupported", http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "text/event-stream")
	w.Header().Set("Cache-Control", "no-cache")
	w.Header().Set("Connection", "keep-alive")

	ctx := r.Context()
	lastID := int64(0)
	if newest, err := s.q.Query(ctx, "", "", "", 1); err == nil && len(newest) > 0 {
		lastID = newest[0].ID
	}

	ticker := time.NewTicker(time.Second)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			events, err := s.q.Since(ctx, lastID, 500)
			if err != nil {
				slog.Error("stream since", "error", err)
				continue
			}
			for _, ev := range events {
				b, err := json.Marshal(ev)
				if err != nil {
					continue
				}
				if _, err := io.WriteString(w, "data: "+string(b)+"\n\n"); err != nil {
					return
				}
				lastID = ev.ID
			}
			flusher.Flush()
		}
	}
}

// ── helpers ──────────────────────────────────────────────────────────────────

func writeJSON(w http.ResponseWriter, v any) {
	w.Header().Set("Content-Type", "application/json")
	if v == nil {
		_, _ = w.Write([]byte("[]"))
		return
	}
	if err := json.NewEncoder(w).Encode(v); err != nil {
		slog.Error("encode json", "error", err)
	}
}

func writeFSErr(w http.ResponseWriter, err error) {
	switch {
	case errors.Is(err, ErrNotFound):
		http.Error(w, "not found", http.StatusNotFound)
	case errors.Is(err, ErrDenied):
		http.Error(w, "forbidden", http.StatusForbidden)
	default:
		http.Error(w, "error", http.StatusInternalServerError)
		slog.Error("fs error", "error", err)
	}
}

// windowParam reads ?window= and validates it is a SQLite-safe negative modifier
// (must start with '-'), matching store.Stats's contract.
func windowParam(r *http.Request, def string) (string, error) {
	v := strings.TrimSpace(r.URL.Query().Get("window"))
	if v == "" {
		return def, nil
	}
	if !strings.HasPrefix(v, "-") {
		return "", errors.New("invalid window: must start with '-'")
	}
	return v, nil
}

// intParam reads a positive integer query param, clamped to maxN.
func intParam(r *http.Request, key string, def, maxN int) int {
	if v := r.URL.Query().Get(key); v != "" {
		if n, err := strconv.Atoi(v); err == nil && n > 0 {
			return min(n, maxN)
		}
	}
	return def
}

func logRequests(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		slog.Debug("request", "method", r.Method, "path", r.URL.Path)
		next.ServeHTTP(w, r)
	})
}
