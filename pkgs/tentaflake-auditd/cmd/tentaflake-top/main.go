// Command tentaflake-top is a live terminal dashboard for Hermes agent activity.
//
// It reads the tentaflake-auditd SQLite database directly (no network surface — you
// run it inside a Tailscale SSH session) and shows, refreshing once a second:
//
//   - a per-agent activity table (events in the recent window, all-time total,
//     and the most recent file operation), and
//   - a live, scrollable log of filesystem events as agents create/write/remove
//     files in their state directories.
//
// It is a read-only view: it never writes events, only SELECTs.
package main

import (
	"context"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"

	"tentaflake/tentaflake-auditd/internal/event"
	"tentaflake/tentaflake-auditd/internal/store"
)

const (
	// ponytail: legacy state-dir name preserved so existing audit DBs survive
	// the hermes→tentaflake rename; rename it in a future major.
	defaultDBPath = "/var/lib/hermes-audit/events.db"
	maxLogLines   = 2000
)

// ── styles ──
var (
	titleStyle  = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("14"))
	dimStyle    = lipgloss.NewStyle().Foreground(lipgloss.Color("8"))
	headerStyle = lipgloss.NewStyle().Bold(true)
	errStyle    = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("9"))
	pausedStyle = lipgloss.NewStyle().Bold(true).Foreground(lipgloss.Color("11"))

	opColors = map[string]lipgloss.Style{
		"create": lipgloss.NewStyle().Foreground(lipgloss.Color("10")),
		"write":  lipgloss.NewStyle().Foreground(lipgloss.Color("14")),
		"remove": lipgloss.NewStyle().Foreground(lipgloss.Color("9")),
		"rename": lipgloss.NewStyle().Foreground(lipgloss.Color("11")),
		"chmod":  lipgloss.NewStyle().Foreground(lipgloss.Color("13")),
	}
)

// ── messages ──
type tickMsg time.Time

type refreshMsg struct {
	agents []store.AgentRow
	events []event.Event
	total  int
	err    error
}

// ── model ──
type model struct {
	st        *store.Store
	window    string // SQLite datetime modifier, e.g. "-300 seconds"
	windowLbl string // human label, e.g. "5m"
	interval  time.Duration
	hostname  string
	dbPath    string

	width, height int
	ready         bool

	agents      []store.AgentRow
	total       int
	logbuf      []event.Event // ascending by ID
	lastID      int64
	lastRefresh time.Time
	err         error

	filter string // "" = all agents
	paused bool
	scroll int // lines scrolled up from the bottom
}

func (m model) Init() tea.Cmd {
	return tea.Batch(m.refresh(), tickCmd(m.interval))
}

func tickCmd(d time.Duration) tea.Cmd {
	return tea.Tick(d, func(t time.Time) tea.Msg { return tickMsg(t) })
}

// refresh queries the store for the agent summary and any events newer than the
// ones already buffered. It is safe to run concurrently (the store serializes
// access via a single connection).
func (m model) refresh() tea.Cmd {
	st, window, afterID := m.st, m.window, m.lastID
	return func() tea.Msg {
		ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
		defer cancel()

		agents, err := st.AgentRows(ctx, window)
		if err != nil {
			return refreshMsg{err: err}
		}
		events, err := st.Since(ctx, afterID, maxLogLines)
		if err != nil {
			return refreshMsg{err: err}
		}
		total := 0
		for _, a := range agents {
			total += a.Total
		}
		return refreshMsg{agents: agents, events: events, total: total}
	}
}

func (m model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	switch msg := msg.(type) {
	case tickMsg:
		cmds := []tea.Cmd{tickCmd(m.interval)}
		if !m.paused {
			cmds = append(cmds, m.refresh())
		}
		return m, tea.Batch(cmds...)

	case refreshMsg:
		m.lastRefresh = time.Now()
		if msg.err != nil {
			m.err = msg.err
			return m, nil
		}
		m.err = nil
		m.agents = msg.agents
		m.total = msg.total
		if len(msg.events) > 0 {
			m.logbuf = append(m.logbuf, msg.events...)
			m.lastID = msg.events[len(msg.events)-1].ID
			if len(m.logbuf) > maxLogLines {
				m.logbuf = m.logbuf[len(m.logbuf)-maxLogLines:]
			}
		}
		return m, nil

	case tea.WindowSizeMsg:
		m.width, m.height, m.ready = msg.Width, msg.Height, true
		return m, nil

	case tea.KeyMsg:
		switch msg.String() {
		case "q", "ctrl+c", "esc":
			return m, tea.Quit
		case "p", " ":
			m.paused = !m.paused
		case "r":
			return m, m.refresh()
		case "up", "k":
			m.scroll++
		case "down", "j":
			if m.scroll > 0 {
				m.scroll--
			}
		case "pgup":
			m.scroll += 10
		case "pgdown":
			if m.scroll -= 10; m.scroll < 0 {
				m.scroll = 0
			}
		case "g", "home":
			m.scroll = 1 << 30 // clamped to top in View
		case "G", "end":
			m.scroll = 0
		case "f":
			m.cycleFilter()
		}
		return m, nil
	}
	return m, nil
}

// cycleFilter advances the agent filter: all → agent[0] → … → all.
func (m *model) cycleFilter() {
	names := make([]string, 0, len(m.agents))
	for _, a := range m.agents {
		names = append(names, a.Agent)
	}
	sort.Strings(names)
	if m.filter == "" {
		if len(names) > 0 {
			m.filter = names[0]
		}
		m.scroll = 0
		return
	}
	for i, n := range names {
		if n == m.filter {
			if i+1 < len(names) {
				m.filter = names[i+1]
			} else {
				m.filter = ""
			}
			m.scroll = 0
			return
		}
	}
	m.filter = ""
	m.scroll = 0
}

func (m model) View() string {
	if !m.ready {
		return "loading…"
	}
	if m.err != nil {
		return errStyle.Render("error: "+m.err.Error()) +
			dimStyle.Render("\n\nretrying every "+m.interval.String()+" · q to quit")
	}

	var b []byte
	out := func(s string) { b = append(b, s...) }

	// ── Header ──
	pause := ""
	if m.paused {
		pause = "  " + pausedStyle.Render("[PAUSED]")
	}
	out(titleStyle.Render("tentaflake-top") + "  " + headerStyle.Render(m.hostname) + pause + "\n")
	out(dimStyle.Render(fmt.Sprintf("%d events retained · window %s · updated %s",
		m.total, m.windowLbl, m.lastRefresh.Format("15:04:05"))) + "\n\n")

	// ── Agent table ──
	out(headerStyle.Render(fmt.Sprintf("  %-16s %8s %8s  %s", "AGENT", m.windowLbl, "TOTAL", "LAST ACTIVITY")) + "\n")
	if len(m.agents) == 0 {
		out(dimStyle.Render("  no events yet — agents have not touched their state dirs") + "\n")
	} else {
		agents := append([]store.AgentRow(nil), m.agents...)
		sort.Slice(agents, func(i, j int) bool {
			if agents[i].Recent != agents[j].Recent {
				return agents[i].Recent > agents[j].Recent
			}
			if agents[i].Total != agents[j].Total {
				return agents[i].Total > agents[j].Total
			}
			return agents[i].Agent < agents[j].Agent
		})
		for _, a := range agents {
			name := a.Agent
			if a.Agent == m.filter {
				name = "▶ " + name
			} else {
				name = "  " + name
			}
			nameStyle := dimStyle
			if a.Recent > 0 {
				nameStyle = lipgloss.NewStyle().Foreground(lipgloss.Color("10"))
			}
			last := dimStyle.Render("—")
			if !a.LastTime.IsZero() {
				last = opStyle(a.LastOp).Render(fmt.Sprintf("%-6s", a.LastOp)) + " " +
					dimStyle.Render(shortPath(a.LastFile, a.Agent))
			}
			out(fmt.Sprintf("%-18s %8d %8d  %s\n",
				nameStyle.Render(fmt.Sprintf("%-16s", name)), a.Recent, a.Total, last))
		}
	}

	// ── Live event log ──
	filtered := m.filtered()
	usedLines := 5 + max(1, len(m.agents)) + 3 // header(2)+blank+tablehdr + rows + log title + footer
	logHeight := m.height - usedLines
	if logHeight < 1 {
		logHeight = 1
	}

	title := "EVENTS"
	if m.filter != "" {
		title += " · " + m.filter
	}
	out("\n" + headerStyle.Render("  "+title) + dimStyle.Render(fmt.Sprintf("  (%d)", len(filtered))) + "\n")

	scroll := m.scroll
	if maxScroll := len(filtered) - logHeight; scroll > maxScroll {
		scroll = maxScroll
	}
	if scroll < 0 {
		scroll = 0
	}
	end := len(filtered) - scroll
	start := end - logHeight
	if start < 0 {
		start = 0
	}
	for i := start; i < end; i++ {
		e := filtered[i]
		out(fmt.Sprintf("  %s %s %s %s\n",
			dimStyle.Render(e.Timestamp.Local().Format("15:04:05")),
			lipgloss.NewStyle().Foreground(lipgloss.Color("12")).Render(fmt.Sprintf("%-12s", trunc(e.Agent, 12))),
			opStyle(e.Op).Render(fmt.Sprintf("%-6s", e.Op)),
			shortPath(e.File, e.Agent)))
	}

	// ── Footer ──
	out("\n" + dimStyle.Render("q quit · ↑↓/jk scroll · g/G top/bottom · f filter agent · p pause · r refresh"))
	return string(b)
}

func (m model) filtered() []event.Event {
	if m.filter == "" {
		return m.logbuf
	}
	out := make([]event.Event, 0, len(m.logbuf))
	for _, e := range m.logbuf {
		if e.Agent == m.filter {
			out = append(out, e)
		}
	}
	return out
}

func opStyle(op string) lipgloss.Style {
	if s, ok := opColors[op]; ok {
		return s
	}
	return dimStyle
}

// shortPath strips the conventional state-dir prefix so the log shows the path
// relative to the agent's state dir. Hermes labels are bare ("coding" →
// /var/lib/hermes-coding/), other runtimes keep their prefix in the label
// ("zeroclaw-assistant" → /var/lib/zeroclaw-assistant/).
func shortPath(path, agent string) string {
	for _, prefix := range []string{"/var/lib/hermes-" + agent + "/", "/var/lib/" + agent + "/"} {
		if len(path) > len(prefix) && path[:len(prefix)] == prefix {
			return path[len(prefix):]
		}
	}
	return path
}

func trunc(s string, n int) string {
	if len(s) <= n {
		return s
	}
	if n <= 1 {
		return s[:n]
	}
	return s[:n-1] + "…"
}

func main() {
	dbPath := flag.String("db", envOr("AUDIT_DB_PATH", defaultDBPath), "path to the tentaflake-auditd SQLite database")
	window := flag.Duration("window", 5*time.Minute, "recent-activity window for the per-agent counts")
	interval := flag.Duration("interval", time.Second, "refresh interval")
	flag.Parse()

	st, err := store.New(*dbPath, 1)
	if err != nil {
		fmt.Fprintf(os.Stderr, "tentaflake-top: cannot open audit database %s: %v\n", *dbPath, err)
		if os.IsPermission(err) {
			fmt.Fprintln(os.Stderr, "hint: you must be a member of the 'hermes-audit' group (or use sudo).")
		} else {
			fmt.Fprintln(os.Stderr, "hint: is tentaflake-auditd enabled? (tentaflake.auditd.enable = true)")
		}
		os.Exit(1)
	}
	defer st.Close()

	host, _ := os.Hostname()
	m := model{
		st:        st,
		window:    fmt.Sprintf("-%d seconds", int((*window).Seconds())),
		windowLbl: humanizeDuration(*window),
		interval:  *interval,
		hostname:  host,
		dbPath:    filepath.Clean(*dbPath),
	}

	if _, err := tea.NewProgram(m, tea.WithAltScreen()).Run(); err != nil {
		fmt.Fprintf(os.Stderr, "tentaflake-top: %v\n", err)
		os.Exit(1)
	}
}

func envOr(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}

func humanizeDuration(d time.Duration) string {
	switch {
	case d%time.Hour == 0:
		return fmt.Sprintf("%dh", int(d.Hours()))
	case d%time.Minute == 0:
		return fmt.Sprintf("%dm", int(d.Minutes()))
	default:
		return d.String()
	}
}
