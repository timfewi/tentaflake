// Package store provides a SQLite-backed persistence layer for audit events.
//
// It uses modernc.org/sqlite (pure Go, no CGo) with WAL mode and
// SetMaxOpenConns(1) for safe concurrent access. The store auto-migrates
// the schema on startup and periodically prunes old events.
package store

import (
	"context"
	"database/sql"
	"fmt"
	"log/slog"
	"strings"
	"time"

	_ "modernc.org/sqlite"

	"tentaflake/tentaflake-auditd/internal/event"
)

// queryTimeout bounds every read query so a slow or contended SQLite file
// cannot wedge an HTTP handler or TUI refresh indefinitely.
const queryTimeout = 5 * time.Second

// Store persists events to SQLite and provides query methods.
type Store struct {
	db             *sql.DB
	retentionHours int
}

// New opens the SQLite database, sets connection pragmas, and
// auto-migrates the schema. WAL mode is enabled for concurrency.
func New(dbPath string, retentionHours int) (*Store, error) {
	db, err := sql.Open("sqlite", dbPath)
	if err != nil {
		return nil, fmt.Errorf("sql open: %w", err)
	}

	// modernc.org/sqlite requires max open conns of 1 for safety
	db.SetMaxOpenConns(1)
	db.SetMaxIdleConns(1)

	// Apply schema and pragmas
	if _, err := db.Exec(schema); err != nil {
		db.Close()
		return nil, fmt.Errorf("schema migrate: %w", err)
	}

	return &Store{
		db:             db,
		retentionHours: retentionHours,
	}, nil
}

// Start consumes events from the input channel, inserts them into SQLite,
// and forwards them to the returned notify channel for broadcasting.
// The goroutine exits when ctx is cancelled or the input channel is closed.
func (s *Store) Start(ctx context.Context, eventCh <-chan event.Event) (<-chan event.Event, error) {
	notifyCh := make(chan event.Event, 100)

	go func() {
		defer close(notifyCh)
		for {
			select {
			case <-ctx.Done():
				return
			case evt, ok := <-eventCh:
				if !ok {
					return
				}
				if err := s.Insert(ctx, evt); err != nil {
					slog.Error("store insert", "error", err, "file", evt.File)
					continue
				}
				// Non-blocking notify; drop if nobody listening.
				select {
				case notifyCh <- evt:
				default:
				}
			}
		}
	}()

	return notifyCh, nil
}

// Insert stores a single event into the database.
func (s *Store) Insert(ctx context.Context, evt event.Event) error {
	query := `INSERT INTO events (agent, file, op, size, timestamp) VALUES (?, ?, ?, ?, ?)`
	_, err := s.db.ExecContext(ctx, query, evt.Agent, evt.File, evt.Op, evt.Size, evt.Timestamp.UTC().Format(time.RFC3339))
	if err != nil {
		return fmt.Errorf("insert event: %w", err)
	}
	return nil
}

// Query retrieves events matching the given filters. Empty strings mean
// no filter for that field. Limit caps the number of results (default 100).
func (s *Store) Query(ctx context.Context, agent, since, until string, limit int) ([]event.Event, error) {
	if limit <= 0 {
		limit = 100
	}

	ctx, cancel := context.WithTimeout(ctx, queryTimeout)
	defer cancel()

	query, args := buildQuery(agent, since, until, limit)

	rows, err := s.db.QueryContext(ctx, query, args...)
	if err != nil {
		return nil, fmt.Errorf("query events: %w", err)
	}
	defer rows.Close()

	var events []event.Event
	for rows.Next() {
		var evt event.Event
		var ts string
		if err := rows.Scan(&evt.ID, &evt.Agent, &evt.File, &evt.Op, &evt.Size, &ts); err != nil {
			return nil, fmt.Errorf("scan event: %w", err)
		}
		parsed, err := parseTimestamp(ts)
		if err != nil {
			slog.Warn("parse timestamp, skipping event", "raw", ts, "error", err)
			continue
		}
		evt.Timestamp = parsed
		events = append(events, evt)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("rows iteration: %w", err)
	}

	return events, nil
}

// Since returns events with an ID greater than afterID, in ascending ID order.
// It is the incremental tail used by readers (e.g. the tentaflake-top TUI) to fetch
// only events they have not yet seen. Limit caps results (default 1000).
func (s *Store) Since(ctx context.Context, afterID int64, limit int) ([]event.Event, error) {
	if limit <= 0 {
		limit = 1000
	}
	ctx, cancel := context.WithTimeout(ctx, queryTimeout)
	defer cancel()
	query := `SELECT id, agent, file, op, size, timestamp FROM events WHERE id > ? ORDER BY id ASC LIMIT ?`
	rows, err := s.db.QueryContext(ctx, query, afterID, limit)
	if err != nil {
		return nil, fmt.Errorf("since query: %w", err)
	}
	defer rows.Close()

	var events []event.Event
	for rows.Next() {
		var evt event.Event
		var ts string
		if err := rows.Scan(&evt.ID, &evt.Agent, &evt.File, &evt.Op, &evt.Size, &ts); err != nil {
			return nil, fmt.Errorf("scan event: %w", err)
		}
		parsed, err := parseTimestamp(ts)
		if err != nil {
			slog.Warn("parse timestamp, skipping event", "raw", ts, "error", err)
			continue
		}
		evt.Timestamp = parsed
		events = append(events, evt)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("rows iteration: %w", err)
	}
	return events, nil
}

// AgentRow is a per-agent activity summary for dashboards: how many events fell
// inside the recent window, the all-time total retained in the DB, and the most
// recent event's op/file/timestamp.
type AgentRow struct {
	Agent    string    `json:"agent"`
	Recent   int       `json:"recent"`
	Total    int       `json:"total"`
	LastOp   string    `json:"last_op"`
	LastFile string    `json:"last_file"`
	LastTime time.Time `json:"last_time"`
}

// AgentRows returns one AgentRow per agent present in the DB. `window` is a
// SQLite datetime modifier like "-5 minutes" (must start with "-") used for the
// Recent count; Total counts every retained row for the agent.
func (s *Store) AgentRows(ctx context.Context, window string) ([]AgentRow, error) {
	ctx, cancel := context.WithTimeout(ctx, queryTimeout)
	defer cancel()
	recent, err := s.Stats(ctx, window)
	if err != nil {
		return nil, fmt.Errorf("agent rows recent: %w", err)
	}

	// Total count + last event (row with the max id) per agent in one pass.
	const q = `
SELECT e.agent, c.cnt, e.op, e.file, e.timestamp
FROM events e
JOIN (SELECT agent, COUNT(*) AS cnt, MAX(id) AS mid FROM events GROUP BY agent) c
  ON e.id = c.mid`
	rows, err := s.db.QueryContext(ctx, q)
	if err != nil {
		return nil, fmt.Errorf("agent rows query: %w", err)
	}
	defer rows.Close()

	var out []AgentRow
	for rows.Next() {
		var r AgentRow
		var ts string
		if err := rows.Scan(&r.Agent, &r.Total, &r.LastOp, &r.LastFile, &ts); err != nil {
			return nil, fmt.Errorf("scan agent row: %w", err)
		}
		if parsed, perr := parseTimestamp(ts); perr == nil {
			r.LastTime = parsed
		}
		r.Recent = recent[r.Agent]
		out = append(out, r)
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("agent rows iteration: %w", err)
	}
	return out, nil
}

// buildQuery constructs the SQL query and argument list for filtering events.
func buildQuery(agent, since, until string, limit int) (string, []any) {
	query := `SELECT id, agent, file, op, size, timestamp FROM events WHERE 1=1`
	args := make([]any, 0, 4)

	if agent != "" {
		query += ` AND agent = ?`
		args = append(args, agent)
	}
	if since != "" {
		query += ` AND timestamp >= ?`
		args = append(args, since)
	}
	if until != "" {
		query += ` AND timestamp <= ?`
		args = append(args, until)
	}

	query += ` ORDER BY id DESC LIMIT ?`
	args = append(args, limit)
	return query, args
}

// parseTimestamp tries RFC3339 first, then SQLite default format.
func parseTimestamp(ts string) (time.Time, error) {
	t, err := time.Parse(time.RFC3339, ts)
	if err == nil {
		return t, nil
	}
	t, err = time.Parse("2006-01-02 15:04:05", ts)
	if err == nil {
		return t.UTC(), nil
	}
	return time.Time{}, err
}

// Stats returns event counts per agent in the given time window.
// Window is a SQL expression like '-24 hours' or '-7 days'.
func (s *Store) Stats(ctx context.Context, window string) (map[string]int, error) {
	// Validate window parameter to prevent SQL injection
	window = strings.TrimSpace(window)
	if !strings.HasPrefix(window, "-") {
		return nil, fmt.Errorf("invalid window %q: must start with '-'", window)
	}
	ctx, cancel := context.WithTimeout(ctx, queryTimeout)
	defer cancel()
	// Wrap the stored column in datetime() so the comparison is format-agnostic:
	// events are stored as RFC3339 ("…T…Z") but datetime('now', …) yields the
	// space-separated form, and a raw string compare only agrees when the date
	// differs. datetime() normalizes both sides to the same canonical format.
	query := `SELECT agent, COUNT(*) as cnt FROM events WHERE datetime(timestamp) >= datetime('now', ?) GROUP BY agent`
	rows, err := s.db.QueryContext(ctx, query, window)
	if err != nil {
		return nil, fmt.Errorf("stats query: %w", err)
	}
	defer rows.Close()

	stats := make(map[string]int)
	for rows.Next() {
		var agent string
		var count int
		if err := rows.Scan(&agent, &count); err != nil {
			return nil, fmt.Errorf("stats scan: %w", err)
		}
		stats[agent] = count
	}
	if err := rows.Err(); err != nil {
		return nil, fmt.Errorf("stats rows: %w", err)
	}

	return stats, nil
}

// Prune deletes events older than the configured retention period.
func (s *Store) Prune(ctx context.Context) error {
	// datetime(timestamp) normalizes the stored RFC3339 value before comparison —
	// see the note in Stats for why a raw string compare is unreliable here.
	query := `DELETE FROM events WHERE datetime(timestamp) < datetime('now', ?)`
	hours := fmt.Sprintf("-%d hours", s.retentionHours)
	result, err := s.db.ExecContext(ctx, query, hours)
	if err != nil {
		return fmt.Errorf("prune events: %w", err)
	}
	n, err := result.RowsAffected()
	if err != nil {
		slog.Warn("rows affected after prune", "error", err)
	}
	if n > 0 {
		slog.Info("pruned old events", "count", n, "retention_hours", s.retentionHours)
	}
	return nil
}

// PruneLoop runs periodic pruning every 10 minutes until ctx is cancelled.
func (s *Store) PruneLoop(ctx context.Context) {
	ticker := time.NewTicker(10 * time.Minute)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			if err := s.Prune(ctx); err != nil {
				slog.Error("prune error", "error", err)
			}
		}
	}
}

// Close shuts down the store and releases the database file.
func (s *Store) Close() error {
	return s.db.Close()
}
