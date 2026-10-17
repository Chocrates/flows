/**
 * Synchronous SQLite wrapper (better-sqlite3 style) with a tiny migration
 * runner and a transaction helper.
 */
import Database from "better-sqlite3";

export type Row = Record<string, unknown>;

const MIGRATIONS: string[] = [
  `CREATE TABLE boards (
     id TEXT PRIMARY KEY, name TEXT NOT NULL, owner_id TEXT NOT NULL,
     labels TEXT NOT NULL DEFAULT '[]', version INTEGER NOT NULL DEFAULT 1,
     created_at TEXT NOT NULL)`,
  `CREATE TABLE members (
     board_id TEXT NOT NULL, user_id TEXT NOT NULL, display_name TEXT NOT NULL,
     role TEXT NOT NULL, time_zone TEXT NOT NULL DEFAULT 'UTC', locale TEXT NOT NULL DEFAULT 'en',
     PRIMARY KEY (board_id, user_id))`,
  `CREATE TABLE columns (
     id TEXT PRIMARY KEY, board_id TEXT NOT NULL, name TEXT NOT NULL, rank TEXT NOT NULL,
     wip_limit INTEGER, version INTEGER NOT NULL DEFAULT 1)`,
  `CREATE TABLE cards (
     id TEXT PRIMARY KEY, board_id TEXT NOT NULL, column_id TEXT NOT NULL,
     title TEXT NOT NULL, description TEXT NOT NULL DEFAULT '', rank TEXT NOT NULL,
     labels TEXT NOT NULL DEFAULT '[]', assignees TEXT NOT NULL DEFAULT '[]',
     due_at TEXT, archived INTEGER NOT NULL DEFAULT 0, version INTEGER NOT NULL DEFAULT 1,
     updated_at TEXT NOT NULL, updated_by TEXT NOT NULL)`,
  `CREATE INDEX cards_by_column ON cards (column_id, rank)`,
];

export class Db {
  private conn: Database.Database;

  constructor(path: string) {
    this.conn = new Database(path);
    this.conn.pragma("journal_mode = WAL");
    this.conn.pragma("foreign_keys = ON");
    this.migrate();
  }

  private migrate(): void {
    this.conn.exec("CREATE TABLE IF NOT EXISTS _migrations (n INTEGER PRIMARY KEY)");
    const done = new Set((this.conn.prepare("SELECT n FROM _migrations").all() as { n: number }[]).map((r) => r.n));
    MIGRATIONS.forEach((sql, n) => {
      if (done.has(n)) return;
      this.tx(() => {
        this.conn.exec(sql);
        this.conn.prepare("INSERT INTO _migrations (n) VALUES (?)").run(n);
      });
    });
  }

  all<T = Row>(sql: string, ...params: unknown[]): T[] {
    return this.conn.prepare(sql).all(...params) as T[];
  }

  get<T = Row>(sql: string, ...params: unknown[]): T | undefined {
    return this.conn.prepare(sql).get(...params) as T | undefined;
  }

  run(sql: string, ...params: unknown[]): { changes: number } {
    const r = this.conn.prepare(sql).run(...params);
    return { changes: r.changes };
  }

  tx<T>(fn: () => T): T {
    return this.conn.transaction(fn)();
  }

  close(): void {
    this.conn.close();
  }
}
