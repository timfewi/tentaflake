import { lookup } from "node:dns/promises";
import { readFileSync, renameSync, writeFileSync } from "node:fs";
import type { DomainStatus } from "./api";
import { isObject } from "./config";
import { randomToken } from "./crypto";
import { HttpError } from "./http";

/** A host name with at least two labels whose last label starts with a letter (so no IP address). */
export const domainPattern =
  /^([a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]([a-z0-9-]{0,61}[a-z0-9])?$/;
const validDomain = (value: string) => value.length <= 253 && domainPattern.test(value);

/** The IPv4 addresses a domain resolves to. */
export type Resolve = (domain: string) => Promise<string[]>;
const resolveIPv4: Resolve = async (domain) =>
  (await lookup(domain, { all: true, family: 4 })).map(({ address }) => address);

/** What the host writes into the site file: its public address and the name derived from it. */
interface HostSite {
  address: string | null;
  defaultDomain: string;
}

/**
 * Where the service answers. With a fixed `origin` (development, tests) that
 * is all. On the host, the site file names the server's address and the
 * domain derived from it, and the domain file holds the domain chosen in
 * setup; the host's HTTPS proxy follows the same two files.
 */
export class Site {
  private cache: { at: number; domain: string; custom: boolean; site: HostSite | null } | null =
    null;

  constructor(
    private readonly config: {
      origin: string | null;
      siteFile: string | null;
      domainFile: string | null;
    },
    private readonly resolve: Resolve = resolveIPv4,
    private readonly now: () => number = Date.now,
  ) {}

  /** The browser origin every mutation must come from. */
  origin(): string {
    return this.config.origin ?? `https://${this.read().domain}`;
  }

  status(): DomainStatus {
    if (this.config.origin !== null) {
      return {
        domain: new URL(this.config.origin).host,
        custom: false,
        address: null,
        configurable: false,
      };
    }
    const { domain, custom, site } = this.read();
    return { domain, custom, address: site?.address ?? null, configurable: true };
  }

  /**
   * Stores the domain the host serves from now on, or with null returns to
   * the derived one. A domain must already resolve to the server's address,
   * so that the switch does not lock its users out.
   */
  async setDomain(input: string | null): Promise<DomainStatus> {
    const { domainFile } = this.config;
    if (domainFile === null) {
      throw new HttpError(409, "conflict", "the origin is fixed by the service configuration");
    }
    const domain = input?.trim().toLowerCase().replace(/\.$/, "") || null;
    if (domain !== null) {
      if (!validDomain(domain)) {
        throw new HttpError(
          400,
          "invalid_request",
          "domain must be a host name such as agents.example.com",
        );
      }
      const { address } = this.read().site ?? { address: null };
      if (address !== null) {
        const addresses = await this.resolve(domain).catch(() => [] as string[]);
        if (!addresses.includes(address)) {
          throw new HttpError(
            400,
            "invalid_request",
            `${domain} does not resolve to this server: set its A record to ${address} first`,
          );
        }
      }
    }
    // Replaced, never removed: the host watches the file for changes.
    const temporary = `${domainFile}.${randomToken(9)}.tmp`;
    writeFileSync(temporary, domain === null ? "" : `${domain}\n`, { mode: 0o600, flag: "wx" });
    renameSync(temporary, domainFile);
    this.cache = null;
    return this.status();
  }

  /** The current domain and host site, re-read at most once a second. */
  private read() {
    if (this.cache !== null && this.now() - this.cache.at < 1000) return this.cache;
    const { siteFile, domainFile } = this.config;
    const site = siteFile === null ? null : parseSite(readFileSync(siteFile, "utf8"));
    const chosen = domainFile === null ? "" : readOptional(domainFile).trim();
    const custom = validDomain(chosen);
    const domain = custom ? chosen : site?.defaultDomain;
    if (domain === undefined) throw new Error("the site has no domain");
    this.cache = { at: this.now(), domain, custom, site };
    return this.cache;
  }
}

function parseSite(text: string): HostSite {
  const site: unknown = JSON.parse(text);
  if (
    !isObject(site) ||
    typeof site.defaultDomain !== "string" ||
    !validDomain(site.defaultDomain) ||
    (site.address !== null && typeof site.address !== "string")
  ) {
    throw new Error("the site file must hold { address, defaultDomain }");
  }
  return { address: site.address as string | null, defaultDomain: site.defaultDomain };
}

function readOptional(path: string): string {
  try {
    return readFileSync(path, "utf8");
  } catch (error) {
    if ((error as NodeJS.ErrnoException).code === "ENOENT") return "";
    throw error;
  }
}
