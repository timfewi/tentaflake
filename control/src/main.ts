import { Auth } from "./auth";
import { loadConfig } from "./config";
import { Secrets } from "./crypto";
import { openDatabase } from "./db";
import { dockerClient } from "./docker";
import { Environments, ensureNetworks } from "./environments";
import { startServer } from "./server";

const configPath = Bun.argv[2];
if (configPath === undefined) {
  console.error("usage: bun run src/main.ts <config.json>");
  process.exit(2);
}
const config = loadConfig(configPath);
const secrets = Secrets.fromFile(config.keyFile);
const db = openDatabase(config.databasePath);
const docker = dockerClient(config.dockerSocket);
await ensureNetworks(docker);
const server = startServer(
  config,
  new Auth(db, secrets, config),
  new Environments(db, docker, secrets, config),
);
console.log(`tentaflake control service listening on ${server.url}`);
