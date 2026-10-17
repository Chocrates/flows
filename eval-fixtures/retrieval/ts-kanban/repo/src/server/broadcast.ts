/**
 * WebSocket fan-out. Each connection subscribes to exactly one board;
 * route handlers call `publish` after a successful write.
 */
import type { IncomingMessage } from "node:http";
import { WebSocketServer, WebSocket } from "ws";
import type { Id, ServerEvent } from "../shared/types";
import { verifyToken } from "./auth";
import type { BoardDao } from "./dao";
import { roleOf } from "../shared/acl";

interface Conn {
  ws: WebSocket;
  userId: Id;
  boardId: Id;
  alive: boolean;
}

export class BoardHub {
  private rooms = new Map<Id, Set<Conn>>();
  private wss: WebSocketServer;

  constructor(private boards: BoardDao) {
    this.wss = new WebSocketServer({ noServer: true });
    setInterval(() => this.sweep(), 30_000).unref();
  }

  handleUpgrade(req: IncomingMessage, socket: import("node:net").Socket, head: Buffer): void {
    const url = new URL(req.url ?? "/", "http://x");
    const userId = verifyToken(url.searchParams.get("token") ?? "");
    const boardId = url.searchParams.get("board");
    const board = boardId ? this.boards.findBoard(boardId) : null;
    if (!userId || !board || roleOf(board, userId) === null) {
      socket.destroy();
      return;
    }
    this.wss.handleUpgrade(req, socket, head, (ws) => this.join(ws, userId, board.id));
  }

  private join(ws: WebSocket, userId: Id, boardId: Id): void {
    const conn: Conn = { ws, userId, boardId, alive: true };
    const room = this.rooms.get(boardId) ?? new Set();
    room.add(conn);
    this.rooms.set(boardId, room);
    ws.on("message", () => (conn.alive = true));
    ws.on("close", () => {
      room.delete(conn);
      if (room.size === 0) this.rooms.delete(boardId);
    });
  }

  /** Send `event` to every connection on `boardId`. */
  publish(boardId: Id, event: ServerEvent): void {
    const payload = JSON.stringify(event);
    for (const conn of this.rooms.get(boardId) ?? []) {
      if (conn.ws.readyState === WebSocket.OPEN) conn.ws.send(payload);
    }
  }

  /** Kick a user from a board's room, e.g. after removal from the board. */
  evict(boardId: Id, userId: Id): void {
    for (const conn of this.rooms.get(boardId) ?? []) {
      if (conn.userId === userId) conn.ws.close(4003, "removed");
    }
  }

  private sweep(): void {
    for (const room of this.rooms.values()) {
      for (const conn of room) {
        if (!conn.alive) conn.ws.terminate();
        conn.alive = false;
      }
    }
  }
}
