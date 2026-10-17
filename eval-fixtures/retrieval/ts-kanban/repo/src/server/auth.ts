/**
 * Request authentication and board-membership guards.
 */
import type { NextFunction, Request, Response } from "express";
import { createHmac, timingSafeEqual } from "node:crypto";
import type { Capability } from "../shared/acl";
import { can, roleOf } from "../shared/acl";
import type { BoardRecord, Id, RoleName } from "../shared/types";
import type { BoardDao } from "./dao";

export interface AuthedRequest extends Request {
  userId: Id;
  clientId: string | null;
  board?: BoardRecord;
  role?: RoleName;
}

const SECRET = process.env.SESSION_SECRET ?? "dev-secret";

/** Tokens are `<userId>.<expiresEpochSec>.<hmac>`. */
export function verifyToken(token: string, now = Date.now()): Id | null {
  const [userId, exp, sig] = token.split(".");
  if (!userId || !exp || !sig) return null;
  const expected = createHmac("sha256", SECRET).update(`${userId}.${exp}`).digest("base64url");
  const a = Buffer.from(sig);
  const b = Buffer.from(expected);
  if (a.length !== b.length || !timingSafeEqual(a, b)) return null;
  if (Number(exp) * 1000 < now) return null;
  return userId;
}

export function authenticate(req: Request, res: Response, next: NextFunction): void {
  const header = req.headers.authorization ?? "";
  const userId = header.startsWith("Bearer ") ? verifyToken(header.slice(7)) : null;
  if (!userId) {
    res.status(401).json({ code: "forbidden", message: "not signed in" });
    return;
  }
  const r = req as AuthedRequest;
  r.userId = userId;
  r.clientId = (req.headers["x-client-id"] as string | undefined) ?? null;
  next();
}

/**
 * Load the board named by `resolveBoardId(req)` and require `cap`.
 * Attaches `board` and `role` to the request for downstream handlers.
 */
export function requireCapability(
  boards: BoardDao,
  cap: Capability,
  resolveBoardId: (req: AuthedRequest) => Id | null,
) {
  return (req: Request, res: Response, next: NextFunction): void => {
    const r = req as AuthedRequest;
    const boardId = resolveBoardId(r);
    const board = boardId ? boards.findBoard(boardId) : null;
    if (!board) {
      res.status(404).json({ code: "not_found", message: "board not found" });
      return;
    }
    const role = roleOf(board, r.userId);
    if (!can(role, cap)) {
      res.status(403).json({ code: "forbidden", message: `missing ${cap}` });
      return;
    }
    r.board = board;
    r.role = role ?? undefined;
    next();
  };
}

/** Any member, regardless of role, may read. */
export function requireMember(boards: BoardDao, resolveBoardId: (req: AuthedRequest) => Id | null) {
  return (req: Request, res: Response, next: NextFunction): void => {
    const r = req as AuthedRequest;
    const id = resolveBoardId(r);
    const board = id ? boards.findBoard(id) : null;
    if (!board || roleOf(board, r.userId) === null) {
      res.status(404).json({ code: "not_found", message: "board not found" });
      return;
    }
    r.board = board;
    r.role = roleOf(board, r.userId)!;
    next();
  };
}
