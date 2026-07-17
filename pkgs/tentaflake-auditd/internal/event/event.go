// Package event defines the shared Event type used across all packages.
//
// This is the ONLY type shared between watcher, store, and server packages.
// Each package imports this type but has no other dependencies on each other.
package event

import "time"

// Event represents a single filesystem event detected by the watcher
// and persisted by the store. It is the sole shared data type across
// all packages in tentaflake-auditd.
type Event struct {
	ID        int64     `json:"id"`
	Agent     string    `json:"agent"`
	File      string    `json:"file"`
	Op        string    `json:"op"` // create, write, remove, rename, chmod
	Timestamp time.Time `json:"timestamp"`
	Size      int64     `json:"size,omitempty"`
}
