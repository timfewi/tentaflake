---
name: handle-the-host
description: Inspect or maintain a resolved Tentaflake host through its authorized private management path.
version: 2.0.0
---

# Handle the host

1. Resolve the operator-supplied host/account and existing authorization before
   mutations. Preserve authentication/recovery access. Local management flags
   do not prove enrollment or remote grants; follow
   [management policy](../../../docs/11-tailscale-management.md).
2. Connect through the approved private path (for Tailscale, `tailscale ssh
   <admin>@<host>`; an interactive sudo session may need `ssh -t`). Do not add
   passwordless sudo, Serve/Funnel or firewall holes to repair a connection.
3. Read `tentaflake status`, `health` and `doctor --security`. Resolve canonical
   IDs, units and state from `/etc/tentaflake/agents.tsv`, backend/flake from
   `/etc/tentaflake/cli.conf`. Never assume runtime prefixes or `/etc/nixos`.
4. Bound logs with `journalctl -u <exact-unit> --since '1 hour ago' -n 100
   --no-pager`. Never dump OCI environments or credentials. `--hide` redacts
   diagnostic names; other logs still need privacy review.
5. Perform only the authorized operation. Prefer `tentaflake start|stop|restart
   <id>`; stop includes loaded brokers. Scaffolds refuse execution. Build/review
   updates before authorized activation; quota migration, rollback, publication
   and credential rotation require authority covering their target/action.
6. Check affected services/application behavior afterward and report evidence
   gaps. Host readiness is not provider/vendor acceptance or Research VPN
   readiness. See [operations](../../../docs/07-operations.md).

For the optional [host API](../../../docs/19-web-ui.md), use canonical IDs,
separate action/log/worker grants and original request IDs after lost replies.
Draft configuration never applies itself. Review complete worker argv/digest;
first decision wins, approval preserves offline authority, and queued drain is
not execution success. Never kill/restart a worker just to force a drain.
