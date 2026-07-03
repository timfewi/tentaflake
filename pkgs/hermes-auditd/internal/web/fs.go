// Package web implements the Agent Console: a read-only HTTP server with a file
// explorer over the Hermes agent state dirs and a live activity monitor backed
// by the audit store. It exposes no write/delete/upload surface.
package web

import (
	"errors"
	"log/slog"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"tentaflake/hermes-auditd/internal/config"
)

// Sentinel errors returned by the explorer; the server maps them to HTTP codes.
var (
	ErrNotFound = errors.New("not found")
	ErrDenied   = errors.New("access denied")
)

// defaultDeny lists basename glob patterns the explorer always hides. It covers
// known Hermes secret files (.env, auth.json, config.yaml, tokens), generic
// credential material (keys, certs, ssh/aws/gnupg), and high-noise caches. The
// denylist is matched against the lowercased basename at EVERY path depth, so a
// secret nested deep in a tree is hidden just like one at the root — which a
// top-level allowlist could not guarantee.
var defaultDeny = []string{
	".env*", "*.env",
	"auth.json*", "auth.lock",
	"config.yaml*",
	"channel_directory.json",
	".hermes_history",
	"*.age", "*.key", "*.pem", "*.p12", "*.pfx", "*.crt",
	".git-credentials", ".netrc", ".npmrc", ".pypirc",
	"id_rsa*", "id_ed25519*", "id_ecdsa*", "known_hosts",
	"secret", "secrets", "*.secret", "token", "tokens", "credentials*",
	".ssh", ".aws", ".gnupg", ".gpg",
	".config", ".cache", ".npm", ".bun", ".local", ".venv",
	"node_modules", ".git", "__pycache__", ".ds_store",
	"audio_cache", ".scratch_tip_shown",
}

// root is one explorable top-level directory, with both its absolute path and
// its fully symlink-resolved form (used to detect symlinks escaping the root).
type root struct {
	name     string
	abs      string
	resolved string
}

// Explorer serves read-only views of a fixed set of agent roots.
type Explorer struct {
	roots map[string]root
	order []string // root names in declaration order, for stable UI listing
	deny  []string
}

// NewExplorer builds an Explorer from the configured roots. Roots whose path is
// missing or unreadable are skipped with a warning (e.g. an unmounted disk)
// rather than failing the whole server. Duplicate root names are an error.
func NewExplorer(cfgRoots []config.Root, extraDeny []string) (*Explorer, error) {
	e := &Explorer{
		roots: make(map[string]root, len(cfgRoots)),
		deny:  append(append([]string{}, defaultDeny...), lowerAll(extraDeny)...),
	}
	for _, cr := range cfgRoots {
		if _, dup := e.roots[cr.Name]; dup {
			return nil, errors.New("duplicate console root name: " + cr.Name)
		}
		abs, err := filepath.Abs(cr.Path)
		if err != nil {
			slog.Warn("console root: bad path, skipping", "name", cr.Name, "path", cr.Path, "error", err)
			continue
		}
		resolved, err := filepath.EvalSymlinks(abs)
		if err != nil {
			slog.Warn("console root: unreadable, skipping", "name", cr.Name, "path", abs, "error", err)
			continue
		}
		e.roots[cr.Name] = root{name: cr.Name, abs: abs, resolved: resolved}
		e.order = append(e.order, cr.Name)
	}
	return e, nil
}

// RootName is a top-level entry shown in the explorer sidebar.
type RootName struct {
	Name string `json:"name"`
}

// Roots returns the configured root names in declaration order.
func (e *Explorer) Roots() []RootName {
	out := make([]RootName, 0, len(e.order))
	for _, n := range e.order {
		out = append(out, RootName{Name: n})
	}
	return out
}

// Entry is a single directory child returned by List.
type Entry struct {
	Name  string    `json:"name"`
	IsDir bool      `json:"is_dir"`
	Size  int64     `json:"size"`
	MTime time.Time `json:"mtime"`
}

// List returns the (deny-filtered, sorted) children of agent's rel directory.
func (e *Explorer) List(agent, rel string) ([]Entry, error) {
	full, err := e.resolve(agent, rel)
	if err != nil {
		return nil, err
	}
	info, err := os.Stat(full)
	if err != nil {
		return nil, ErrNotFound
	}
	if !info.IsDir() {
		return nil, ErrDenied
	}
	dirents, err := os.ReadDir(full)
	if err != nil {
		return nil, err
	}
	out := make([]Entry, 0, len(dirents))
	for _, d := range dirents {
		if e.denied(d.Name()) {
			continue
		}
		fi, err := d.Info()
		if err != nil {
			continue
		}
		out = append(out, Entry{
			Name:  d.Name(),
			IsDir: fi.IsDir(),
			Size:  fi.Size(),
			MTime: fi.ModTime().UTC(),
		})
	}
	sort.Slice(out, func(i, j int) bool {
		if out[i].IsDir != out[j].IsDir {
			return out[i].IsDir // directories first
		}
		return strings.ToLower(out[i].Name) < strings.ToLower(out[j].Name)
	})
	return out, nil
}

// Stat resolves agent's rel path to an absolute file path and its FileInfo,
// enforcing all security checks. Used for read/download of regular files.
func (e *Explorer) Stat(agent, rel string) (string, os.FileInfo, error) {
	full, err := e.resolve(agent, rel)
	if err != nil {
		return "", nil, err
	}
	info, err := os.Stat(full)
	if err != nil {
		return "", nil, ErrNotFound
	}
	if info.IsDir() {
		return "", nil, ErrDenied
	}
	return full, info, nil
}

// resolve validates a (agent, rel) request and returns the absolute path. It
// neutralizes traversal by rooting + cleaning the relative path, rejects any
// denied path component, and rejects symlinks that resolve outside the root.
func (e *Explorer) resolve(agent, rel string) (string, error) {
	r, ok := e.roots[agent]
	if !ok {
		return "", ErrNotFound
	}
	// Anchor at "/" then Clean so any "../" collapses inside the root and can
	// never climb above it; strip the leading separator to get a safe relative.
	rel = strings.TrimPrefix(filepath.Clean("/"+rel), string(filepath.Separator))
	if rel != "" {
		for _, comp := range strings.Split(rel, string(filepath.Separator)) {
			if e.denied(comp) {
				return "", ErrDenied
			}
		}
	}
	full := filepath.Join(r.abs, rel)
	if !within(r.abs, full) {
		return "", ErrDenied
	}
	// Defend against symlinks pointing outside the root: resolve and re-check.
	resolved, err := filepath.EvalSymlinks(full)
	if err != nil {
		return "", ErrNotFound
	}
	if !within(r.resolved, resolved) {
		return "", ErrDenied
	}
	return full, nil
}

// within reports whether path is base itself or lies inside base.
func within(base, path string) bool {
	rel, err := filepath.Rel(base, path)
	if err != nil {
		return false
	}
	return rel == "." || (rel != ".." && !strings.HasPrefix(rel, ".."+string(filepath.Separator)))
}

// denied reports whether a basename matches any denylist pattern (case-insensitive).
func (e *Explorer) denied(name string) bool {
	lower := strings.ToLower(name)
	for _, pat := range e.deny {
		if ok, _ := filepath.Match(pat, lower); ok {
			return true
		}
	}
	return false
}

// openRegular opens path for reading only if it is a regular file, guarding
// against serving devices, FIFOs, or sockets that slipped through.
func openRegular(path string) (*os.File, error) {
	f, err := os.Open(path)
	if err != nil {
		return nil, ErrNotFound
	}
	info, err := f.Stat()
	if err != nil || !info.Mode().IsRegular() {
		f.Close()
		return nil, ErrDenied
	}
	return f, nil
}

func lowerAll(in []string) []string {
	out := make([]string, len(in))
	for i, s := range in {
		out[i] = strings.ToLower(s)
	}
	return out
}
