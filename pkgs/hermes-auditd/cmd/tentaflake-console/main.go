// Command tentaflake-console serves the Agent Console: a read-only web UI that
// combines a file explorer over the Hermes agent state dirs with a live activity
// monitor backed by the same audit database the hermes-auditd daemon writes.
//
// It opens events.db for queries only (no watcher, no pruning) and exposes a
// GET-only HTTP surface on a loopback address, meant to be published on the
// tailnet via `tailscale serve`.
package main

import (
	"context"
	"errors"
	"log/slog"
	"net/http"
	"os"
	"os/signal"
	"syscall"
	"time"

	"tentaflake/hermes-auditd/internal/config"
	"tentaflake/hermes-auditd/internal/store"
	"tentaflake/hermes-auditd/internal/web"
)

func main() {
	slog.SetDefault(slog.New(slog.NewJSONHandler(os.Stderr, &slog.HandlerOptions{
		Level: slog.LevelInfo,
	})))

	cfg, err := config.Load()
	if err != nil {
		slog.Error("config load failed", "error", err)
		os.Exit(1)
	}

	slog.Info("starting tentaflake-console",
		"addr", cfg.ConsoleAddr,
		"db_path", cfg.DBPath,
		"roots", len(cfg.ConsoleRoots),
	)

	st, err := store.New(cfg.DBPath, cfg.RetentionHours)
	if err != nil {
		slog.Error("store open failed", "error", err)
		os.Exit(1)
	}
	defer st.Close()

	exp, err := web.NewExplorer(cfg.ConsoleRoots, cfg.ConsoleDeny)
	if err != nil {
		slog.Error("explorer init failed", "error", err)
		os.Exit(1)
	}

	srv := &http.Server{
		Addr:              cfg.ConsoleAddr,
		Handler:           web.NewServer(st, exp).Handler(),
		ReadHeaderTimeout: 10 * time.Second,
	}

	go func() {
		slog.Info("tentaflake-console listening", "addr", cfg.ConsoleAddr)
		if err := srv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
			slog.Error("listen failed", "error", err)
			os.Exit(1)
		}
	}()

	sigCh := make(chan os.Signal, 1)
	signal.Notify(sigCh, syscall.SIGINT, syscall.SIGTERM)
	sig := <-sigCh
	slog.Info("shutting down", "signal", sig.String())

	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if err := srv.Shutdown(ctx); err != nil {
		slog.Error("shutdown error", "error", err)
	}
	slog.Info("shutdown complete")
}
