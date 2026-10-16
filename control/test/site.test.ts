import { describe, expect, test } from "bun:test";
import { existsSync, readFileSync } from "node:fs";
import type { DomainStatus } from "../src/api";
import {
  type Client,
  call,
  defaultDomain,
  type Harness,
  harness,
  hostAddress,
  newUser,
  origin,
} from "./helpers";

const domain = "agents.example.com";
const derived = `https://${defaultDomain}`;
const status = async (response: Response) => (await response.json()) as DomainStatus;
const putDomain = (h: Harness, client: Client, value: unknown, at: string) =>
  call(h, "PUT", "/api/v1/domain", { domain: value }, client, { origin: at });

describe("domain", () => {
  test("a fixed origin cannot be changed", async () => {
    const h = harness();
    const { client } = await newUser(h, "admin", "admin");
    expect(await status(await call(h, "GET", "/api/v1/domain", undefined, client))).toEqual({
      domain: "tentaflake.example",
      custom: false,
      address: null,
      configurable: false,
    });
    expect((await putDomain(h, client, domain, origin)).status).toBe(409);
  });

  test("on the host the derived domain applies until the admin chooses one", async () => {
    const h = harness({ hostSite: true });
    // Signing in is a mutation, so it already needs the derived origin.
    const { client } = await newUser(h, "admin", "admin", derived);
    expect(await status(await call(h, "GET", "/api/v1/domain", undefined, client))).toEqual({
      domain: defaultDomain,
      custom: false,
      address: hostAddress,
      configurable: true,
    });

    // The domain must resolve to the server's address first.
    let response = await putDomain(h, client, domain, derived);
    expect(response.status).toBe(400);
    expect(((await response.json()) as { message: string }).message).toContain(hostAddress);
    h.resolved.set(domain, ["198.51.100.1"]);
    expect((await putDomain(h, client, domain, derived)).status).toBe(400);
    expect(existsSync(h.domainFile)).toBe(false);

    h.resolved.set(domain, ["198.51.100.1", hostAddress]);
    response = await putDomain(h, client, "Agents.Example.com.", derived);
    expect(response.status).toBe(200);
    expect(await status(response)).toMatchObject({ domain, custom: true });
    expect(readFileSync(h.domainFile, "utf8")).toBe(`${domain}\n`);

    // The origin follows at once: the derived one is refused from now on.
    expect((await putDomain(h, client, null, derived)).status).toBe(403);
    response = await putDomain(h, client, null, `https://${domain}`);
    expect(await status(response)).toMatchObject({ domain: defaultDomain, custom: false });
    expect(readFileSync(h.domainFile, "utf8")).toBe("");
  });

  test("refuses names that are not host names, and members", async () => {
    const h = harness({ hostSite: true });
    const { client } = await newUser(h, "admin", "admin", derived);
    for (const name of [
      "localhost",
      "203.0.113.7",
      "-a.example",
      "a..example",
      "a_b.example",
      "*.example.com",
      7,
    ]) {
      expect((await putDomain(h, client, name, derived)).status).toBe(400);
    }
    const member = await newUser(h, "member", "member", derived);
    h.resolved.set(domain, [hostAddress]);
    const response = await putDomain(h, member.client, domain, derived);
    expect(response.status).toBe(403);
    expect(((await response.json()) as { error: string }).error).toBe("forbidden");
  });
});
