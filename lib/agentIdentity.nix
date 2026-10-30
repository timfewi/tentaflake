# Host identities are independent of the selected runtime.
{
  container = name: "agent-${name}";
  stateDir = name: "/var/lib/tentaflake/agents/${name}";
}
