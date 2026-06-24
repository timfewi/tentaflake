package main

import (
	"context"
	"log/slog"
	"os"
	"os/signal"
	"sync"
	"syscall"

	"tentaflake/hermes-auditd/internal/config"
	"tentaflake/hermes-auditd/internal/store"
	"tentaflake/hermes-auditd/internal/watcher"
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

	// Start store consumer
	notifyCh, err := st.Start(ctx, eventCh)
	if err != nil {
		slog.Error("store start failed", "error", err)
		os.Exit(1)
	}

	// Start periodic prune loop
	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		defer wg.Done()
		st.PruneLoop(ctx)
	}()

	// Events are read back out of SQLite directly by the `hermes-top` TUI
	// (run over Tailscale SSH), so the daemon intentionally exposes no network
	// surface. The live notify channel is therefore unused here; drain it so a
	// full buffer never blocks the store's non-blocking send.
	go func() {
		for range notifyCh {
		}
	}()

	slog.Info("hermes-auditd running",
		"watch_count", len(cfg.WatchDirs),
	)

	// Wait for shutdown signal
	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)
	sig := <-sigCh

	slog.Info("shutting down", "signal", sig.String())
	cancel()

	// Wait for background goroutines to finish
	wg.Wait()
	slog.Info("shutdown complete")
}
