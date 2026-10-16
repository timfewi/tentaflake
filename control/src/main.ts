import { Agent, IdleStop } from "./agent";
import { Auth } from "./auth";
import { BrokerFiles, owner } from "./broker";
import { loadConfig } from "./config";
import { Secrets } from "./crypto";
import { openDatabase } from "./db";
import { dockerClient } from "./docker";
import { Environments, ensureNetworks } from "./environments";
import { Keys } from "./keys";
import { startServer } from "./server";
import { Site } from "./site";
import { uiServer } from "./ui";

const configPath = Bun.argv[2];
if (configPath === undefined) {
  console.error("usage: bun run src/main.ts <config.json>");
  process.exit(2);
}
const config = loadConfig(configPath);
const secrets = Secrets.fromFile(config.keyFile);
const db = openDatabase(config.databasePath);
const docker = dockerClient(config.dockerSocket);
const gateway = await ensureNetworks(docker);
const brokerOwner = config.brokerUser === null ? null : owner(config.brokerUser);
const broker = new BrokerFiles(db, secrets, config.brokerDirectory, brokerOwner);
broker.sync();
const brokerUrl = `http://${gateway}:${config.brokerPort}/v1`;
const environments = new Environments(db, docker, secrets, broker, { ...config, brokerUrl });
const agent = new Agent(environments);
const site = new Site(config);
const idle = new IdleStop(environments, agent, config.environmentIdleMinutes * 60_000);
setInterval(() => idle.run().catch((error) => console.error(`idle stop: ${error}`)), 60_000);
const server = startServer(
  config,
  site,
  new Auth(db, secrets, config),
  environments,
  new Keys(db, secrets, broker),
  agent,
  config.uiDirectory === null ? undefined : uiServer(config.uiDirectory, () => site.origin()),
);
console.log(`tentaflake control service listening on ${server.url}`);
