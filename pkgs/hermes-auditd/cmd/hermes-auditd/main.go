package main

import (
	"context"
	"log/slog"
	"os"
	"os/signal"
	"syscall"
	"time"

	"github.com/timfewi/tentaflake/hermes-auditd/internal/config"
	"github.com/timfewi/tentaflake/hermes-auditd/internal/store"
	"github.com/timfewi/tentaflake/hermes-auditd/internal/watcher"
)

func main() {
	// Structured JSON logging to stderr
	slog.SetDefault(slog.New(slog.NewJSONHandler(os.Stderr, &slog.HandlerOptions{
		Level: slog.LevelInfo,
	})))

	cfg, err := config.Load()
	if err != nil {
		slog.Error("config load failed", "error", err)
		os.Exit(1)
	}

	slog.Info("starting hermes-auditd",
		"port", cfg.Port,
		"db_path", cfg.DBPath,
		"watch_dirs", cfg.WatchDirs,
		"retention_hours", cfg.RetentionHours,
	)

	// Initialize store
	st, err := store.New(cfg.DBPath, cfg.RetentionHours)
	if err != nil {
		slog.Error("store init failed", "error", err)
		os.Exit(1)
	}
	defer st.Close()

	// Initialize watcher
	watch, err := watcher.NewWatcher(cfg.WatchDirs)
	if err != nil {
		slog.Error("watcher init failed", "error", err)
		os.Exit(1)
	}

	// Context with cancel for graceful shutdown
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()

	// Start watcher
	eventCh := watch.Start(ctx)

	// Start store consumer — returns notify channel for future broadcast
	notifyCh, err := st.Start(ctx, eventCh)
	if err != nil {
		slog.Error("store start failed", "error", err)
		os.Exit(1)
	}

	// Start periodic prune loop
	go st.PruneLoop(ctx)

	// Discard notify channel until server package is implemented
	_ = notifyCh

	slog.Info("hermes-auditd running",
		"watch_count", len(cfg.WatchDirs),
	)

	// Wait for shutdown signal
	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)
	sig := <-sigCh

	slog.Info("shutting down", "signal", sig.String())
	cancel()

	// Allow brief cleanup
	time.Sleep(200 * time.Millisecond)
	slog.Info("shutdown complete")
}
