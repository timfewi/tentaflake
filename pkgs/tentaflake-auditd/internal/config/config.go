// Package config loads the tentaflake-auditd configuration from environment variables.
//
// Every shared value (port, paths, user names) is defined here in ONE place.
// The Go binary reads from env vars; the NixOS module sets them.
// No hardcoded magic strings exist elsewhere in the codebase.
package config

import (
	"fmt"
	"os"
	"strconv"
	"strings"
)

// Config holds all configuration for tentaflake-auditd.
// All values are populated from environment variables with sensible defaults.
type Config struct {
	// Port is the HTTP/WebSocket listen port (AUDIT_PORT, default 9090).
	Port int

	// DBPath is the path to the SQLite database file (AUDIT_DB_PATH).
	DBPath string

	// WatchDirs is the list of directories to monitor (AUDIT_WATCH_DIRS, comma-separated).
	WatchDirs []string

	// RetentionHours is the number of hours to retain events (AUDIT_RETENTION_HOURS, default 24).
	RetentionHours int

	// ConsoleAddr is the listen address for the Agent Console web server
	// (AUDIT_CONSOLE_ADDR, default 127.0.0.1:9090). Used by cmd/tentaflake-console.
	ConsoleAddr string

	// ConsoleRoots are the agent file roots exposed (read-only) by the console
	// file explorer (AUDIT_CONSOLE_ROOTS, comma-separated "name:path" pairs).
	ConsoleRoots []Root

	// ConsoleDeny are extra basename glob patterns hidden by the file explorer,
	// appended to the built-in secret/clutter denylist (AUDIT_CONSOLE_DENY,
	// comma-separated, case-insensitive filepath.Match patterns).
	ConsoleDeny []string
}

// Root maps a display name (the agent) to a host directory the console exposes.
type Root struct {
	Name string
	Path string
}

var (
	ErrPortOutOfRange   = fmt.Errorf("port out of range [1,65535]")
	ErrRetentionInvalid = fmt.Errorf("retention hours must be >= 1")
)

// Load reads configuration from environment variables, applies defaults,
// and returns a validated Config.
func Load() (*Config, error) {
	port, err := envInt("AUDIT_PORT", 9090)
	if err != nil {
		return nil, fmt.Errorf("AUDIT_PORT: %w", err)
	}
	if port < 1 || port > 65535 {
		return nil, fmt.Errorf("AUDIT_PORT %d: %w", port, ErrPortOutOfRange)
	}

	// ponytail: legacy state-dir name preserved so existing audit DBs survive
	// the hermes→tentaflake rename; rename it in a future major.
	dbPath := envStr("AUDIT_DB_PATH", "/var/lib/hermes-audit/events.db")

	watchDirsRaw := envStr("AUDIT_WATCH_DIRS", "")
	var watchDirs []string
	if watchDirsRaw != "" {
		watchDirs = strings.Split(watchDirsRaw, ",")
		for i, d := range watchDirs {
			watchDirs[i] = strings.TrimSpace(d)
		}
	}

	retention, err := envInt("AUDIT_RETENTION_HOURS", 24)
	if err != nil {
		return nil, fmt.Errorf("AUDIT_RETENTION_HOURS: %w", err)
	}
	if retention < 1 {
		return nil, fmt.Errorf("AUDIT_RETENTION_HOURS %d: %w", retention, ErrRetentionInvalid)
	}

	consoleAddr := envStr("AUDIT_CONSOLE_ADDR", "127.0.0.1:9090")

	roots, err := parseRoots(envStr("AUDIT_CONSOLE_ROOTS", ""))
	if err != nil {
		return nil, fmt.Errorf("AUDIT_CONSOLE_ROOTS: %w", err)
	}

	var deny []string
	if raw := strings.TrimSpace(envStr("AUDIT_CONSOLE_DENY", "")); raw != "" {
		for _, p := range strings.Split(raw, ",") {
			if p = strings.TrimSpace(p); p != "" {
				deny = append(deny, p)
			}
		}
	}

	return &Config{
		Port:           port,
		DBPath:         dbPath,
		WatchDirs:      watchDirs,
		RetentionHours: retention,
		ConsoleAddr:    consoleAddr,
		ConsoleRoots:   roots,
		ConsoleDeny:    deny,
	}, nil
}

// parseRoots parses a comma-separated list of "name:path" pairs into Roots.
// Whitespace around each pair and field is trimmed; empty entries are skipped.
// Only the first ':' separates name from path, so absolute paths are fine.
func parseRoots(raw string) ([]Root, error) {
	raw = strings.TrimSpace(raw)
	if raw == "" {
		return nil, nil
	}
	var roots []Root
	for _, pair := range strings.Split(raw, ",") {
		pair = strings.TrimSpace(pair)
		if pair == "" {
			continue
		}
		name, path, ok := strings.Cut(pair, ":")
		name = strings.TrimSpace(name)
		path = strings.TrimSpace(path)
		if !ok || name == "" || path == "" {
			return nil, fmt.Errorf("invalid root %q: want \"name:path\"", pair)
		}
		roots = append(roots, Root{Name: name, Path: path})
	}
	return roots, nil
}

func envStr(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}

func envInt(key string, def int) (int, error) {
	v := os.Getenv(key)
	if v == "" {
		return def, nil
	}
	n, err := strconv.Atoi(v)
	if err != nil {
		return 0, fmt.Errorf("invalid %q value %q: %w", key, v, err)
	}
	return n, nil
}
