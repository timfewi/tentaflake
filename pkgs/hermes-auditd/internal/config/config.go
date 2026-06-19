// Package config loads the hermes-auditd configuration from environment variables.
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

// Config holds all configuration for hermes-auditd.
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
}

// Load reads configuration from environment variables, applies defaults,
// and returns a validated Config.
func Load() (*Config, error) {
	port, err := envInt("AUDIT_PORT", 9090)
	if err != nil {
		return nil, fmt.Errorf("AUDIT_PORT: %w", err)
	}
	if port < 1 || port > 65535 {
		return nil, fmt.Errorf("AUDIT_PORT %d out of range [1,65535]", port)
	}

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
		return nil, fmt.Errorf("AUDIT_RETENTION_HOURS %d must be >= 1", retention)
	}

	return &Config{
		Port:           port,
		DBPath:         dbPath,
		WatchDirs:      watchDirs,
		RetentionHours: retention,
	}, nil
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
