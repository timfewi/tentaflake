import { DatabaseSync } from 'node:sqlite';
import { lstatSync, chmodSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { createHash, randomUUID } from 'node:crypto';

export class Refusal extends Error {
  constructor(message, status = 409) { super(message); this.status = status; }
}
export const identity = value => typeof value === 'string' && /^[A-Za-z0-9_.:-]{1,128}$/.test(value);
export function privateDirectory(path, { groupTraverse = false } = {}) {
  if (resolve(path) !== path) throw new Refusal('Noncanonical state directory');
  let current = '/';
  for (const part of path.split('/').filter(Boolean)) {
    current = join(current, part);
    const s = lstatSync(current);
    if (!s.isDirectory() || s.isSymbolicLink()) throw new Refusal('Unsafe state directory');
  }
  const s = lstatSync(path);
  if (s.uid !== process.getuid() || (s.mode & 0o777) !== (groupTraverse ? 0o710 : 0o700) ||
      (groupTraverse && s.gid !== process.getgid())) throw new Refusal('State directory must have exact private ownership and mode');
}
export function stateFile(path) {
  try {
    const s = lstatSync(path);
    if (!s.isFile() || s.isSymbolicLink() || s.nlink !== 1 || s.uid !== process.getuid() || (s.mode & 0o077))
      throw new Refusal('Unsafe ledger file');
  } catch (e) { if (e.code !== 'ENOENT') throw e; }
}
function database(path) {
  for (const suffix of ['', '-journal', '-wal', '-shm']) stateFile(path + suffix);
  const db = new DatabaseSync(path);
  try {
    chmodSync(path, 0o600);
    db.exec('PRAGMA busy_timeout=0; PRAGMA journal_mode=DELETE; PRAGMA synchronous=FULL;');
    return db;
  } catch (error) { db.close(); throw error; }
}
export class Ledger {
  constructor(directory) {
    privateDirectory(directory);
    // Separate persistent lock database: SQLite releases its exclusive OS lock on
    // death, unlike a stale lock-file heuristic. Never delete/replace this file.
    try {
      this.owner = database(join(directory, 'owner.sqlite'));
      this.owner.exec('BEGIN EXCLUSIVE');
    } catch (error) {
      this.owner?.close();
      if (error instanceof Refusal) throw error;
      throw new Refusal('Another runner owns this state or its lock is unavailable');
    }
    try {
      this.db = database(join(directory, 'receipts.sqlite'));
      const version = this.db.prepare('PRAGMA user_version').get().user_version;
      if (version !== 0 && version !== 1) throw new Refusal('Unknown receipt schema');
      this.db.exec(`CREATE TABLE IF NOT EXISTS runs (
        id TEXT PRIMARY KEY, request_id TEXT UNIQUE NOT NULL, digest TEXT NOT NULL,
        session_id TEXT NOT NULL, input TEXT NOT NULL, state TEXT NOT NULL,
        result TEXT, submission_id TEXT, created_at INTEGER NOT NULL);
        CREATE TABLE IF NOT EXISTS operations (
        id TEXT PRIMARY KEY, run_id TEXT NOT NULL, task_id TEXT NOT NULL, kind TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS sessions (id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL);
        CREATE TABLE IF NOT EXISTS flags (name TEXT PRIMARY KEY, value TEXT NOT NULL);
        CREATE UNIQUE INDEX IF NOT EXISTS run_submission ON runs(submission_id);
        PRAGMA user_version=1;`);
      // An admitted-but-unsettled run is enough to quarantine, even if no send
      // was observed. This also covers the admission/SDK-submit crash window.
      this.db.prepare("UPDATE runs SET state='uncertain' WHERE state IN ('running','stopping')").run();
    } catch (e) { this.db?.close(); this.owner.close(); throw e; }
    this.stopped = false;
  }
  get quarantined() { return !!this.db.prepare("SELECT 1 FROM flags WHERE name='quarantined' UNION ALL SELECT 1 FROM runs WHERE state='uncertain' LIMIT 1").get(); }
  quarantine() { this.db.prepare("INSERT OR REPLACE INTO flags VALUES('quarantined','1')").run(); }
  get busy() { return !!this.db.prepare("SELECT 1 FROM runs WHERE state IN ('running','stopping') LIMIT 1").get(); }
  byRequest(id) { return this.db.prepare('SELECT * FROM runs WHERE request_id=?').get(id); }
  get(id) { return this.db.prepare('SELECT * FROM runs WHERE id=?').get(id); }
  admit(body) {
    const { request_id, session_id, input, context_mode } = body;
    if (!identity(request_id) || !identity(session_id) || context_mode !== 'persistent' ||
        typeof input !== 'string' || !input.trim() || Buffer.byteLength(input) > 65536 ||
        Object.keys(body).some(k => !['request_id','session_id','input','context_mode'].includes(k)))
      throw new Refusal('Invalid persistent run request', 400);
    const digest = createHash('sha256').update(JSON.stringify([session_id, input, context_mode])).digest('hex');
    const previous = this.byRequest(request_id);
    if (previous) {
      if (previous.digest !== digest) throw new Refusal('Request identity conflict');
      return { row: previous, duplicate: true };
    }
    if (this.stopped || this.quarantined || this.busy) throw new Refusal('Runner stopped, busy or quarantined');
    const id = randomUUID();
    this.db.prepare("INSERT INTO runs(id,request_id,digest,session_id,input,state,created_at) VALUES(?,?,?,?,?,'running',?)")
      .run(id, request_id, digest, session_id, input, Date.now());
    return { row: this.get(id), duplicate: false };
  }
  operation(runId, taskId, kind) {
    if (this.stopped || this.quarantined || this.get(runId)?.state !== 'running' || !identity(taskId))
      throw new Refusal('Outbound operation denied');
    const id = randomUUID();
    this.db.prepare('INSERT INTO operations VALUES(?,?,?,?)').run(id, runId, taskId, kind);
    return id;
  }
  uncertain(id) { this.db.prepare("UPDATE runs SET state='uncertain' WHERE id=? AND state IN ('running','stopping')").run(id); }
  stopping(id) { this.db.prepare("UPDATE runs SET state='stopping' WHERE id=? AND state='running'").run(id); }
  bind(id, submission) {
    if (!Number.isSafeInteger(submission) || submission <= 0) throw new Refusal('Invalid SDK submission');
    // SQLite coerces numeric parameters into TEXT as e.g. "9.0"; the
    // checkpoint audit looks up the canonical decimal SDK identity "9".
    this.db.prepare('UPDATE runs SET submission_id=? WHERE id=?').run(String(submission), id);
  }
  finish(id, state, result) {
    if (!['completed','failed'].includes(state) || this.get(id)?.state !== 'running')
      throw new Refusal('Cannot settle uncertain or stopped work');
    const encoded = JSON.stringify(result);
    if (Buffer.byteLength(encoded) > 1024 * 1024) { this.uncertain(id); throw new Refusal('Result exceeds limit'); }
    // Only the caller observing committed SDK settlement and idle state may use
    // this method. Receipts are retained; no automatic reconciliation operation.
    this.db.prepare('UPDATE runs SET state=?,result=? WHERE id=?').run(state, encoded, id);
  }
  session(id) {
    const row = this.db.prepare('SELECT conversation_id FROM sessions WHERE id=?').get(id);
    if (!row) return undefined;
    const value = Number(row.conversation_id);
    if (!Number.isSafeInteger(value) || value <= 0) throw new Refusal('Invalid persisted SDK session');
    return value;
  }
  bindSession(id, conversation) {
    if (!Number.isSafeInteger(conversation) || conversation <= 0) throw new Refusal('Invalid SDK session');
    this.db.prepare('INSERT INTO sessions VALUES(?,?)').run(id, conversation);
  }
  close() {
    if (this.closed) return;
    this.closed = true; this.stopped = true;
    try { this.db.close(); } finally { this.owner.close(); }
  }
}
