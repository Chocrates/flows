/**
 * Row <-> domain mapping and all SQL for boards, columns and cards.
 * Writes use optimistic concurrency: an update only applies if the stored
 * version equals the caller's base version.
 */
import type { BoardRecord, CardRecord, ColumnRecord, Id, RoleName } from "../shared/types";
import type { Db, Row } from "./db";

export class StaleWriteError extends Error {
  constructor(public current: unknown) {
    super("stale write");
  }
}

function toCard(r: Row): CardRecord {
  return {
    id: r.id as string,
    boardId: r.board_id as string,
    columnId: r.column_id as string,
    title: r.title as string,
    description: r.description as string,
    rank: r.rank as string,
    labels: JSON.parse(r.labels as string),
    assignees: JSON.parse(r.assignees as string),
    dueAt: (r.due_at as string | null) ?? null,
    archived: r.archived === 1,
    version: r.version as number,
    updatedAt: r.updated_at as string,
    updatedBy: r.updated_by as string,
  };
}

function toColumn(r: Row): ColumnRecord {
  return {
    id: r.id as string,
    boardId: r.board_id as string,
    name: r.name as string,
    rank: r.rank as string,
    wipLimit: (r.wip_limit as number | null) ?? null,
    version: r.version as number,
  };
}

export class BoardDao {
  constructor(private db: Db) {}

  findBoard(id: Id): BoardRecord | null {
    const b = this.db.get("SELECT * FROM boards WHERE id = ?", id);
    if (!b) return null;
    const members = this.db.all("SELECT * FROM members WHERE board_id = ?", id).map((m) => ({
      userId: m.user_id as string,
      displayName: m.display_name as string,
      role: m.role as RoleName,
      timeZone: m.time_zone as string,
      locale: m.locale as string,
    }));
    return {
      id: b.id as string,
      name: b.name as string,
      ownerId: b.owner_id as string,
      members,
      labels: JSON.parse(b.labels as string),
      version: b.version as number,
      createdAt: b.created_at as string,
    };
  }

  columnsOf(boardId: Id): ColumnRecord[] {
    return this.db.all("SELECT * FROM columns WHERE board_id = ? ORDER BY rank", boardId).map(toColumn);
  }

  findColumn(id: Id): ColumnRecord | null {
    const r = this.db.get("SELECT * FROM columns WHERE id = ?", id);
    return r ? toColumn(r) : null;
  }

  renameColumn(id: Id, name: string): ColumnRecord {
    this.db.run("UPDATE columns SET name = ?, version = version + 1 WHERE id = ?", name, id);
    return this.findColumn(id)!;
  }

  deleteColumn(id: Id): void {
    this.db.tx(() => {
      this.db.run("DELETE FROM cards WHERE column_id = ?", id);
      this.db.run("DELETE FROM columns WHERE id = ?", id);
    });
  }

  setRole(boardId: Id, userId: Id, role: RoleName): void {
    this.db.run("UPDATE members SET role = ? WHERE board_id = ? AND user_id = ?", role, boardId, userId);
  }
}

export class CardDao {
  constructor(private db: Db) {}

  find(id: Id): CardRecord | null {
    const r = this.db.get("SELECT * FROM cards WHERE id = ?", id);
    return r ? toCard(r) : null;
  }

  listByBoard(boardId: Id): CardRecord[] {
    return this.db.all("SELECT * FROM cards WHERE board_id = ? ORDER BY column_id, rank", boardId).map(toCard);
  }

  insert(c: CardRecord): CardRecord {
    this.db.run(
      `INSERT INTO cards (id, board_id, column_id, title, description, rank, labels, assignees,
                          due_at, archived, version, updated_at, updated_by)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)`,
      c.id, c.boardId, c.columnId, c.title, c.description, c.rank,
      JSON.stringify(c.labels), JSON.stringify(c.assignees), c.dueAt, c.archived ? 1 : 0,
      c.updatedAt, c.updatedBy,
    );
    return this.find(c.id)!;
  }

  /** Apply `patch` iff the stored version is `baseVersion`; else StaleWriteError. */
  update(id: Id, baseVersion: number, patch: Partial<CardRecord>, actor: Id): CardRecord {
    return this.db.tx(() => {
      const cur = this.find(id);
      if (!cur) throw new Error("not found");
      if (cur.version !== baseVersion) throw new StaleWriteError(cur);
      const next = { ...cur, ...patch };
      this.db.run(
        `UPDATE cards SET column_id = ?, title = ?, description = ?, rank = ?, labels = ?, assignees = ?,
                due_at = ?, archived = ?, version = version + 1, updated_at = ?, updated_by = ?
         WHERE id = ?`,
        next.columnId, next.title, next.description, next.rank, JSON.stringify(next.labels),
        JSON.stringify(next.assignees), next.dueAt, next.archived ? 1 : 0, new Date().toISOString(), actor, id,
      );
      return this.find(id)!;
    });
  }

  remove(id: Id): boolean {
    return this.db.run("DELETE FROM cards WHERE id = ?", id).changes > 0;
  }

  /** Rewrite every rank in a column with evenly spaced keys. */
  rewriteRanks(columnId: Id, ranks: Map<Id, string>): void {
    this.db.tx(() => {
      for (const [id, rank] of ranks) this.db.run("UPDATE cards SET rank = ? WHERE id = ? AND column_id = ?", rank, id, columnId);
    });
  }
}
