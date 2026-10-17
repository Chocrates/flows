/**
 * HTTP server bootstrap: JSON body parsing, auth, routers, websocket
 * upgrade, and a last-resort error handler.
 */
import express, { NextFunction, Request, Response } from "express";
import { createServer } from "node:http";
import { authenticate } from "./auth";
import { BoardHub } from "./broadcast";
import { BoardDao, CardDao } from "./dao";
import { Db } from "./db";
import { boardRouter } from "./routes/boardRoutes";
import { cardRouter } from "./routes/cardRoutes";

export interface ServerConfig {
  port: number;
  dbPath: string;
  corsOrigin: string;
}

export function buildServer(cfg: ServerConfig) {
  const db = new Db(cfg.dbPath);
  const boards = new BoardDao(db);
  const cards = new CardDao(db);
  const hub = new BoardHub(boards);

  const app = express();
  app.use(express.json({ limit: "256kb" }));
  app.use((req, res, next) => {
    res.setHeader("access-control-allow-origin", cfg.corsOrigin);
    res.setHeader("access-control-allow-headers", "authorization, content-type, x-client-id");
    res.setHeader("access-control-allow-methods", "GET, POST, PATCH, PUT, DELETE");
    if (req.method === "OPTIONS") return void res.status(204).end();
    next();
  });
  app.get("/healthz", (_req, res) => res.json({ ok: true }));
  app.use(authenticate);
  app.use(boardRouter(boards, cards, hub));
  app.use(cardRouter(boards, cards, hub));

  app.use((err: unknown, _req: Request, res: Response, _next: NextFunction) => {
    console.error(err);
    res.status(500).json({ code: "internal", message: "unexpected error" });
  });

  const server = createServer(app);
  server.on("upgrade", (req, socket, head) => {
    if (req.url?.startsWith("/live")) hub.handleUpgrade(req, socket as never, head);
    else socket.destroy();
  });
  return { app, server, db };
}

if (require.main === module) {
  const { server } = buildServer({
    port: Number(process.env.PORT ?? 8080),
    dbPath: process.env.DB_PATH ?? "kanban.sqlite",
    corsOrigin: process.env.CORS_ORIGIN ?? "http://localhost:5173",
  });
  server.listen(Number(process.env.PORT ?? 8080));
}
